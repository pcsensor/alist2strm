use axum::{
    extract::{Path, State},
    http::header,
    middleware,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Json, Response,
    },
    routing::{get, post, put},
    Router,
};
use futures_util::stream::Stream;
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::sync::Arc;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::alist::AlistClient;
use crate::auth::{hash_password, require_admin_auth, verify_password, SessionManager};
use crate::config::{AlistConfig, ConfigStore, StrmTaskConfig};
use crate::task::{TaskManager, TaskRuntimeInfo};

#[derive(Clone)]
pub struct AppState {
    pub config_store: ConfigStore,
    pub task_manager: Arc<TaskManager>,
    pub session_manager: Arc<SessionManager>,
}

#[derive(Serialize)]
struct ApiResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl ApiResponse {
    fn ok<T: Serialize>(data: T) -> Json<Self> {
        Json(Self {
            success: true,
            data: serde_json::to_value(data).ok(),
            error: None,
        })
    }

    fn err(msg: impl Into<String>) -> Json<Self> {
        Json(Self {
            success: false,
            data: None,
            error: Some(msg.into()),
        })
    }
}

pub fn create_api_routes(state: AppState) -> Router {
    // 公开接口（无需登录）
    let public_routes = Router::new()
        .route("/api/status", get(get_status))
        .route("/api/auth/me", get(get_auth_me))
        .route("/api/auth/init", post(init_admin_auth))
        .route("/api/auth/login", post(login));

    // 受保护接口（必须管理员鉴权）
    let protected_routes = Router::new()
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/password", post(change_password))
        .route("/api/config", get(get_config).post(update_config))
        .route("/api/alist/test", post(test_alist))
        .route("/api/tasks", get(list_tasks).post(create_task))
        .route(
            "/api/tasks/:id",
            put(update_task).delete(delete_task),
        )
        .route("/api/tasks/:id/run", post(run_task_manually))
        .route("/api/tasks/events", get(sse_task_events))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_admin_auth,
        ));

    Router::new()
        .merge(public_routes)
        .merge(protected_routes)
        .with_state(state)
}

async fn get_status() -> impl IntoResponse {
    ApiResponse::ok(serde_json::json!({
        "name": "alist2strm",
        "version": env!("CARGO_PKG_VERSION"),
        "status": "online"
    }))
}

// ----------------- 认证相关 API -----------------

#[derive(Deserialize)]
struct InitAuthReq {
    username: String,
    password: String,
}

/// 初次使用时设置管理员账号密码
async fn init_admin_auth(
    State(state): State<AppState>,
    Json(payload): Json<InitAuthReq>,
) -> Response {
    let mut cfg = state.config_store.get().await;

    if cfg.auth.initialized {
        return ApiResponse::err("管理员账号已经设置，禁止重复初始化").into_response();
    }

    let username = payload.username.trim();
    if username.is_empty() {
        return ApiResponse::err("用户名不能为空").into_response();
    }

    let password = payload.password.trim();
    if password.len() < 5 {
        return ApiResponse::err("密码长度不能少于 5 位").into_response();
    }

    let hashed = hash_password(password, &cfg.auth.secret);
    cfg.auth.username = username.to_string();
    cfg.auth.password = hashed;
    cfg.auth.initialized = true;

    if let Err(e) = state.config_store.update(cfg.clone()).await {
        return ApiResponse::err(format!("保存管理员账号失败: {}", e)).into_response();
    }

    let token = state.session_manager.create_token().await;
    (
        [(header::SET_COOKIE, format!("auth_token={}; Path=/; HttpOnly; SameSite=Lax", token))],
        ApiResponse::ok(serde_json::json!({
            "token": token,
            "username": cfg.auth.username,
        })),
    )
        .into_response()
}

#[derive(Deserialize)]
struct LoginReq {
    username: String,
    password: String,
}

async fn login(
    State(state): State<AppState>,
    Json(payload): Json<LoginReq>,
) -> Response {
    let cfg = state.config_store.get().await;

    if !cfg.auth.enabled {
        let token = state.session_manager.create_token().await;
        return (
            [(header::SET_COOKIE, format!("auth_token={}; Path=/; HttpOnly; SameSite=Lax", token))],
            ApiResponse::ok(serde_json::json!({
                "token": token,
                "username": cfg.auth.username,
            })),
        )
            .into_response();
    }

    if !cfg.auth.initialized {
        return ApiResponse::err("系统尚未初始化管理员账号，请先设置").into_response();
    }

    if payload.username.trim() == cfg.auth.username
        && verify_password(&payload.password, &cfg.auth.password, &cfg.auth.secret)
    {
        let token = state.session_manager.create_token().await;
        (
            [(header::SET_COOKIE, format!("auth_token={}; Path=/; HttpOnly; SameSite=Lax", token))],
            ApiResponse::ok(serde_json::json!({
                "token": token,
                "username": cfg.auth.username,
            })),
        )
            .into_response()
    } else {
        ApiResponse::err("用户名或密码错误").into_response()
    }
}

async fn get_auth_me(
    State(state): State<AppState>,
    req: axum::extract::Request,
) -> impl IntoResponse {
    let cfg = state.config_store.get().await;
    if !cfg.auth.enabled {
        return ApiResponse::ok(serde_json::json!({
            "auth_enabled": false,
            "initialized": true,
            "logged_in": true,
            "username": "",
        }));
    }

    let mut is_logged_in = false;

    if let Some(auth_header) = req.headers().get(header::AUTHORIZATION) {
        if let Ok(val) = auth_header.to_str() {
            if val.starts_with("Bearer ") {
                let token = val[7..].trim();
                is_logged_in = state.session_manager.validate_token(token).await;
            }
        }
    }

    if !is_logged_in {
        if let Some(cookie_header) = req.headers().get(header::COOKIE) {
            if let Ok(val) = cookie_header.to_str() {
                for part in val.split(';') {
                    let pair: Vec<&str> = part.trim().split('=').collect();
                    if pair.len() == 2 && pair[0] == "auth_token" {
                        is_logged_in = state.session_manager.validate_token(pair[1]).await;
                        break;
                    }
                }
            }
        }
    }

    ApiResponse::ok(serde_json::json!({
        "auth_enabled": true,
        "initialized": cfg.auth.initialized,
        "logged_in": is_logged_in,
        "username": if is_logged_in { cfg.auth.username.clone() } else { String::new() },
    }))
}

async fn logout(
    State(state): State<AppState>,
    req: axum::extract::Request,
) -> impl IntoResponse {
    if let Some(auth_header) = req.headers().get(header::AUTHORIZATION) {
        if let Ok(val) = auth_header.to_str() {
            if val.starts_with("Bearer ") {
                let token = val[7..].trim();
                state.session_manager.revoke_token(token).await;
            }
        }
    }
    (
        [(header::SET_COOKIE, "auth_token=; Path=/; Max-Age=0")],
        ApiResponse::ok("已退出登录"),
    )
}

#[derive(Deserialize)]
struct ChangePasswordReq {
    old_password: String,
    new_password: String,
}

async fn change_password(
    State(state): State<AppState>,
    Json(payload): Json<ChangePasswordReq>,
) -> impl IntoResponse {
    let mut cfg = state.config_store.get().await;

    if !verify_password(&payload.old_password, &cfg.auth.password, &cfg.auth.secret) {
        return ApiResponse::err("旧密码验证失败");
    }

    if payload.new_password.trim().len() < 5 {
        return ApiResponse::err("新密码长度不能少于 5 位");
    }

    let hashed = hash_password(payload.new_password.trim(), &cfg.auth.secret);
    cfg.auth.password = hashed;

    if let Err(e) = state.config_store.update(cfg).await {
        return ApiResponse::err(e);
    }

    ApiResponse::ok("密码修改成功")
}

// ----------------- 配置与 AList API -----------------

async fn get_config(State(state): State<AppState>) -> impl IntoResponse {
    let cfg = state.config_store.get().await;
    ApiResponse::ok(cfg)
}

#[derive(Deserialize)]
struct UpdateConfigReq {
    alist: AlistConfig,
}

async fn update_config(
    State(state): State<AppState>,
    Json(payload): Json<UpdateConfigReq>,
) -> impl IntoResponse {
    let mut current = state.config_store.get().await;
    current.alist = payload.alist;

    if let Err(e) = state.config_store.update(current).await {
        return ApiResponse::err(e);
    }

    ApiResponse::ok("配置已更新")
}

#[derive(Deserialize)]
struct AlistTestReq {
    base_url: String,
    public_url: Option<String>,
    token: String,
}

async fn test_alist(Json(req): Json<AlistTestReq>) -> impl IntoResponse {
    let client = match AlistClient::new(req.base_url, req.public_url, req.token, 0.0) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e),
    };

    match client.test_connection().await {
        Ok(info) => ApiResponse::ok(serde_json::json!({
            "connected": true,
            "username": info.username.unwrap_or_else(|| "guest".to_string()),
            "base_path": info.base_path,
        })),
        Err(e) => ApiResponse::err(e),
    }
}

// ----------------- 任务管理 API -----------------

#[derive(Serialize)]
struct TaskViewItem {
    #[serde(flatten)]
    config: StrmTaskConfig,
    runtime: TaskRuntimeInfo,
}

async fn list_tasks(State(state): State<AppState>) -> impl IntoResponse {
    let cfg = state.config_store.get().await;
    let runtimes = state.task_manager.get_all_status().await;

    let items: Vec<TaskViewItem> = cfg
        .tasks
        .into_iter()
        .map(|t| {
            let rt = runtimes
                .iter()
                .find(|r| r.task_id == t.id)
                .cloned()
                .unwrap_or(TaskRuntimeInfo {
                    task_id: t.id.clone(),
                    status: crate::task::TaskRunStatus::Idle,
                    last_run: None,
                    last_duration_secs: None,
                    last_stats: None,
                    last_error: None,
                });

            TaskViewItem {
                config: t,
                runtime: rt,
            }
        })
        .collect();

    ApiResponse::ok(items)
}

async fn create_task(
    State(state): State<AppState>,
    Json(mut new_task): Json<StrmTaskConfig>,
) -> impl IntoResponse {
    if new_task.id.trim().is_empty() {
        new_task.id = uuid::Uuid::new_v4().to_string();
    }
    if new_task.name.trim().is_empty() {
        new_task.name = "未命名任务".to_string();
    }

    let mut cfg = state.config_store.get().await;
    cfg.tasks.push(new_task.clone());

    if let Err(e) = state.config_store.update(cfg).await {
        return ApiResponse::err(e);
    }

    let _ = state.task_manager.reload_scheduler().await;
    ApiResponse::ok(new_task)
}

async fn update_task(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(updated): Json<StrmTaskConfig>,
) -> impl IntoResponse {
    let mut cfg = state.config_store.get().await;
    let idx = cfg.tasks.iter().position(|t| t.id == id);

    let idx = match idx {
        Some(i) => i,
        None => return ApiResponse::err("未找到该任务"),
    };

    let mut final_task = updated;
    final_task.id = id;
    cfg.tasks[idx] = final_task.clone();

    if let Err(e) = state.config_store.update(cfg).await {
        return ApiResponse::err(e);
    }

    let _ = state.task_manager.reload_scheduler().await;
    ApiResponse::ok(final_task)
}

async fn delete_task(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let mut cfg = state.config_store.get().await;
    let initial_len = cfg.tasks.len();
    cfg.tasks.retain(|t| t.id != id);

    if cfg.tasks.len() == initial_len {
        return ApiResponse::err("未找到该任务");
    }

    if let Err(e) = state.config_store.update(cfg).await {
        return ApiResponse::err(e);
    }

    let _ = state.task_manager.reload_scheduler().await;
    ApiResponse::ok("删除成功")
}

async fn run_task_manually(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    match state.task_manager.clone().trigger_task(id).await {
        Ok(()) => ApiResponse::ok("任务已启动"),
        Err(e) => ApiResponse::err(e),
    }
}

async fn sse_task_events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.task_manager.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|msg| match msg {
        Ok(evt) => {
            let json = serde_json::to_string(&evt).unwrap_or_default();
            Some(Ok(Event::default().data(json)))
        }
        Err(_) => None,
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

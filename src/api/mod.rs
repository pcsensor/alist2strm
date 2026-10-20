use axum::{
    extract::{Path, State},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Json,
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
use crate::config::{AlistConfig, ConfigStore, StrmTaskConfig};
use crate::task::{TaskManager, TaskRuntimeInfo};

#[derive(Clone)]
pub struct AppState {
    pub config_store: ConfigStore,
    pub task_manager: Arc<TaskManager>,
}

#[derive(Serialize)]
struct ApiResponse<T> {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl<T: Serialize> ApiResponse<T> {
    fn ok(data: T) -> Json<Self> {
        Json(Self {
            success: true,
            data: Some(data),
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
    Router::new()
        .route("/api/status", get(get_status))
        .route("/api/config", get(get_config).post(update_config))
        .route("/api/alist/test", post(test_alist))
        .route("/api/tasks", get(list_tasks).post(create_task))
        .route(
            "/api/tasks/:id",
            put(update_task).delete(delete_task),
        )
        .route("/api/tasks/:id/run", post(run_task_manually))
        .route("/api/tasks/events", get(sse_task_events))
        .with_state(state)
}

async fn get_status() -> impl IntoResponse {
    ApiResponse::ok(serde_json::json!({
        "name": "alist2strm",
        "version": env!("CARGO_PKG_VERSION"),
        "status": "online"
    }))
}

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

/// 手动执行任务核心入口
async fn run_task_manually(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    match state.task_manager.clone().trigger_task(id).await {
        Ok(()) => ApiResponse::ok("任务已启动"),
        Err(e) => ApiResponse::err(e),
    }
}

/// SSE 实时进度与日志流
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

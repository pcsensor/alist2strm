use alist2strm::api::{create_api_routes, AppState};
use alist2strm::auth::SessionManager;
use alist2strm::config::ConfigStore;
use alist2strm::task::TaskManager;
use axum::{
    body::Body,
    http::{header, Request, StatusCode},
};
use std::sync::Arc;
use tempfile::tempdir;
use tower::util::ServiceExt;

#[tokio::test]
async fn test_admin_initialization_and_auth_workflow() {
    let tmp = tempdir().unwrap();
    let cfg_path = tmp.path().join("config.yaml");

    let config_store = ConfigStore::new(cfg_path);
    let task_manager = Arc::new(TaskManager::new(config_store.clone()));
    let session_manager = Arc::new(SessionManager::new());

    let state = AppState {
        config_store,
        task_manager,
        session_manager,
    };

    let app = create_api_routes(state);

    // 1. 初次启动，查询 /api/auth/me，应处于未初始化状态 (initialized: false)
    let me_res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/auth/me")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(me_res.status(), StatusCode::OK);
    let body_bytes = axum::body::to_bytes(me_res.into_body(), usize::MAX).await.unwrap();
    let me_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(me_json["data"]["initialized"], false);
    assert_eq!(me_json["data"]["logged_in"], false);

    // 2. 未初始化时直接登录，应当被拒绝
    let login_premature = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({
                        "username": "admin",
                        "password": "any"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let body_bytes = axum::body::to_bytes(login_premature.into_body(), usize::MAX).await.unwrap();
    let premature_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(premature_json["success"], false);

    // 3. 用户初次设置管理员账号和密码 (如 myadmin / mySecretPassword789)
    let init_res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/init")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({
                        "username": "myadmin",
                        "password": "mySecretPassword789"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(init_res.status(), StatusCode::OK);
    let body_bytes = axum::body::to_bytes(init_res.into_body(), usize::MAX).await.unwrap();
    let init_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(init_json["success"], true);
    assert_eq!(init_json["data"]["username"], "myadmin");
    let token = init_json["data"]["token"].as_str().unwrap().to_string();
    assert!(!token.is_empty());

    // 4. 重复初始化应当被拒绝
    let init_repeat = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/init")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({
                        "username": "hacker",
                        "password": "hackerpassword"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let body_bytes = axum::body::to_bytes(init_repeat.into_body(), usize::MAX).await.unwrap();
    let repeat_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(repeat_json["success"], false);

    // 5. 携带初始化获得的 Bearer Token 访问受保护接口
    let authed_res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/tasks")
                .header(header::AUTHORIZATION, format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(authed_res.status(), StatusCode::OK);

    // 6. 用刚才新设置的用户名与密码正常登录
    let login_custom = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({
                        "username": "myadmin",
                        "password": "mySecretPassword789"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let body_bytes = axum::body::to_bytes(login_custom.into_body(), usize::MAX).await.unwrap();
    let custom_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(custom_json["success"], true);
    assert_eq!(custom_json["data"]["username"], "myadmin");

    println!("初次由用户自定义设置管理员账号密码的完整流程测试全部通过！");
}

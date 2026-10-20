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
use tower::ServiceExt; // for oneshot

#[tokio::test]
async fn test_admin_authentication_workflow() {
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

    // 1. 未登录访问受保护接口，应当返回 401 Unauthorized
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/tasks")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // 2. 错误密码登录，返回失败
    let login_bad = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({
                        "username": "admin",
                        "password": "wrongpassword"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let body_bytes = axum::body::to_bytes(login_bad.into_body(), usize::MAX).await.unwrap();
    let bad_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(bad_json["success"], false);

    // 3. 正确默认密码登录 (admin / adminadmin)
    let login_good = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({
                        "username": "admin",
                        "password": "adminadmin"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let body_bytes = axum::body::to_bytes(login_good.into_body(), usize::MAX).await.unwrap();
    let good_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(good_json["success"], true);
    let token = good_json["data"]["token"].as_str().unwrap().to_string();
    assert!(!token.is_empty());

    // 4. 携带 Bearer Token 访问受保护接口，应当成功返回 200
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

    // 5. 修改密码
    let change_pwd_res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/password")
                .header(header::AUTHORIZATION, format!("Bearer {}", token))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({
                        "old_password": "adminadmin",
                        "new_password": "mynewpassword123"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(change_pwd_res.status(), StatusCode::OK);

    // 6. 用新密码登录
    let login_new = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({
                        "username": "admin",
                        "password": "mynewpassword123"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let body_bytes = axum::body::to_bytes(login_new.into_body(), usize::MAX).await.unwrap();
    let new_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(new_json["success"], true);

    println!("管理员认证核心流程测试全部通过！");
}

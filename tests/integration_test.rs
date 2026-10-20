use alist2strm::alist::AlistClient;
use alist2strm::config::{StrmMode, StrmTaskConfig};
use alist2strm::strm::StrmEngine;
use axum::{
    routing::{get, post},
    Json, Router,
};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tempfile::tempdir;

#[tokio::test]
async fn test_full_strm_generation_pipeline() {
    let list_req_count = Arc::new(AtomicUsize::new(0));
    let c = list_req_count.clone();

    // 1. 模拟 AList 服务端
    let mock_alist_app = Router::new()
        .route(
            "/api/me",
            get(|| async {
                Json(serde_json::json!({
                    "code": 200,
                    "message": "success",
                    "data": {
                        "username": "admin",
                        "base_path": "/",
                        "role": 2
                    }
                }))
            }),
        )
        .route(
            "/api/fs/list",
            post(move |Json(payload): Json<serde_json::Value>| {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::Relaxed);
                    let path = payload.get("path").and_then(|v| v.as_str()).unwrap_or("");

                    if path == "/百度网盘/影视" {
                        Json(serde_json::json!({
                            "code": 200,
                            "message": "success",
                            "data": {
                                "content": [
                                    {
                                        "name": "星际穿越.2014.mp4",
                                        "size": 1500000000,
                                        "is_dir": false,
                                        "sign": "sign_interstellar"
                                    },
                                    {
                                        "name": ".DS_Store",
                                        "size": 6000,
                                        "is_dir": false,
                                        "sign": ""
                                    },
                                    {
                                        "name": "Avatar_BDMV",
                                        "size": 0,
                                        "is_dir": true,
                                        "sign": ""
                                    }
                                ],
                                "total": 3
                            }
                        }))
                    } else if path == "/百度网盘/影视/Avatar_BDMV" {
                        Json(serde_json::json!({
                            "code": 200,
                            "message": "success",
                            "data": {
                                "content": [
                                    {
                                        "name": "BDMV",
                                        "size": 0,
                                        "is_dir": true,
                                        "sign": ""
                                    }
                                ],
                                "total": 1
                            }
                        }))
                    } else if path == "/百度网盘/影视/Avatar_BDMV/BDMV" {
                        Json(serde_json::json!({
                            "code": 200,
                            "message": "success",
                            "data": {
                                "content": [
                                    {
                                        "name": "STREAM",
                                        "size": 0,
                                        "is_dir": true,
                                        "sign": ""
                                    }
                                ],
                                "total": 1
                            }
                        }))
                    } else if path == "/百度网盘/影视/Avatar_BDMV/BDMV/STREAM" {
                        Json(serde_json::json!({
                            "code": 200,
                            "message": "success",
                            "data": {
                                "content": [
                                    {
                                        "name": "00001.m2ts",
                                        "size": 1000000, // 片头小文件
                                        "is_dir": false,
                                        "sign": "sign_intro"
                                    },
                                    {
                                        "name": "00002.m2ts",
                                        "size": 45000000000u64, // 真正的正片大文件
                                        "is_dir": false,
                                        "sign": "sign_main"
                                    }
                                ],
                                "total": 2
                            }
                        }))
                    } else {
                        Json(serde_json::json!({
                            "code": 200,
                            "message": "success",
                            "data": { "content": [], "total": 0 }
                        }))
                    }
                }
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, mock_alist_app).await.unwrap();
    });

    let mock_server_url = format!("http://127.0.0.1:{}", port);

    // 2. 准备临时目标目录
    let tmp = tempdir().unwrap();
    let target_dir = tmp.path().to_str().unwrap().to_string();

    let client = AlistClient::new(
        mock_server_url.clone(),
        Some("https://alist.publichost.com:5244".to_string()),
        "test-token".to_string(),
        0.0,
    )
    .unwrap();

    let me = client.test_connection().await.unwrap();
    assert_eq!(me.username.as_deref(), Some("admin"));
    assert_eq!(me.base_path, "/");

    let task = StrmTaskConfig {
        id: "test-task-1".to_string(),
        name: "测试网盘生成".to_string(),
        cron: None,
        source_dir: "/百度网盘/影视".to_string(),
        target_dir: target_dir.clone(),
        mode: StrmMode::AlistURL,
        flatten: false,
        overwrite: false,
        concurrency: 5,
        sync_delete: true,
        video_extensions: vec![".mp4".to_string(), ".m2ts".to_string()],
    };

    let engine = StrmEngine::new(task.clone(), client, me.base_path);

    // 3. 执行生成
    let stats = engine.execute(|msg| println!("[LOG] {}", msg)).await.unwrap();

    assert_eq!(stats.scanned_dirs, 4);
    assert_eq!(stats.created_strm, 2); // 1个星际穿越 + 1个BDMV折叠后的正片
    assert_eq!(stats.skipped_strm, 0);

    // 4. 检查生成的普通电影 .strm 文件
    let strm_file_1 = tmp.path().join("星际穿越.2014.strm");
    assert!(strm_file_1.exists(), "星际穿越.2014.strm 应当被成功创建");
    let content_1 = std::fs::read_to_string(&strm_file_1).unwrap();
    println!("普通电影 STRM 内容: {}", content_1);
    // 验证是否使用了 public_url，并包含 URL 编码和 sign
    assert!(content_1.starts_with("https://alist.publichost.com:5244/d/"));
    assert!(content_1.contains("?sign=sign_interstellar"));

    // 5. 检查 BDMV 蓝光原盘生成结果
    let bdmv_strm = tmp.path().join("Avatar_BDMV.strm");
    assert!(bdmv_strm.exists(), "Avatar_BDMV.strm 应当以原盘根目录为名生成单个正片");
    let bdmv_content = std::fs::read_to_string(&bdmv_strm).unwrap();
    println!("BDMV 正片 STRM 内容: {}", bdmv_content);
    // 验证选中的是最大的 00002.m2ts
    assert!(bdmv_content.contains("00002.m2ts"));
    assert!(bdmv_content.contains("?sign=sign_main"));

    // 6. 测试增量跳过：再次执行，不覆盖已有文件
    let stats_second = engine.execute(|msg| println!("[LOG 2] {}", msg)).await.unwrap();
    assert_eq!(stats_second.created_strm, 0, "二次运行不应重新写入");
    assert_eq!(stats_second.skipped_strm, 2, "二次运行应当全部跳过");

    println!("所有自动化集成测试验证通过！");
}

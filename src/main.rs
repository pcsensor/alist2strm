mod alist;
mod api;
mod auth;
mod config;
mod strm;
mod task;
mod tasklog;
mod web;

use clap::Parser;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::info;

#[derive(Parser, Debug)]
#[command(author, version, about = "AList 302 STRM Generator with WebUI")]
struct Args {
    /// 配置文件路径
    #[arg(short, long, default_value = "config.yaml")]
    config: PathBuf,

    /// 监听端口 (默认使用配置文件端口或 8383)
    #[arg(short, long)]
    port: Option<u16>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "alist2strm=info,tower_http=info".into()),
        )
        .init();

    let args = Args::parse();
    info!("正在加载配置文件: {:?}", args.config);

    let config_store = config::ConfigStore::new(args.config);
    let app_cfg = config_store.get().await;

    let port = args.port.unwrap_or(app_cfg.port);

    let task_manager = Arc::new(task::TaskManager::new(config_store.clone()));
    let session_manager = Arc::new(auth::SessionManager::new());

    // 启动 Cron 定时调度器
    if let Err(e) = task_manager.reload_scheduler().await {
        tracing::warn!("启动定时任务调度器失败: {}", e);
    }

    // 定期清理过期任务日志
    task_manager.start_log_maintenance();

    let state = api::AppState {
        config_store,
        task_manager,
        session_manager,
    };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = api::create_api_routes(state)
        .fallback(web::static_handler)
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("🚀 AList2STRM 服务启动成功！");
    info!("👉 访问地址: http://127.0.0.1:{}", port);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

mod api;
mod config;
mod logger;
mod process;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tracing;

use crate::process::registry;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    /// 配置文件路径
    #[arg(short, long, value_name = "c", default_value = "procd.yaml")]
    pub config: String,
}

#[tokio::main]
async fn main() {
    logger::setup();

    let log_buf = logger::LogBuffer::new(100);

    tracing::info!("starting ...");

    let args = Args::parse();
    let cfg_path = args.config.as_str();
    tracing::info!("using config {}", cfg_path);

    let cfg = config::Config::from_file(cfg_path).unwrap();

    // 设置当前进程的工作目录
    if let Err(e) = cfg.set_current_dir(cfg_path) {
        tracing::warn!("set_current_dir failed: {:?}", e);
        std::process::exit(1);
    }

    let reg = Arc::new(registry::Registry::new());
    // Spawn process
    for process_cfg in cfg.process.clone() {
        process_cfg.start_spawn(reg.clone());
    }

    let _guard = logger::init_tracing(&cfg.log_dir, log_buf.clone());

    let cfg_arc = Arc::new(cfg.clone());

    let state = api::auth::AuthState::new();
    // 启动后台清理任务
    state.clone().cleanup_task();

    // 启动后台，定时检查文件变化任务
    reg.clone().watch(cfg.restart_delay.unwrap_or(Duration::from_secs(10)));

    // Set up web API
    let app = api::handlers::build_router()
        .layer(axum::Extension(reg.clone()))
        .layer(axum::Extension(cfg_arc))
        .layer(axum::Extension(state))
        .layer(axum::Extension(log_buf));

    tracing::info!("Listening on {}", cfg.http.addr);

    let addr = &cfg.http.addr;
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .unwrap();
}

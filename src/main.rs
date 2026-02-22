mod api;
mod config;
mod logger;
mod process;

use std::net::SocketAddr;
use std::{env, sync::Arc};
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
    let log_buf = logger::new_logbuf();

    // init_tracing();

    tracing::info!("starting ...");

    let args = Args::parse();
    let cfg_path = args.config.as_str();
    tracing::info!("using config {}", cfg_path);

    let cfg = config::Config::from_file(cfg_path).unwrap();

    let home = cfg.home.clone();
    if !home.is_empty() {
        if let Err(e) = env::set_current_dir(home.clone()) {
            tracing::warn!("set_current_dir({:?}) failed: {:?}", home, e);
            std::process::exit(1);
        }
    }

    match env::current_dir() {
        Ok(dir) => tracing::info!("current_dir: {}", dir.display()),
        Err(e) => tracing::warn!("get current_dir failed: {}", e),
    }

    let reg = Arc::new(registry::Registry::new());
    // Spawn processes
    for process_cfg in cfg.processes.clone() {
        process_cfg.start_spawn(reg.clone());
    }

    let cfg_arc = Arc::new(cfg.clone());

    let state = api::auth::AuthState::new();
    // 启动后台清理任务
    state.clone().cleanup_task();

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

mod api;
mod config;
mod process;

use std::{env, sync::Arc};
use tracing;
use tracing_subscriber::EnvFilter;

use crate::process::registry;
use clap::Parser;

fn init_tracing() {
    let log_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("trace,tower_http=trace"));

    tracing_subscriber::fmt().with_env_filter(log_filter).init();
    tracing::info!("starting ...");
}

#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    /// 配置文件路径
    #[arg(short, long, value_name = "c", default_value = "procd.yaml")]
    pub config: String,
}

#[tokio::main]
async fn main() {
    init_tracing();

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
        let reg = reg.clone();
        tokio::spawn(process::supervisor::supervise(process_cfg, reg));
    }

    let cfg_arc = Arc::new(cfg.clone());

    // Set up web API
    let app = api::handlers::build_router()
        .layer(axum::Extension(reg.clone()))
        .layer(axum::Extension(cfg_arc));

    tracing::info!("Listening on {}", cfg.http.addr);

    let addr = &cfg.http.addr;
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

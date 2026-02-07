mod api;
mod config;
mod process;

use std::sync::Arc;
use tracing;
use tracing_subscriber::EnvFilter;

use crate::process::registry;
use clap::Parser;

fn init_tracing() {
    let log_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("trace,tower_http=trace"));

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

    let mut cfg = config::Config::from_file(cfg_path).unwrap();

    cfg.check_and_init();

    let reg = Arc::new(registry::Registry::new());
    // Spawn processes
    for process_cfg in cfg.processes.clone() {
        let reg = reg.clone();
        tokio::spawn(process::supervisor::supervise(process_cfg, reg));
    }

    // Set up web API
    let app = api::handlers::build_router(reg.clone());

    tracing::info!("Listening on {}", cfg.http.addr);

    let addr = &cfg.http.addr;
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

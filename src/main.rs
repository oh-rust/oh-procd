mod api;
mod config;
mod process;

use axum::{
    Router,
    extract::Extension,
    routing::{get, post},
};

use std::sync::Arc;
use tower_http::trace::TraceLayer;
use tracing;
use tracing_subscriber::EnvFilter;

use crate::process::registry;

#[tokio::main]
async fn main() {
    let log_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("trace,tower_http=trace"));

    tracing_subscriber::fmt().with_env_filter(log_filter).init();
    tracing::info!("starting ...");

    let mut cfg = config::Config {
        http: config::HttpConfig {
            addr: "127.0.0.1:8080".to_string(),
        },
        home: "/var/".to_string(),
        log_dir: "/var/log/procd".to_string(),
        processes: vec![config::ProcessConfig {
            name: "web-api".to_string(),
            cmd: "/usr/bin/python3".to_string(),
            args: vec![
                "-m".to_string(),
                "http.server".to_string(),
                "8090".to_string(),
            ],
            envs: vec![],
            output_dir: "".to_string(),
            home: "".to_string(),
            redirect_output: true,
            max_run: None,
        }],
    };

    for pc in cfg.processes.iter_mut() {
        if pc.output_dir.is_empty() {
            let mut path = std::path::PathBuf::from(&cfg.log_dir);
            path.push(&pc.name);
            pc.output_dir = path.to_string_lossy().to_string()
        }

        if pc.home.is_empty() {
            pc.home = cfg.home.clone();
        }
    }

    let reg = Arc::new(registry::Registry::new());

    // Spawn processes
    for process_cfg in cfg.processes.clone() {
        let reg = reg.clone();
        tokio::spawn(process::supervisor::supervise(process_cfg, reg));
    }

    // Set up web API
    let app: Router = Router::new()
        .route("/api/processes", get(api::handlers::list_processes))
        .route(
            "/api/process/{name}/restart",
            post(api::handlers::restart_process),
        )
        .layer(Extension(reg))
        .layer(TraceLayer::new_for_http());

    tracing::info!("Listening on {}", cfg.http.addr);

    let addr = &cfg.http.addr;
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

use axum::{
    Json, Router,
    extract::{self, Extension},
    middleware, response,
    routing::{get, post},
};

use serde::Serialize;
use std::sync::Arc;
use tower_http::trace::TraceLayer;

use crate::api::auth::basic_auth;
use crate::process::registry::{ControlMsg, ProcState, ProcessOut, Registry};

const INDEX_HTML: &str = include_str!("asset/index.html");

async fn index() -> response::Html<&'static str> {
    response::Html(INDEX_HTML)
}

#[derive(Serialize)]
struct ListResponse<T> {
    code: i32,
    message: String,
    data: T,
    server: ServerInfo,
}

#[derive(Serialize)]
struct ServerInfo {
    start: String,  // 进程启动时间
    memory: String, // 进程使用的内存
    cpu_usage: f32, // 进程的cpu使用情况
    pid: u32,

    sys_total_memory: String, // 系统 总内存，
    sys_used_memory: String,  // 系统，使用的内存
    sys_total_swap: String,   //  系统，
    sys_used_swap: String,
}
async fn list_processes(Extension(reg): Extension<Arc<Registry>>) -> Json<ListResponse<Vec<ProcessOut>>> {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    let mut server = ServerInfo {
        start: reg.start_time(),
        memory: "".to_string(),

        sys_total_memory: format!("{:.1} MB", (sys.total_memory() as f64) / 1024.0 / 1024.0),
        sys_used_memory: format!("{:.1} MB", (sys.used_memory() as f64) / 1024.0 / 1024.0),
        sys_total_swap: format!("{:.1} MB", (sys.total_swap() as f64) / 1024.0 / 1024.0),
        sys_used_swap: format!("{:.1} MB", (sys.used_swap() as f64) / 1024.0 / 1024.0),

        cpu_usage: 0.0,
        pid: 0,
    };

    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    if let Some(proc) = sys.process(sysinfo::get_current_pid().unwrap()) {
        server.memory = format!("{:.1} MB", (proc.memory() as f64) / 1024.0 / 1024.0);
        server.cpu_usage = proc.cpu_usage();
        server.pid = proc.pid().as_u32();
    }
    let mut items = reg.list();
    for x in items.iter_mut() {
        if x.pid == 0 {
            continue;
        }
        if let Some(proc) = sys.process(sysinfo::Pid::from_u32(x.pid)) {
            x.memory_used = format!("{:.1} MB", (proc.memory() as f64) / 1024.0 / 1024.0);
        }
    }

    let val: ListResponse<Vec<ProcessOut>> = ListResponse {
        code: 0,
        message: "success".to_string(),
        data: items,
        server: server,
    };

    Json(val)
}

async fn restart_process(
    Extension(reg): Extension<Arc<Registry>>,
    extract::Path(name): extract::Path<String>,
) -> impl response::IntoResponse {
    tracing::info!("Restarting process: {}", name);
    reg.set_state(&name, ProcState::Stopping);
    let reg = reg.as_ref();

    match reg.get_control(&name) {
        Some(tx) => {
            if let Err(e) = tx.send(ControlMsg::Restart).await {
                tracing::error!("failed to send restart to {}: {}", name, e);
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to restart process",
                )
            } else {
                (axum::http::StatusCode::OK, "restart signal sent")
            }
        }
        None => (axum::http::StatusCode::NOT_FOUND, "process not found"),
    }
}
async fn kill_process(
    Extension(reg): Extension<Arc<Registry>>,
    extract::Path(name): extract::Path<String>,
) -> impl response::IntoResponse {
    tracing::info!("Killing process: {}", name);
    reg.set_state(&name, ProcState::Stopping);
    let reg = reg.as_ref();

    match reg.get_control(&name) {
        Some(tx) => {
            if let Err(e) = tx.send(ControlMsg::Kill).await {
                tracing::error!("failed to send restart to {}: {}", name, e);
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to restart process",
                )
            } else {
                (axum::http::StatusCode::OK, "restart signal sent")
            }
        }
        None => (axum::http::StatusCode::NOT_FOUND, "process not found"),
    }
}
async fn start_process(
    Extension(reg): Extension<Arc<Registry>>,
    extract::Path(name): extract::Path<String>,
) -> impl response::IntoResponse {
    tracing::info!("Starting process: {}", name);
    reg.set_state(&name, ProcState::Ready);

    match reg.as_ref().find(&name) {
        Some(pe) => {
            pe.cmd.clone().start_spawn(reg);
            (axum::http::StatusCode::OK, "start signal sent")
        }
        None => (axum::http::StatusCode::NOT_FOUND, "process not found"),
    }
}

async fn logs(Extension(lb): Extension<crate::logger::LogBuffer>) -> Json<Vec<String>> {
    let mut lines = lb.get_logs();
    lines.reverse();
    Json(lines)
}

pub fn build_router() -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/logs", get(logs))
        .route("/api/processes", get(list_processes))
        .route("/api/process/{name}/restart", post(restart_process))
        .route("/api/process/{name}/kill", post(kill_process))
        .route("/api/process/{name}/start", post(start_process))
        .layer(middleware::from_fn(basic_auth))
        .layer(TraceLayer::new_for_http())
}

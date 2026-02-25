use axum::{
    Json, Router,
    extract::{self, ConnectInfo, Extension, Request},
    http::header,
    middleware, response,
    routing::{get, post},
};

use rand::RngExt;
use serde::Serialize;
use std::net::SocketAddr;
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

use std::collections::HashMap;
use sysinfo::Pid;

/// 获取父进程 pid 的所有子进程 PID 列表和总内存（KB）
/// 返回 (Vec<Pid>, total_memory)
fn get_child_pids_and_total_memory(processes: &HashMap<Pid, sysinfo::Process>, parent_pid: Pid) -> (Vec<Pid>, u64) {
    let mut pids = Vec::new();
    let mut total_memory = 0;

    // 找出直接子进程
    for proc in processes.values().filter(|p| p.parent() == Some(parent_pid)) {
        pids.push(proc.pid());
        total_memory += proc.memory();

        // 递归获取孙子进程
        let (child_pids, child_memory) = get_child_pids_and_total_memory(processes, proc.pid());
        pids.extend(child_pids);
        total_memory += child_memory;
    }

    (pids, total_memory)
}

async fn list_processes(Extension(reg): Extension<Arc<Registry>>, req: Request) -> Json<ListResponse<Vec<ProcessOut>>> {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();

    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    let processes = sys.processes();

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

    if let Some(proc) = sys.process(sysinfo::get_current_pid().unwrap()) {
        server.memory = format!("{:.1} MB", (proc.memory() as f64) / 1024.0 / 1024.0);
        server.cpu_usage = proc.cpu_usage();
        server.pid = proc.pid().as_u32();
    }

    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let hostname = host.split(':').next().unwrap_or("");

    let mut items = reg.list();
    for x in items.iter_mut() {
        if x.pid == 0 {
            continue;
        }
        let parent_pid = sysinfo::Pid::from_u32(x.pid);
        let (child_pids, total_memory) = get_child_pids_and_total_memory(processes, parent_pid);

        if total_memory > 0 {
            x.memory_used = format!("{:.1} MB", (total_memory as f64) / 1024.0 / 1024.0);
        }

        if child_pids.len() > 0 {
            x.child_pids = child_pids.iter().map(|p| p.as_u32()).collect();
        }

        // if let Some(proc) = sys.process(sysinfo::Pid::from_u32(x.pid)) {
        //     x.memory_used = format!("{:.1} MB", (proc.memory() as f64) / 1024.0 / 1024.0);
        // }

        if x.web_address.contains("{") {
            x.web_address = x.web_address.replace("{HOST}", hostname);
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
        .layer(TraceLayer::new_for_http().make_span_with(|req: &Request<_>| {
            let client_addr = req
                .extensions()
                .get::<ConnectInfo<SocketAddr>>()
                .map(|ci| ci.0.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let log_id: u64 = rand::rng().random_range(1..9999999);
            tracing::info_span!("HTTP", log_id = log_id, client = client_addr)
        }))
}

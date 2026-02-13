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
    #[serde(default)]
    server_start: String,
}
async fn list_processes(Extension(reg): Extension<Arc<Registry>>) -> Json<ListResponse<Vec<ProcessOut>>> {
    let val = ListResponse {
        code: 0,
        message: "success".to_string(),
        data: reg.list(),
        server_start: reg.start_time(),
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

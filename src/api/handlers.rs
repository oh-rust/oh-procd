use axum::{
    Json, Router,
    extract::{self, Extension},
    middleware, response,
    routing::{get, post},
};

use std::sync::Arc;
use tower_http::trace::TraceLayer;

use crate::api::auth::basic_auth;
use crate::process::registry::{ControlMsg, ProcState, ProcessOut, Registry};

const INDEX_HTML: &str = include_str!("asset/index.html");

async fn index() -> response::Html<&'static str> {
    response::Html(INDEX_HTML)
}

async fn list_processes(Extension(reg): Extension<Arc<Registry>>) -> Json<Vec<ProcessOut>> {
    Json(reg.list())
}

async fn restart_process(
    Extension(reg): Extension<Arc<Registry>>,
    extract::Path(name): extract::Path<String>,
) -> impl response::IntoResponse {
    // Logic to stop and restart the process
    tracing::info!("Restarting process: {}", name);
    // Placeholder: Just simulate the stop and start
    reg.set_state(&name, ProcState::Stopped);
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

pub fn build_router() -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/processes", get(list_processes))
        .route("/api/process/{name}/restart", post(restart_process))
        .layer(middleware::from_fn(basic_auth))
        .layer(TraceLayer::new_for_http())
}

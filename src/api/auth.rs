use crate::config::Config;
use axum::{
    extract::{ConnectInfo, Extension},
    http::{StatusCode, header},
    middleware::Next,
    response::IntoResponse,
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use dashmap::DashMap;
use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::time::interval;

#[derive(Clone)]
pub struct AuthState {
    pub failed: Arc<DashMap<String, Vec<Instant>>>, // 近期每个 IP 的失败时间戳列表
}

const FAILURE_WINDOW: Duration = Duration::from_secs(2 * 60); // 失败统计周期
const MAX_FAILURES: usize = 10; //单个 IP 最多失败数

impl AuthState {
    pub fn new() -> Self {
        Self {
            failed: Arc::new(DashMap::new()),
        }
    }
    // 启动后台清理任务
    pub fn cleanup_task(self) {
        let state = self.clone();
        let mut timer = interval(Duration::from_secs(60)); // 每分钟清理一次
        tokio::spawn(async move {
            loop {
                timer.tick().await;
                let now = Instant::now();
                state.failed.retain(|_ip, times| {
                    times.retain(|t| now.duration_since(*t) <= FAILURE_WINDOW);
                    !times.is_empty() // 如果列表为空，删除该 IP
                });
            }
        });
    }
}

pub async fn basic_auth(
    Extension(cfg): Extension<Arc<Config>>,
    Extension(state): Extension<AuthState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: axum::http::Request<axum::body::Body>,
    next: Next,
) -> impl IntoResponse {
    let cfg = cfg.as_ref();
    if cfg.auth.username.is_empty() {
        // 没有配置，则不检查
        return next.run(req).await;
    }

    let ip = addr.ip().to_string(); // 当前请求 IP

    // 检查该 IP 是否被封禁
    {
        let now = Instant::now();
        if let Some(mut entry) = state.failed.get_mut(&ip) {
            // 清理超过 5 分钟的旧记录
            entry.retain(|t| now.duration_since(*t) <= FAILURE_WINDOW);
            if entry.len() >= MAX_FAILURES {
                tracing::warn!("forbidden");
                return StatusCode::FORBIDDEN.into_response();
            }
        }
    }

    let authorized = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Basic "))
        .and_then(|encoded| STANDARD.decode(encoded).ok())
        .and_then(|decoded| String::from_utf8(decoded).ok())
        .and_then(|s| {
            s.split_once(':')
                .map(|(user, pass)| (user.to_string(), pass.to_string()))
        })
        .map(|(user, pass)| {
            if cfg.auth.check(&user, &pass) {
                return true;
            }
            tracing::warn!(user = user, pass = pass, "login failed");
            return false;
        })
        .unwrap_or(false);

    if authorized {
        next.run(req).await
    } else {
        tracing::warn!("auth failed");
        // 记录失败
        {
            let now = Instant::now();
            state
                .failed
                .entry(ip.clone())
                .and_modify(|v| {
                    v.retain(|t| now.duration_since(*t) <= FAILURE_WINDOW);
                    v.push(now);
                })
                .or_insert_with(|| vec![now]);
        }

        let mut resp = StatusCode::UNAUTHORIZED.into_response();
        resp.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            header::HeaderValue::from_static(r#"Basic realm="procd""#),
        );
        resp
    }
}

use crate::config::Config;
use axum::{
    extract::Extension,
    http::{StatusCode, header},
    middleware::Next,
    response::IntoResponse,
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use std::sync::Arc;

pub async fn basic_auth(
    Extension(cfg): Extension<Arc<Config>>,
    req: axum::http::Request<axum::body::Body>,
    next: Next,
) -> impl IntoResponse {
    let cfg = cfg.as_ref();
    if cfg.auth.username.is_empty() {
        // 没有配置，则不检查
        return next.run(req).await;
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
        .map(|(user, pass)| cfg.auth.check(&user, &pass))
        .unwrap_or(false);

    if authorized {
        next.run(req).await
    } else {
        let mut resp = StatusCode::UNAUTHORIZED.into_response();
        resp.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            header::HeaderValue::from_static(r#"Basic realm="procd""#),
        );
        resp
    }
}

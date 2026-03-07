//! Per-endpoint rate limiting middleware.
//!
//! Three middleware functions — one per tier (LLM, search, task) — each backed
//! by a [`governor`] GCRA rate limiter keyed by client IP address.
//!
//! Rate limiting is applied **inside** the auth layer so that only authenticated
//! requests consume quota. When a request exceeds the tier's limit, a `429 Too
//! Many Requests` response is returned with a `Retry-After` header.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use aivyx_audit::{AuditEvent, AuditLog};

use crate::app_state::{AppState, KeyedRateLimiter};

/// Create a 429 JSON response with `Retry-After` header.
fn rate_limit_response(retry_after_secs: u64) -> Response {
    let body = serde_json::json!({
        "error": "rate limit exceeded",
        "code": 429,
        "retry_after": retry_after_secs,
    });
    let mut resp = (StatusCode::TOO_MANY_REQUESTS, axum::Json(body)).into_response();
    resp.headers_mut()
        .insert("Retry-After", retry_after_secs.to_string().parse().unwrap());
    resp
}

/// Log a rate limit event to the audit log.
fn log_rate_limit(audit_log: &AuditLog, remote_addr: &str, tier: &str, path: &str) {
    metrics::counter!("rate_limit_rejections_total", "tier" => tier.to_string()).increment(1);

    let _ = audit_log.append(AuditEvent::RateLimitExceeded {
        remote_addr: remote_addr.to_string(),
        tier: tier.to_string(),
        path: path.to_string(),
    });
}

/// Check the rate limiter and return an error response if exceeded.
fn check_rate_limit(
    limiter: &KeyedRateLimiter,
    addr: &SocketAddr,
    audit_log: &AuditLog,
    tier: &str,
    path: &str,
    window_secs: u64,
) -> Option<Response> {
    let ip = addr.ip();
    match limiter.check_key(&ip) {
        Ok(_) => None,
        Err(_not_until) => {
            log_rate_limit(audit_log, &addr.to_string(), tier, path);
            Some(rate_limit_response(window_secs))
        }
    }
}

/// Rate limiting middleware for LLM-tier endpoints.
///
/// Applies to: `/chat`, `/chat/stream`, `/chat/audio`, `/teams/*/run`,
/// `/teams/*/run/stream`, `/digest`.
pub async fn rate_limit_llm(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if let Some(ref limiters) = state.endpoint_rate_limiters {
        let window = state
            .config
            .read()
            .await
            .server
            .as_ref()
            .and_then(|s| s.rate_limit.as_ref())
            .map(|r| r.llm.window_secs)
            .unwrap_or(60);
        let path = req.uri().path().to_string();
        if let Some(resp) =
            check_rate_limit(&limiters.llm, &addr, &state.audit_log, "llm", &path, window)
        {
            return resp;
        }
    }
    next.run(req).await
}

/// Rate limiting middleware for search-tier endpoints.
///
/// Applies to: `/memory/search`, `/memory/profile/extract`.
pub async fn rate_limit_search(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if let Some(ref limiters) = state.endpoint_rate_limiters {
        let window = state
            .config
            .read()
            .await
            .server
            .as_ref()
            .and_then(|s| s.rate_limit.as_ref())
            .map(|r| r.search.window_secs)
            .unwrap_or(60);
        let path = req.uri().path().to_string();
        if let Some(resp) = check_rate_limit(
            &limiters.search,
            &addr,
            &state.audit_log,
            "search",
            &path,
            window,
        ) {
            return resp;
        }
    }
    next.run(req).await
}

/// Rate limiting middleware for task-tier endpoints.
///
/// Applies to: `POST /tasks`, `/tasks/{id}/resume`.
pub async fn rate_limit_task(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if let Some(ref limiters) = state.endpoint_rate_limiters {
        let window = state
            .config
            .read()
            .await
            .server
            .as_ref()
            .and_then(|s| s.rate_limit.as_ref())
            .map(|r| r.task.window_secs)
            .unwrap_or(60);
        let path = req.uri().path().to_string();
        if let Some(resp) = check_rate_limit(
            &limiters.task,
            &addr,
            &state.audit_log,
            "task",
            &path,
            window,
        ) {
            return resp;
        }
    }
    next.run(req).await
}

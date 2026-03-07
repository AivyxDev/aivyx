//! Bearer token authentication middleware.
//!
//! Extracts `Authorization: Bearer <token>` from the request, hashes it with
//! SHA-256, and compares against `AppState::bearer_token_hash`. Failed attempts
//! are audit-logged as `HttpAuthFailed` and rate-limited per IP.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use aivyx_audit::AuditEvent;

use crate::app_state::AppState;

/// Maximum failed auth attempts per IP within the rate limit window.
const MAX_AUTH_FAILURES: usize = 10;

/// Rate limit window duration (60 seconds).
const RATE_LIMIT_WINDOW_SECS: u64 = 60;

/// Auth middleware for protected routes.
///
/// Returns 401 if the `Authorization` header is missing, malformed, or contains
/// an invalid token. Returns 429 if the client IP has exceeded the rate limit
/// for failed attempts. Logs `HttpAuthFailed` on rejection.
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let remote_addr = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| ci.0.to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // Check rate limit before processing auth
    if let Some(ip) = parse_ip(&remote_addr)
        && is_rate_limited(&state, ip)
    {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            axum::Json(serde_json::json!({"error": "too many failed auth attempts", "code": 429})),
        )
            .into_response();
    }

    let auth_header = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    let token = match auth_header {
        Some(h) if h.starts_with("Bearer ") => &h[7..],
        _ => {
            let reason = if auth_header.is_some() {
                "malformed authorization header"
            } else {
                "missing authorization header"
            };
            record_auth_failure(&state, &remote_addr, reason);
            return (
                StatusCode::UNAUTHORIZED,
                axum::Json(serde_json::json!({"error": reason, "code": 401})),
            )
                .into_response();
        }
    };

    // SHA-256 hash and constant-time compare
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let provided_hash: [u8; 32] = hasher.finalize().into();

    let current_hash = *state.bearer_token_hash.read().await;
    if provided_hash.ct_eq(&current_hash).unwrap_u8() == 0 {
        record_auth_failure(&state, &remote_addr, "invalid bearer token");
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({"error": "invalid bearer token", "code": 401})),
        )
            .into_response();
    }

    next.run(request).await
}

/// Parse an IP address from a "ip:port" string.
fn parse_ip(addr: &str) -> Option<std::net::IpAddr> {
    addr.parse::<std::net::SocketAddr>()
        .map(|sa| sa.ip())
        .ok()
        .or_else(|| addr.parse::<std::net::IpAddr>().ok())
}

/// Check if an IP is rate-limited due to too many failed auth attempts.
fn is_rate_limited(state: &AppState, ip: std::net::IpAddr) -> bool {
    let now = std::time::Instant::now();
    let window = std::time::Duration::from_secs(RATE_LIMIT_WINDOW_SECS);

    let mut limiter = state
        .auth_rate_limiter
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(timestamps) = limiter.get_mut(&ip) {
        timestamps.retain(|t| now.duration_since(*t) < window);
        timestamps.len() >= MAX_AUTH_FAILURES
    } else {
        false
    }
}

/// Record a failed authentication attempt for rate limiting and audit.
fn record_auth_failure(state: &AppState, remote_addr: &str, reason: &str) {
    // Audit log
    if let Err(e) = state.audit_log.append(AuditEvent::HttpAuthFailed {
        remote_addr: remote_addr.to_string(),
        reason: reason.to_string(),
    }) {
        tracing::error!("failed to audit auth failure: {e}");
    }

    // Rate limit tracking
    if let Some(ip) = parse_ip(remote_addr) {
        let mut limiter = state
            .auth_rate_limiter
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        limiter
            .entry(ip)
            .or_default()
            .push(std::time::Instant::now());
    }
}

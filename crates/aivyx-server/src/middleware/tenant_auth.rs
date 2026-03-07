//! Multi-tenant authentication middleware.
//!
//! When multi-tenancy is enabled, this middleware replaces `auth_middleware`:
//!
//! 1. Extracts `Authorization: Bearer <token>` from the request.
//! 2. Tries `ApiKeyStore::lookup_by_token()` — if found, builds tenant-scoped
//!    `AuthContext` with the key's role and tenant association.
//! 3. Falls back to the legacy single-user bearer token (constant-time compare).
//! 4. Checks `TenantStatus::Active` — rejects Suspended/Deleted tenants.
//! 5. Parses `X-Aivyx-Tags` header for cost allocation tags.
//! 6. Inserts `AuthContext` into request extensions.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use aivyx_audit::AuditEvent;
use aivyx_tenant::{AuthContext, TenantStatus};

use crate::app_state::AppState;

/// Tenant-aware auth middleware for multi-tenant mode.
///
/// Tries API key authentication first (via `ApiKeyStore`), then falls back to
/// the legacy single-user bearer token. On successful API key auth, verifies
/// the tenant is active and builds a tenant-scoped `AuthContext`.
pub async fn tenant_auth_middleware(
    State(state): State<Arc<AppState>>,
    mut request: Request<axum::body::Body>,
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

    // Parse cost allocation tags from X-Aivyx-Tags header (key=value,key=value)
    let tags = parse_tags(&request);

    // --- Try API key lookup first ---
    if let Some(ref api_key_store) = state.api_key_store {
        match api_key_store.lookup_by_token(token, &state.master_key) {
            Ok(Some(key_record)) => {
                // Verify tenant is active
                if let Some(ref tenant_store) = state.tenant_store {
                    match tenant_store.get_tenant(&key_record.tenant_id, &state.master_key) {
                        Ok(Some(tenant)) => match &tenant.status {
                            TenantStatus::Active => {
                                let mut auth_ctx = AuthContext::tenant_user(
                                    key_record.tenant_id,
                                    tenant.name,
                                    key_record.key_id.clone(),
                                    key_record.role,
                                );
                                auth_ctx.tags = tags;
                                request.extensions_mut().insert(auth_ctx);
                                return next.run(request).await;
                            }
                            TenantStatus::Suspended { reason } => {
                                return (
                                    StatusCode::FORBIDDEN,
                                    axum::Json(serde_json::json!({
                                        "error": format!("tenant suspended: {reason}"),
                                        "code": 403,
                                    })),
                                )
                                    .into_response();
                            }
                            TenantStatus::Deleted => {
                                return (
                                    StatusCode::FORBIDDEN,
                                    axum::Json(serde_json::json!({
                                        "error": "tenant has been deleted",
                                        "code": 403,
                                    })),
                                )
                                    .into_response();
                            }
                        },
                        Ok(None) => {
                            return (
                                StatusCode::FORBIDDEN,
                                axum::Json(serde_json::json!({
                                    "error": "tenant not found for API key",
                                    "code": 403,
                                })),
                            )
                                .into_response();
                        }
                        Err(e) => {
                            tracing::error!("tenant lookup failed: {e}");
                            return (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                axum::Json(serde_json::json!({
                                    "error": "internal server error",
                                    "code": 500,
                                })),
                            )
                                .into_response();
                        }
                    }
                }
            }
            Ok(None) => {
                // API key not found — fall through to legacy bearer token
            }
            Err(e) => {
                tracing::error!("API key lookup failed: {e}");
                // Fall through to legacy bearer token check
            }
        }
    }

    // --- Fall back to legacy single-user bearer token ---
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

    // Legacy bearer token → single-user Admin
    let mut auth_ctx = AuthContext::single_user();
    auth_ctx.tags = tags;
    request.extensions_mut().insert(auth_ctx);

    next.run(request).await
}

/// Parse `X-Aivyx-Tags` header into a tag map.
///
/// Format: `key1=value1,key2=value2`
fn parse_tags(
    request: &Request<axum::body::Body>,
) -> std::collections::HashMap<String, String> {
    let mut tags = std::collections::HashMap::new();
    if let Some(header) = request
        .headers()
        .get("x-aivyx-tags")
        .and_then(|v| v.to_str().ok())
    {
        for pair in header.split(',') {
            let pair = pair.trim();
            if let Some((k, v)) = pair.split_once('=') {
                tags.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    tags
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
    let window = std::time::Duration::from_secs(60);

    let mut limiter = state
        .auth_rate_limiter
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(timestamps) = limiter.get_mut(&ip) {
        timestamps.retain(|t| now.duration_since(*t) < window);
        timestamps.len() >= 10
    } else {
        false
    }
}

/// Record a failed authentication attempt for rate limiting and audit.
fn record_auth_failure(state: &AppState, remote_addr: &str, reason: &str) {
    metrics::counter!("auth_failures_total", "reason" => reason.to_string()).increment(1);

    if let Err(e) = state.audit_log.append(AuditEvent::HttpAuthFailed {
        remote_addr: remote_addr.to_string(),
        reason: reason.to_string(),
    }) {
        tracing::error!("failed to audit auth failure: {e}");
    }

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

//! Authentication route handlers for SSO login/logout and user info.

use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Serialize;

use crate::extractors::AuthContextExt;

/// Response body for `GET /auth/me`.
#[derive(Serialize)]
struct MeResponse {
    principal: String,
    role: String,
    tenant_id: Option<String>,
    tenant_name: Option<String>,
}

/// `POST /auth/login` — placeholder for OIDC login flow.
///
/// Returns 501 Not Implemented. The actual OIDC authorization code flow
/// requires network access to the IdP and will be implemented when JWKS
/// validation is added.
pub async fn login() -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        axum::Json(serde_json::json!({
            "error": "OIDC login not yet implemented",
            "code": 501,
        })),
    )
}

/// `POST /auth/logout` — placeholder for session logout.
///
/// Returns 501 Not Implemented. Session destruction will be wired up
/// once the SSO session cache is integrated into the auth middleware.
pub async fn logout() -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        axum::Json(serde_json::json!({
            "error": "logout not yet implemented",
            "code": 501,
        })),
    )
}

/// `GET /auth/me` — returns the current authenticated user's info.
///
/// Extracts the `AuthContext` from the request (inserted by the auth
/// middleware) and returns the principal, role, and tenant information.
pub async fn me(auth: AuthContextExt) -> impl IntoResponse {
    let principal = format!("{:?}", auth.principal);
    let role = auth.role.to_string();
    let (tenant_id, tenant_name) = match &auth.tenant {
        Some(ctx) => (
            Some(ctx.tenant_id.to_string()),
            Some(ctx.tenant_name.clone()),
        ),
        None => (None, None),
    };

    (
        StatusCode::OK,
        axum::Json(MeResponse {
            principal,
            role,
            tenant_id,
            tenant_name,
        }),
    )
}

//! Axum extractor for [`AuthContext`].
//!
//! The auth middleware inserts an `AuthContext` into request extensions.
//! Handlers declare `auth: AuthContext` to extract it via this impl.

use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};

use aivyx_tenant::AuthContext;

/// Newtype wrapper so we can implement `FromRequestParts` in this crate.
///
/// Handlers use `AuthContextExt` as their parameter type, then access
/// the inner `AuthContext` via `Deref` or `.0`.
pub struct AuthContextExt(pub AuthContext);

impl std::ops::Deref for AuthContextExt {
    type Target = AuthContext;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Rejection type when `AuthContext` is missing from request extensions.
pub struct MissingAuthContext;

impl IntoResponse for MissingAuthContext {
    fn into_response(self) -> Response {
        (
            StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({"error": "missing auth context", "code": 401})),
        )
            .into_response()
    }
}

impl<S: Send + Sync> FromRequestParts<S> for AuthContextExt {
    type Rejection = MissingAuthContext;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<AuthContext>()
            .cloned()
            .map(AuthContextExt)
            .ok_or(MissingAuthContext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deref_to_auth_context() {
        let ctx = AuthContext::single_user();
        let ext = AuthContextExt(ctx.clone());
        assert_eq!(ext.role, ctx.role);
    }
}

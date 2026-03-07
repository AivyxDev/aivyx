//! HTTP error mapping for the aivyx server.
//!
//! Wraps `AivyxError` and implements Axum's `IntoResponse` trait to produce
//! consistent JSON error responses with appropriate HTTP status codes.

use aivyx_core::AivyxError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// JSON error response body.
#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
    code: u16,
}

/// Server error wrapper that maps `AivyxError` variants to HTTP status codes.
pub struct ServerError(pub AivyxError);

impl From<AivyxError> for ServerError {
    fn from(err: AivyxError) -> Self {
        ServerError(err)
    }
}

impl From<std::io::Error> for ServerError {
    fn from(err: std::io::Error) -> Self {
        ServerError(AivyxError::Io(err))
    }
}

impl IntoResponse for ServerError {
    fn into_response(self) -> Response {
        let (status, message) = map_error(&self.0);
        let body = ErrorBody {
            error: message,
            code: status.as_u16(),
        };
        (status, axum::Json(body)).into_response()
    }
}

/// Maps an `AivyxError` to an HTTP status code and error message.
fn map_error(err: &AivyxError) -> (StatusCode, String) {
    match err {
        AivyxError::RateLimit(msg) => (StatusCode::TOO_MANY_REQUESTS, msg.clone()),
        AivyxError::CapabilityDenied(msg) => (StatusCode::FORBIDDEN, msg.clone()),
        AivyxError::NotInitialized(msg) => (StatusCode::SERVICE_UNAVAILABLE, msg.clone()),
        AivyxError::LlmProvider(msg) => (StatusCode::BAD_GATEWAY, msg.clone()),
        AivyxError::Http(msg) => (StatusCode::BAD_GATEWAY, msg.clone()),
        AivyxError::Channel(msg) => (StatusCode::BAD_GATEWAY, msg.clone()),
        AivyxError::Serialization(e) => (StatusCode::BAD_REQUEST, e.to_string()),
        AivyxError::TomlDe(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
        AivyxError::Config(msg) if msg.contains("not found") => {
            (StatusCode::NOT_FOUND, msg.clone())
        }
        other => {
            tracing::error!("internal error: {other}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal server error".to_string(),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    async fn status_of(err: AivyxError) -> StatusCode {
        let resp = ServerError(err).into_response();
        resp.status()
    }

    async fn body_of(err: AivyxError) -> serde_json::Value {
        let resp = ServerError(err).into_response();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn rate_limit_maps_to_429() {
        assert_eq!(
            status_of(AivyxError::RateLimit("slow down".into())).await,
            StatusCode::TOO_MANY_REQUESTS
        );
    }

    #[tokio::test]
    async fn not_initialized_maps_to_503() {
        assert_eq!(
            status_of(AivyxError::NotInitialized("not ready".into())).await,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[tokio::test]
    async fn capability_denied_maps_to_403() {
        assert_eq!(
            status_of(AivyxError::CapabilityDenied("nope".into())).await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn error_body_has_code_field() {
        let body = body_of(AivyxError::RateLimit("test".into())).await;
        assert_eq!(body["code"], 429);
        assert_eq!(body["error"], "test");
    }

    #[tokio::test]
    async fn config_not_found_maps_to_404() {
        assert_eq!(
            status_of(AivyxError::Config("agent not found".into())).await,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn llm_provider_error_maps_to_502() {
        assert_eq!(
            status_of(AivyxError::LlmProvider("model unavailable".into())).await,
            StatusCode::BAD_GATEWAY
        );
    }

    #[tokio::test]
    async fn http_error_maps_to_502() {
        assert_eq!(
            status_of(AivyxError::Http("connection refused".into())).await,
            StatusCode::BAD_GATEWAY
        );
    }

    #[tokio::test]
    async fn channel_error_maps_to_502() {
        assert_eq!(
            status_of(AivyxError::Channel("telegram poll failed".into())).await,
            StatusCode::BAD_GATEWAY
        );
    }

    #[tokio::test]
    async fn serialization_error_maps_to_400() {
        let serde_err = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
        assert_eq!(
            status_of(AivyxError::Serialization(serde_err)).await,
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn generic_error_maps_to_500_with_masked_message() {
        let body = body_of(AivyxError::Storage("database corruption detected".into())).await;
        assert_eq!(body["code"], 500);
        // Internal error messages must not leak to clients
        assert_eq!(body["error"], "internal server error");
        assert!(
            !body["error"]
                .as_str()
                .unwrap()
                .contains("database corruption")
        );
    }

    #[tokio::test]
    async fn io_error_maps_to_500() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "no access");
        let server_err = ServerError::from(io_err);
        let resp = server_err.into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn toml_de_error_maps_to_400() {
        assert_eq!(
            status_of(AivyxError::TomlDe("expected string".into())).await,
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn config_without_not_found_maps_to_500() {
        // Config errors without "not found" should be internal errors
        assert_eq!(
            status_of(AivyxError::Config("invalid value".into())).await,
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[tokio::test]
    async fn server_error_from_aivyx_error() {
        let err = AivyxError::Agent("turn loop failed".into());
        let server_err = ServerError::from(err);
        let resp = server_err.into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn rate_limit_body_preserves_message() {
        let body = body_of(AivyxError::RateLimit("retry after 30s".into())).await;
        assert_eq!(body["code"], 429);
        assert_eq!(body["error"], "retry after 30s");
    }

    #[tokio::test]
    async fn capability_denied_body_preserves_message() {
        let body = body_of(AivyxError::CapabilityDenied(
            "filesystem access denied".into(),
        ))
        .await;
        assert_eq!(body["code"], 403);
        assert_eq!(body["error"], "filesystem access denied");
    }
}

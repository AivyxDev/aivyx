//! OpenAPI specification endpoint.

use axum::http::{StatusCode, header};
use axum::response::IntoResponse;

/// `GET /api/openapi.json` — serves the OpenAPI 3.1 specification.
///
/// The spec is compiled into the binary via `include_str!()` for zero
/// runtime file I/O. No authentication required — API documentation
/// should be publicly accessible.
pub async fn openapi_spec() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        include_str!("../openapi.json"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    #[tokio::test]
    async fn openapi_returns_valid_json() {
        let resp = openapi_spec().await.into_response();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let spec: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(spec["openapi"], "3.1.0");
        assert_eq!(spec["info"]["title"], "Aivyx Engine API");
    }

    #[tokio::test]
    async fn openapi_content_type_is_json() {
        let resp = openapi_spec().await.into_response();
        let ct = resp.headers().get(header::CONTENT_TYPE).unwrap();
        assert_eq!(ct, "application/json");
    }

    #[tokio::test]
    async fn openapi_contains_health_endpoint() {
        let resp = openapi_spec().await.into_response();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let spec: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(spec["paths"]["/health"].is_object());
    }
}

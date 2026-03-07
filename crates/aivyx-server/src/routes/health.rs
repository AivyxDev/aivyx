//! Health check endpoint.

use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Serialize;

/// Response body for `GET /health`.
#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

/// `GET /health` — returns 200 OK with a simple JSON body.
///
/// This endpoint is mounted outside the auth middleware so it can be used
/// for load balancer health checks without a bearer token.
pub async fn health() -> impl IntoResponse {
    (StatusCode::OK, axum::Json(HealthResponse { status: "ok" }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    #[tokio::test]
    async fn health_returns_200_ok() {
        let resp = health().await.into_response();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["status"], "ok");
    }
}

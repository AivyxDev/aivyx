//! Health check endpoint.

use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Serialize;

/// Response body for `GET /health`.
#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    instance_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scaling_mode: Option<&'static str>,
}

/// `GET /health` — returns 200 OK with a simple JSON body.
///
/// This endpoint is mounted outside the auth middleware so it can be used
/// for load balancer health checks without a bearer token.
pub async fn health() -> impl IntoResponse {
    // Instance ID and scaling mode are populated from env vars
    // rather than AppState to keep the health endpoint stateless
    // (it's outside the auth middleware and doesn't have access to state).
    let instance_id = std::env::var("AIVYX_INSTANCE_ID").ok();
    let scaling_mode = if std::env::var("AIVYX_STATELESS_MODE").map_or(false, |v| v == "1") {
        Some("stateless")
    } else {
        None
    };

    (
        StatusCode::OK,
        axum::Json(HealthResponse {
            status: "ok",
            instance_id,
            scaling_mode,
        }),
    )
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

    #[tokio::test]
    async fn health_includes_scaling_fields_when_configured() {
        // The health endpoint reads from env vars, so we test the struct serialization
        let resp = HealthResponse {
            status: "ok",
            instance_id: Some("node-1".into()),
            scaling_mode: Some("stateless"),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["instance_id"], "node-1");
        assert_eq!(json["scaling_mode"], "stateless");
    }

    #[tokio::test]
    async fn health_omits_scaling_fields_when_not_configured() {
        let resp = HealthResponse {
            status: "ok",
            instance_id: None,
            scaling_mode: None,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json.get("instance_id").is_none());
        assert!(json.get("scaling_mode").is_none());
    }
}

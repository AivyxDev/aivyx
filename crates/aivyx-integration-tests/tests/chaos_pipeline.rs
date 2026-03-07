//! Integration tests for the chaos fault-injection middleware.
//!
//! These tests build a minimal axum router with the chaos middleware layer
//! applied directly, avoiding the full `AppState` / `build_router()` setup.

use std::sync::Arc;

use aivyx_server::middleware::chaos::{ChaosConfig, ChaosLayer};
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use http_body_util::BodyExt;
use tower::ServiceExt;

/// Simple health handler that returns "ok".
async fn health_handler() -> &'static str {
    "ok"
}

/// Build a minimal router with the chaos middleware applied.
fn test_router(config: ChaosConfig) -> Router {
    let config = Arc::new(config);
    Router::new()
        .route("/health", get(health_handler))
        .layer(ChaosLayer::new(config))
}

/// Helper: send a GET /health request to the given router.
async fn get_health(router: Router) -> (StatusCode, String) {
    let req = Request::builder()
        .uri("/health")
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&body).to_string();
    (status, text)
}

// ────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn chaos_disabled_passes_through() {
    let router = test_router(ChaosConfig {
        enabled: false,
        http_error_probability: 1.0,
        latency_probability: 1.0,
        latency_ms: 5000,
        corrupt_body_probability: 1.0,
    });

    let (status, body) = get_health(router).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn chaos_http_error_prob_1_returns_500() {
    let router = test_router(ChaosConfig {
        enabled: true,
        http_error_probability: 1.0,
        latency_probability: 0.0,
        latency_ms: 0,
        corrupt_body_probability: 0.0,
    });

    let (status, body) = get_health(router).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(body.contains("chaos: injected server error"));
}

#[tokio::test]
async fn chaos_http_error_prob_0_never_injects() {
    let router = test_router(ChaosConfig {
        enabled: true,
        http_error_probability: 0.0,
        latency_probability: 0.0,
        latency_ms: 0,
        corrupt_body_probability: 0.0,
    });

    let (status, body) = get_health(router).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn chaos_latency_still_returns_response() {
    let router = test_router(ChaosConfig {
        enabled: true,
        http_error_probability: 0.0,
        latency_probability: 1.0,
        latency_ms: 10,
        corrupt_body_probability: 0.0,
    });

    let (status, body) = get_health(router).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn chaos_corrupt_body_returns_empty() {
    let router = test_router(ChaosConfig {
        enabled: true,
        http_error_probability: 0.0,
        latency_probability: 0.0,
        latency_ms: 0,
        corrupt_body_probability: 1.0,
    });

    let (status, body) = get_health(router).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.is_empty(), "expected empty body, got: {body}");
}

#[tokio::test]
async fn chaos_multiple_fault_types() {
    // http_error 0.0 means we pass through to the handler,
    // latency 1.0 adds a delay, then corrupt 1.0 replaces the body.
    let router = test_router(ChaosConfig {
        enabled: true,
        http_error_probability: 0.0,
        latency_probability: 1.0,
        latency_ms: 10,
        corrupt_body_probability: 1.0,
    });

    let (status, body) = get_health(router).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.is_empty(),
        "expected empty body after latency + corruption, got: {body}"
    );
}

#[test]
fn chaos_config_serde_roundtrip() {
    let config = ChaosConfig {
        enabled: true,
        http_error_probability: 0.25,
        latency_probability: 0.5,
        latency_ms: 200,
        corrupt_body_probability: 0.1,
    };

    let json = serde_json::to_string(&config).unwrap();
    let deserialized: ChaosConfig = serde_json::from_str(&json).unwrap();

    assert!(deserialized.enabled);
    assert!((deserialized.http_error_probability - 0.25).abs() < f64::EPSILON);
    assert!((deserialized.latency_probability - 0.5).abs() < f64::EPSILON);
    assert_eq!(deserialized.latency_ms, 200);
    assert!((deserialized.corrupt_body_probability - 0.1).abs() < f64::EPSILON);
}

#[test]
fn chaos_config_default_is_disabled() {
    let config = ChaosConfig::default();
    assert!(!config.enabled);
    assert!((config.http_error_probability - 0.0).abs() < f64::EPSILON);
    assert!((config.latency_probability - 0.0).abs() < f64::EPSILON);
    assert_eq!(config.latency_ms, 0);
    assert!((config.corrupt_body_probability - 0.0).abs() < f64::EPSILON);
}

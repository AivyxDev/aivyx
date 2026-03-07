//! Integration tests for the aivyx-server HTTP API.
//!
//! Uses `tower::ServiceExt::oneshot` to test the router without a TCP listener.

use std::sync::Arc;

use aivyx_agent::{AgentProfile, AgentSession, SessionStore};
use aivyx_audit::AuditLog;
use aivyx_config::{AivyxConfig, AivyxDirs};
use aivyx_crypto::{MasterKey, derive_audit_key};
use aivyx_server::{AppState, build_router};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

const TEST_TOKEN: &str = "test-bearer-token-abc123";

/// Create a test AppState with temp dirs, mock agent profile, and known token.
fn setup_test_state() -> (Arc<AppState>, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("aivyx-server-test-{}", rand::random::<u64>()));
    std::fs::create_dir_all(dir.join("agents")).unwrap();
    std::fs::create_dir_all(dir.join("teams")).unwrap();
    std::fs::create_dir_all(dir.join("sessions")).unwrap();
    std::fs::create_dir_all(dir.join("keys")).unwrap();
    std::fs::create_dir_all(dir.join("memory")).unwrap();

    // Write a test agent profile
    let profile = AgentProfile::template("test-agent", "test assistant");
    let profile_path = dir.join("agents").join("test-agent.toml");
    profile.save(&profile_path).unwrap();

    let dirs = AivyxDirs::new(&dir);
    let config = AivyxConfig::default();
    let master_key = MasterKey::from_bytes([42u8; 32]);
    let agent_key = MasterKey::from_bytes([42u8; 32]);

    let audit_key = derive_audit_key(&master_key);
    let audit_log = AuditLog::new(dir.join("audit.log"), &audit_key);
    let session_store = SessionStore::open(dir.join("sessions").join("sessions.db")).unwrap();

    let mut hasher = Sha256::new();
    hasher.update(TEST_TOKEN.as_bytes());
    let bearer_token_hash: [u8; 32] = hasher.finalize().into();

    let agent_dirs = AivyxDirs::new(&dir);
    let state = Arc::new(AppState {
        agent_session: Arc::new(AgentSession::new(agent_dirs, config.clone(), agent_key)),
        session_store,
        memory_manager: None,
        audit_log,
        master_key,
        dirs,
        config,
        bearer_token_hash: tokio::sync::RwLock::new(bearer_token_hash),
        auth_rate_limiter: std::sync::Mutex::new(std::collections::HashMap::new()),
        sidecar_mode: false,
        endpoint_rate_limiters: None,
        federation: None,
    });

    (state, dir)
}

fn auth_header() -> (&'static str, String) {
    ("authorization", format!("Bearer {}", TEST_TOKEN))
}

async fn response_body(resp: axum::response::Response) -> serde_json::Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

// ---------------------------------------------------------------------------
// Health
// ---------------------------------------------------------------------------

#[tokio::test]
async fn health_returns_200_no_auth() {
    let (state, dir) = setup_test_state();
    let router = build_router(state);

    let req = Request::builder()
        .uri("/health")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_body(resp).await;
    assert_eq!(body["status"], "ok");

    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

#[tokio::test]
async fn auth_missing_returns_401() {
    let (state, dir) = setup_test_state();
    let router = build_router(state);

    let req = Request::builder()
        .uri("/status")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn auth_wrong_token_returns_401() {
    let (state, dir) = setup_test_state();
    let router = build_router(state);

    let req = Request::builder()
        .uri("/status")
        .header("authorization", "Bearer wrong-token")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn auth_valid_token_passes() {
    let (state, dir) = setup_test_state();
    let router = build_router(state);

    let (key, value) = auth_header();
    let req = Request::builder()
        .uri("/status")
        .header(key, value)
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------
// Security headers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn security_headers_present() {
    let (state, dir) = setup_test_state();
    let router = build_router(state);

    let req = Request::builder()
        .uri("/health")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(
        resp.headers().get("x-content-type-options").unwrap(),
        "nosniff"
    );
    assert_eq!(resp.headers().get("x-frame-options").unwrap(), "DENY");

    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------
// Agents
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_agents_returns_profiles() {
    let (state, dir) = setup_test_state();
    let router = build_router(state);

    let (key, value) = auth_header();
    let req = Request::builder()
        .uri("/agents")
        .header(key, value)
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_body(resp).await;
    let agents = body.as_array().unwrap();
    assert!(!agents.is_empty());
    assert_eq!(agents[0]["name"], "test-agent");

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn get_agent_returns_profile() {
    let (state, dir) = setup_test_state();
    let router = build_router(state);

    let (key, value) = auth_header();
    let req = Request::builder()
        .uri("/agents/test-agent")
        .header(key, value)
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_body(resp).await;
    assert_eq!(body["name"], "test-agent");

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn get_agent_not_found_returns_404() {
    let (state, dir) = setup_test_state();
    let router = build_router(state);

    let (key, value) = auth_header();
    let req = Request::builder()
        .uri("/agents/nonexistent")
        .header(key, value)
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn create_agent_returns_201() {
    let (state, dir) = setup_test_state();
    let router = build_router(state);

    let (key, value) = auth_header();
    let body = serde_json::json!({"name": "new-agent", "role": "helper"});
    let req = Request::builder()
        .method("POST")
        .uri("/agents")
        .header(key, value)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_sessions_empty() {
    let (state, dir) = setup_test_state();
    let router = build_router(state);

    let (key, value) = auth_header();
    let req = Request::builder()
        .uri("/sessions")
        .header(key, value)
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_body(resp).await;
    assert!(body.as_array().unwrap().is_empty());

    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn status_returns_summary() {
    let (state, dir) = setup_test_state();
    let router = build_router(state);

    let (key, value) = auth_header();
    let req = Request::builder()
        .uri("/status")
        .header(key, value)
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_body(resp).await;
    assert_eq!(body["agent_count"], 1);
    assert!(body["memory"].is_null());

    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------
// Audit
// ---------------------------------------------------------------------------

#[tokio::test]
async fn audit_recent_returns_entries() {
    let (state, dir) = setup_test_state();

    // Append a test event
    state
        .audit_log
        .append(aivyx_audit::AuditEvent::SystemInit {
            timestamp: chrono::Utc::now(),
        })
        .unwrap();

    let router = build_router(state);

    let (key, value) = auth_header();
    let req = Request::builder()
        .uri("/audit?last=5")
        .header(key, value)
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_body(resp).await;
    let entries = body.as_array().unwrap();
    assert_eq!(entries.len(), 1);

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn audit_verify_returns_valid() {
    let (state, dir) = setup_test_state();

    state
        .audit_log
        .append(aivyx_audit::AuditEvent::SystemInit {
            timestamp: chrono::Utc::now(),
        })
        .unwrap();

    let router = build_router(state);

    let (key, value) = auth_header();
    let req = Request::builder()
        .method("POST")
        .uri("/audit/verify")
        .header(key, value)
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_body(resp).await;
    assert_eq!(body["valid"], true);

    std::fs::remove_dir_all(&dir).ok();
}

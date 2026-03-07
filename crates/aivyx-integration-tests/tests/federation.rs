//! Integration tests for federation routes.
//!
//! Uses `tower::ServiceExt::oneshot` to test the router without a TCP listener,
//! following the same pattern as `server_pipeline.rs`.

use std::collections::HashMap;
use std::sync::Arc;

use aivyx_agent::{AgentProfile, AgentSession, SessionStore};
use aivyx_audit::AuditLog;
use aivyx_config::{AivyxConfig, AivyxDirs};
use aivyx_crypto::{MasterKey, derive_audit_key};
use aivyx_federation::auth::FederationAuth;
use aivyx_federation::client::FederationClient;
use aivyx_federation::config::FederationConfig;
use aivyx_server::{AppState, build_router};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

const TEST_TOKEN: &str = "test-bearer-token-abc123";

/// Create a test AppState with federation enabled.
///
/// The `FederationClient` is configured with no peers (empty peer list),
/// so `list_peers` returns an empty list and relay requests to unknown
/// peers are denied.
fn setup_federation_state() -> (Arc<AppState>, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "aivyx-federation-test-{}",
        rand::random::<u64>()
    ));
    std::fs::create_dir_all(dir.join("agents")).unwrap();
    std::fs::create_dir_all(dir.join("teams")).unwrap();
    std::fs::create_dir_all(dir.join("sessions")).unwrap();
    std::fs::create_dir_all(dir.join("keys")).unwrap();
    std::fs::create_dir_all(dir.join("memory")).unwrap();
    std::fs::create_dir_all(dir.join("billing")).unwrap();

    // Write a test agent profile so /federation/ping returns a non-empty agents list
    let profile = AgentProfile::template("federation-agent", "federation test assistant");
    let profile_path = dir.join("agents").join("federation-agent.toml");
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

    let cost_ledger = Arc::new(
        aivyx_billing::CostLedger::open(dir.join("billing").join("costs.db")).unwrap(),
    );

    // Build a FederationClient with no peers — federation is "enabled" but
    // has no configured peers, so list_peers returns [] and relay fails.
    let fed_config = FederationConfig {
        instance_id: "test-instance-001".to_string(),
        enabled: true,
        private_key_path: None,
        peers: Vec::new(),
        failover: Default::default(),
    };
    let fed_auth = FederationAuth::generate("test-instance-001".to_string());
    let federation_client = Arc::new(FederationClient::new(fed_config, fed_auth));

    let agent_dirs = AivyxDirs::new(&dir);
    let state = Arc::new(AppState {
        agent_session: Arc::new(AgentSession::new(agent_dirs, config.clone(), agent_key)),
        session_store,
        memory_manager: None,
        audit_log,
        master_key,
        dirs,
        config: Arc::new(tokio::sync::RwLock::new(config)),
        push_notification_configs: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        bearer_token_hash: tokio::sync::RwLock::new(bearer_token_hash),
        auth_rate_limiter: std::sync::Mutex::new(std::collections::HashMap::new()),
        sidecar_mode: false,
        endpoint_rate_limiters: None,
        federation: Some(federation_client),
        prometheus_handle: None,
        tenant_store: None,
        api_key_store: None,
        multi_tenant_enabled: false,
        cost_ledger,
        budget_enforcer: None,
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
// Federation ping
// ---------------------------------------------------------------------------

/// GET /federation/ping returns instance_id and agents list when federation
/// is enabled.
#[tokio::test]
async fn federation_ping() {
    let (state, dir) = setup_federation_state();
    let router = build_router(state).await;

    let (key, value) = auth_header();
    let req = Request::builder()
        .uri("/federation/ping")
        .header(key, value)
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = response_body(resp).await;
    assert_eq!(body["instance_id"], "test-instance-001");
    assert!(body["agents"].is_array(), "agents should be an array");
    // We created a "federation-agent" profile, so it should appear in the list
    let agents = body["agents"].as_array().unwrap();
    let agent_names: Vec<&str> = agents.iter().filter_map(|a| a.as_str()).collect();
    assert!(
        agent_names.contains(&"federation-agent"),
        "expected 'federation-agent' in agents list, got: {:?}",
        agent_names,
    );

    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------
// Federation peers
// ---------------------------------------------------------------------------

/// GET /federation/peers returns the configured peers (empty list in this test).
#[tokio::test]
async fn federation_peers() {
    let (state, dir) = setup_federation_state();
    let router = build_router(state).await;

    let (key, value) = auth_header();
    let req = Request::builder()
        .uri("/federation/peers")
        .header(key, value)
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = response_body(resp).await;
    assert!(body["peers"].is_array(), "peers should be an array");
    let peers = body["peers"].as_array().unwrap();
    assert!(peers.is_empty(), "peers list should be empty when no peers are configured");

    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------
// Federation relay without trust
// ---------------------------------------------------------------------------

/// POST /federation/relay/chat without a configured trust policy for the target
/// peer returns 403 Forbidden.
#[tokio::test]
async fn federation_relay_without_trust() {
    let (state, dir) = setup_federation_state();
    let router = build_router(state).await;

    let (key, value) = auth_header();
    let relay_body = serde_json::json!({
        "peer_id": "unknown-peer",
        "agent": "some-agent",
        "message": "hello from federation",
    });

    let req = Request::builder()
        .method("POST")
        .uri("/federation/relay/chat")
        .header(key, value)
        .header("content-type", "application/json")
        .body(Body::from(relay_body.to_string()))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    // The relay_chat handler checks peer_trust_policy() first. Since there are
    // no peers configured at all, the peer lookup returns None for the trust
    // policy, resulting in a 403 Forbidden response.
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let body = response_body(resp).await;
    assert!(
        body["error"].as_str().unwrap().contains("no trust policy"),
        "error message should mention missing trust policy, got: {}",
        body["error"],
    );

    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------
// Ed25519 signature roundtrip (unit test)
// ---------------------------------------------------------------------------

/// Sign a request body with FederationAuth and verify it with the corresponding
/// public key. This exercises the Ed25519 sign/verify path used by federation
/// peer-to-peer communication.
#[test]
fn federation_ed25519_signature_roundtrip() {
    let auth = FederationAuth::generate("roundtrip-test".to_string());
    let public_key = auth.public_key_base64();

    // Sign a sample body
    let body = b"federation integration test payload";
    let signed_header = auth.sign_request(body);

    // Verify the signature
    assert_eq!(signed_header.instance_id, "roundtrip-test");
    FederationAuth::verify_request(&public_key, &signed_header, body)
        .expect("valid signature should verify successfully");

    // Verify that a tampered body is rejected
    let tampered = b"tampered payload";
    let result = FederationAuth::verify_request(&public_key, &signed_header, tampered);
    assert!(
        result.is_err(),
        "tampered body should fail verification"
    );

    // Verify that signing with a different instance produces a different signature
    // that cannot be verified with the first instance's public key
    let other_auth = FederationAuth::generate("other-instance".to_string());
    let other_header = other_auth.sign_request(body);
    let result = FederationAuth::verify_request(&public_key, &other_header, body);
    assert!(
        result.is_err(),
        "signature from a different keypair should fail verification"
    );
}

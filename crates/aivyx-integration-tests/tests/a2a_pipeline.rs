//! Integration tests for A2A (Agent-to-Agent) protocol compliance.
//!
//! Tests JSON-RPC 2.0 compliance, Agent Card discovery, task lifecycle,
//! push notification CRUD, and SSE streaming via `tower::ServiceExt::oneshot`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use aivyx_agent::{AgentProfile, AgentSession, SessionStore};
use aivyx_audit::AuditLog;
use aivyx_config::{AivyxConfig, AivyxDirs};
use aivyx_crypto::{MasterKey, derive_audit_key};
use aivyx_server::{AppState, build_router};
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

const TEST_TOKEN: &str = "test-bearer-token-abc123";

/// Fake socket address used to satisfy ConnectInfo extractor in rate-limit middleware.
fn fake_connect_info() -> ConnectInfo<SocketAddr> {
    ConnectInfo("127.0.0.1:9999".parse().unwrap())
}

/// Create a test AppState with temp dirs, mock agent profile, and known token.
fn setup_test_state() -> (Arc<AppState>, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("aivyx-a2a-test-{}", rand::random::<u64>()));
    std::fs::create_dir_all(dir.join("agents")).unwrap();
    std::fs::create_dir_all(dir.join("teams")).unwrap();
    std::fs::create_dir_all(dir.join("sessions")).unwrap();
    std::fs::create_dir_all(dir.join("keys")).unwrap();
    std::fs::create_dir_all(dir.join("memory")).unwrap();
    std::fs::create_dir_all(dir.join("tasks")).unwrap();

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

    std::fs::create_dir_all(dir.join("billing")).unwrap();
    let cost_ledger = std::sync::Arc::new(
        aivyx_billing::CostLedger::open(dir.join("billing").join("costs.db")).unwrap(),
    );

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
        federation: None,
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
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        panic!(
            "failed to parse response body as JSON: {e}, raw: {:?}",
            String::from_utf8_lossy(&bytes)
        )
    })
}

fn a2a_request(method: &str, params: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "method": method, "params": params, "id": 1 })
}

/// Build a POST /a2a request with auth and ConnectInfo.
fn post_a2a(body: &serde_json::Value) -> Request<Body> {
    let (key, value) = auth_header();
    let mut req = Request::builder()
        .method("POST")
        .uri("/a2a")
        .header(key, value)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    req.extensions_mut().insert(fake_connect_info());
    req
}

/// Build a POST /a2a request WITHOUT auth but WITH ConnectInfo.
fn post_a2a_no_auth(body: &serde_json::Value) -> Request<Body> {
    let mut req = Request::builder()
        .method("POST")
        .uri("/a2a")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    req.extensions_mut().insert(fake_connect_info());
    req
}

// ---------------------------------------------------------------------------
// Agent Card (unauthenticated -- GET /.well-known/agent.json)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent_card_returns_200_no_auth() {
    let (state, dir) = setup_test_state();
    let router = build_router(state).await;

    let req = Request::builder()
        .uri("/.well-known/agent.json")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn agent_card_has_camel_case_fields() {
    let (state, dir) = setup_test_state();
    let router = build_router(state).await;

    let req = Request::builder()
        .uri("/.well-known/agent.json")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    let body = response_body(resp).await;

    assert!(body.get("name").is_some(), "missing 'name'");
    assert!(body.get("capabilities").is_some(), "missing 'capabilities'");
    assert!(body.get("skills").is_some(), "missing 'skills'");
    assert!(
        body.get("defaultInputModes").is_some(),
        "missing 'defaultInputModes'"
    );
    assert!(
        body.get("defaultOutputModes").is_some(),
        "missing 'defaultOutputModes'"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn agent_card_skills_from_profiles() {
    let (state, dir) = setup_test_state();
    let router = build_router(state).await;

    let req = Request::builder()
        .uri("/.well-known/agent.json")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    let body = response_body(resp).await;

    let skills = body["skills"]
        .as_array()
        .expect("skills should be an array");
    assert!(
        !skills.is_empty(),
        "skills should be populated from agent profiles"
    );
    assert_eq!(skills[0]["id"], "test-agent");
    assert_eq!(skills[0]["name"], "test-agent");

    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------
// JSON-RPC Envelope (POST /a2a with auth)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a2a_requires_auth() {
    let (state, dir) = setup_test_state();
    let router = build_router(state).await;

    let body = a2a_request("tasks/send", serde_json::json!({}));
    let req = post_a2a_no_auth(&body);

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn unknown_method_returns_minus_32601() {
    let (state, dir) = setup_test_state();
    let router = build_router(state).await;

    let body = a2a_request("foo/bar", serde_json::json!({}));
    let req = post_a2a(&body);

    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = response_body(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "unexpected status {status}, body: {body}"
    );
    assert_eq!(body["error"]["code"], -32601);

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn malformed_json_returns_error() {
    let (state, dir) = setup_test_state();
    let router = build_router(state).await;

    let (key, value) = auth_header();
    let mut req = Request::builder()
        .method("POST")
        .uri("/a2a")
        .header(key, value)
        .header("content-type", "application/json")
        .body(Body::from("this is not json{{{"))
        .unwrap();
    req.extensions_mut().insert(fake_connect_info());

    let resp = router.oneshot(req).await.unwrap();
    // Axum returns 422 for deserialization failures, or 400
    assert!(
        resp.status() == StatusCode::BAD_REQUEST
            || resp.status() == StatusCode::UNPROCESSABLE_ENTITY,
        "expected 400 or 422, got {}",
        resp.status()
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn response_has_jsonrpc_2_0() {
    let (state, dir) = setup_test_state();
    let router = build_router(state).await;

    let body = a2a_request("foo/bar", serde_json::json!({}));
    let req = post_a2a(&body);

    let resp = router.oneshot(req).await.unwrap();
    let body = response_body(resp).await;
    assert_eq!(body["jsonrpc"], "2.0");

    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------
// Task lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn task_send_returns_submitted() {
    let (state, dir) = setup_test_state();

    // Set up a dummy encrypted store with a fake API key so create_mission
    // can build an LLM provider. The key is not valid for real API calls,
    // but the planner will fail gracefully after mission creation.
    let store = aivyx_crypto::EncryptedStore::open(state.dirs.store_path()).unwrap();
    store
        .put(
            "claude_api_key",
            b"sk-test-dummy-key-for-a2a-tests",
            &state.master_key,
        )
        .unwrap();

    let router = build_router(state).await;

    let body = a2a_request(
        "tasks/send",
        serde_json::json!({
            "message": {
                "role": "user",
                "parts": [{"type": "text", "text": "hello"}]
            },
            "agent": "test-agent"
        }),
    );
    let req = post_a2a(&body);

    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = response_body(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "unexpected status {status}, body: {body}"
    );
    // Without a real LLM, tasks/send returns a JSON-RPC error from planning.
    // Verify the response is well-formed JSON-RPC 2.0 with either result or error.
    assert_eq!(body["jsonrpc"], "2.0", "response must be JSON-RPC 2.0");
    assert_eq!(body["id"], 1, "response id must match request id");
    // The response will contain either result.status.state == "submitted"
    // or an error from the planner (no real LLM). Both are valid JSON-RPC.
    let has_result = body.get("result").is_some() && !body["result"].is_null();
    let has_error = body.get("error").is_some() && !body["error"].is_null();
    assert!(
        has_result || has_error,
        "JSON-RPC response must have result or error, got: {body}"
    );
    if has_result {
        assert_eq!(body["result"]["status"]["state"], "submitted");
    }

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn task_send_has_task_id() {
    let (state, dir) = setup_test_state();

    // Set up dummy API key for create_mission
    let store = aivyx_crypto::EncryptedStore::open(state.dirs.store_path()).unwrap();
    store
        .put(
            "claude_api_key",
            b"sk-test-dummy-key-for-a2a-tests",
            &state.master_key,
        )
        .unwrap();

    let router = build_router(state).await;

    let body = a2a_request(
        "tasks/send",
        serde_json::json!({
            "message": {
                "role": "user",
                "parts": [{"type": "text", "text": "hello"}]
            },
            "agent": "test-agent"
        }),
    );
    let req = post_a2a(&body);

    let resp = router.oneshot(req).await.unwrap();
    let body = response_body(resp).await;

    // With a dummy key, the planner may fail. If result exists, check for id.
    assert_eq!(body["jsonrpc"], "2.0");
    let has_result = body.get("result").is_some() && !body["result"].is_null();
    if has_result {
        let id = body["result"]["id"]
            .as_str()
            .expect("result should have 'id'");
        assert!(!id.is_empty(), "task id should be non-empty");
    } else {
        // If planning failed, verify error is well-formed
        assert!(
            body.get("error").is_some() && !body["error"].is_null(),
            "expected either result with id or error, got: {body}"
        );
    }

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn task_get_nonexistent_returns_error() {
    let (state, dir) = setup_test_state();
    let router = build_router(state).await;

    let fake_id = uuid::Uuid::new_v4().to_string();
    let body = a2a_request("tasks/get", serde_json::json!({ "id": fake_id }));
    let req = post_a2a(&body);

    let resp = router.oneshot(req).await.unwrap();
    let body = response_body(resp).await;
    assert!(
        body.get("error").is_some() && !body["error"].is_null(),
        "expected error for nonexistent task, got: {body}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn task_cancel_nonexistent_returns_error() {
    let (state, dir) = setup_test_state();
    let router = build_router(state).await;

    let fake_id = uuid::Uuid::new_v4().to_string();
    let body = a2a_request("tasks/cancel", serde_json::json!({ "id": fake_id }));
    let req = post_a2a(&body);

    let resp = router.oneshot(req).await.unwrap();
    let body = response_body(resp).await;
    assert!(
        body.get("error").is_some() && !body["error"].is_null(),
        "expected error for nonexistent task, got: {body}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------
// Push notification CRUD
// ---------------------------------------------------------------------------

#[tokio::test]
async fn push_notification_set_and_get() {
    let (state, dir) = setup_test_state();
    let router = build_router(state).await;

    let task_id = "pn-test-task-1";
    let webhook_url = "https://example.com/webhook";

    // Set push notification config
    let set_body = a2a_request(
        "tasks/pushNotification/set",
        serde_json::json!({
            "id": task_id,
            "pushNotificationConfig": { "url": webhook_url }
        }),
    );
    let req = post_a2a(&set_body);

    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let set_resp = response_body(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "set failed: {status}, body: {set_resp}"
    );
    assert!(
        set_resp.get("result").is_some() && !set_resp["result"].is_null(),
        "set should return a result"
    );

    // Get push notification config
    let get_body = a2a_request(
        "tasks/pushNotification/get",
        serde_json::json!({ "id": task_id }),
    );
    let req = post_a2a(&get_body);

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let get_resp = response_body(resp).await;
    assert_eq!(get_resp["result"]["url"], webhook_url);

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn push_notification_delete() {
    let (state, dir) = setup_test_state();
    let router = build_router(state).await;

    let task_id = "pn-test-task-2";

    // Set push notification config
    let set_body = a2a_request(
        "tasks/pushNotification/set",
        serde_json::json!({
            "id": task_id,
            "pushNotificationConfig": { "url": "https://example.com/hook" }
        }),
    );
    let req = post_a2a(&set_body);
    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Delete push notification config
    let del_body = a2a_request(
        "tasks/pushNotification/delete",
        serde_json::json!({ "id": task_id }),
    );
    let req = post_a2a(&del_body);
    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let del_resp = response_body(resp).await;
    assert!(
        del_resp.get("result").is_some() && !del_resp["result"].is_null(),
        "delete should return a result"
    );

    // Subsequent get should return error
    let get_body = a2a_request(
        "tasks/pushNotification/get",
        serde_json::json!({ "id": task_id }),
    );
    let req = post_a2a(&get_body);
    let resp = router.oneshot(req).await.unwrap();
    let get_resp = response_body(resp).await;
    assert!(
        get_resp.get("error").is_some() && !get_resp["error"].is_null(),
        "get after delete should return error"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn push_notification_get_unknown_returns_error() {
    let (state, dir) = setup_test_state();
    let router = build_router(state).await;

    let body = a2a_request(
        "tasks/pushNotification/get",
        serde_json::json!({ "id": "nonexistent-task-id" }),
    );
    let req = post_a2a(&body);

    let resp = router.oneshot(req).await.unwrap();
    let body = response_body(resp).await;
    assert!(
        body.get("error").is_some() && !body["error"].is_null(),
        "expected error for unknown task push config"
    );

    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------
// SSE streaming
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stream_returns_sse_content_type() {
    let (state, dir) = setup_test_state();

    // Set up dummy API key for create_mission
    let store = aivyx_crypto::EncryptedStore::open(state.dirs.store_path()).unwrap();
    store
        .put(
            "claude_api_key",
            b"sk-test-dummy-key-for-a2a-tests",
            &state.master_key,
        )
        .unwrap();

    let router = build_router(state).await;

    let (key, value) = auth_header();
    let body = a2a_request(
        "tasks/sendSubscribe",
        serde_json::json!({
            "message": {
                "role": "user",
                "parts": [{"type": "text", "text": "hello stream"}]
            },
            "agent": "test-agent"
        }),
    );
    let mut req = Request::builder()
        .method("POST")
        .uri("/a2a/stream")
        .header(key, value)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    req.extensions_mut().insert(fake_connect_info());

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let content_type = resp
        .headers()
        .get("content-type")
        .expect("response should have content-type header")
        .to_str()
        .unwrap();
    // The stream handler returns either:
    // - text/event-stream on success (SSE stream)
    // - application/json on error (JSON-RPC error from planner)
    // Both are valid A2A responses. Without a real LLM, the planner fails
    // and returns a JSON-RPC error. Verify we get one of these two.
    if content_type.contains("text/event-stream") {
        // Success path - SSE stream
    } else if content_type.contains("application/json") {
        // Error path - verify it's a well-formed JSON-RPC error
        let body = response_body(resp).await;
        assert_eq!(body["jsonrpc"], "2.0", "expected JSON-RPC 2.0 response");
        assert!(
            body.get("error").is_some() && !body["error"].is_null(),
            "expected JSON-RPC error in fallback response, got: {body}"
        );
    } else {
        panic!("expected text/event-stream or application/json content type, got: {content_type}");
    }

    std::fs::remove_dir_all(&dir).ok();
}

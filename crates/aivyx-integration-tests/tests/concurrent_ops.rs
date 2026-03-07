//! Integration tests for concurrent operations across crates.
//!
//! These tests verify that core data structures are safe under concurrent
//! access via `tokio::spawn` and `tokio::join!`.

use std::path::PathBuf;
use std::sync::Arc;

use aivyx_agent::SessionStore;
use aivyx_agent::session_store::{PersistedSession, SessionMetadata};
use aivyx_audit::{AuditEvent, AuditLog};
use aivyx_core::{AgentId, AutonomyTier, SessionId};
use aivyx_crypto::{EncryptedStore, MasterKey, derive_audit_key};
use aivyx_llm::ChatMessage;
use chrono::Utc;

// ────────────────────────────────────────────────────────────────────
// Helpers
// ────────────────────────────────────────────────────────────────────

fn temp_dir(prefix: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("aivyx-integ-{prefix}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn cleanup(dir: &PathBuf) {
    std::fs::remove_dir_all(dir).ok();
}

// ────────────────────────────────────────────────────────────────────
// 1. Concurrent audit appends
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn concurrent_audit_appends() {
    let dir = temp_dir("conc-audit");
    let audit_key = derive_audit_key(&MasterKey::from_bytes([33u8; 32]));
    let log = Arc::new(AuditLog::new(dir.join("audit.log"), &audit_key));

    let mut handles = Vec::new();
    for i in 0..10u32 {
        let log = Arc::clone(&log);
        handles.push(tokio::spawn(async move {
            let event = AuditEvent::AgentCreated {
                agent_id: AgentId::new(),
                autonomy_tier: if i % 2 == 0 {
                    AutonomyTier::Trust
                } else {
                    AutonomyTier::Leash
                },
            };
            log.append(event).unwrap();
        }));
    }

    // Join all
    for h in handles {
        h.await.unwrap();
    }

    // Verify 10 entries
    let count = log.len().unwrap();
    assert_eq!(
        count, 10,
        "should have 10 audit entries after concurrent appends"
    );

    // Verify chain integrity
    let result = log.verify().unwrap();
    assert!(
        result.valid,
        "audit chain should be valid after concurrent appends"
    );
    assert_eq!(result.entries_checked, 10);

    cleanup(&dir);
}

// ────────────────────────────────────────────────────────────────────
// 2. Concurrent encrypted store access
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn concurrent_encrypted_store_access() {
    let dir = temp_dir("conc-store");
    let store = Arc::new(EncryptedStore::open(dir.join("store.db")).unwrap());
    let key = Arc::new(MasterKey::from_bytes([44u8; 32]));

    let mut handles = Vec::new();
    for i in 0..10u32 {
        let store = Arc::clone(&store);
        let key = Arc::clone(&key);
        handles.push(tokio::spawn(async move {
            let k = format!("key-{i}");
            let v = format!("value-{i}");
            store.put(&k, v.as_bytes(), &key).unwrap();
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    // Read all 10 and verify
    for i in 0..10u32 {
        let k = format!("key-{i}");
        let expected = format!("value-{i}");
        let data = store.get(&k, &key).unwrap();
        assert!(data.is_some(), "key-{i} should exist");
        let actual = String::from_utf8(data.unwrap()).unwrap();
        assert_eq!(actual, expected, "value mismatch for key-{i}");
    }

    // Verify list_keys returns all 10
    let keys = store.list_keys().unwrap();
    assert_eq!(keys.len(), 10, "should have 10 keys");

    cleanup(&dir);
}

// ────────────────────────────────────────────────────────────────────
// 3. Concurrent session saves
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn concurrent_session_saves() {
    let dir = temp_dir("conc-sessions");
    let store = Arc::new(SessionStore::open(dir.join("sessions.db")).unwrap());
    let key = Arc::new(MasterKey::from_bytes([55u8; 32]));

    let mut session_ids = Vec::new();
    let mut handles = Vec::new();

    for i in 0..5u32 {
        let store = Arc::clone(&store);
        let key = Arc::clone(&key);
        let sid = SessionId::new();
        session_ids.push(sid);

        handles.push(tokio::spawn(async move {
            let session = PersistedSession {
                metadata: SessionMetadata {
                    session_id: sid,
                    agent_name: format!("agent-{i}"),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    message_count: 2,
                },
                messages: vec![
                    ChatMessage::user(format!("hello from {i}")),
                    ChatMessage::assistant(format!("reply to {i}")),
                ],
            };
            store.save(&session, &key).unwrap();
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    // Verify all 5 sessions exist
    let list = store.list(&key).unwrap();
    assert_eq!(
        list.len(),
        5,
        "should have 5 sessions after concurrent saves"
    );

    // Verify each session can be loaded individually
    for (i, sid) in session_ids.iter().enumerate() {
        let loaded = store.load(sid, &key, 0).unwrap();
        assert!(loaded.is_some(), "session {i} should be loadable");
        let loaded = loaded.unwrap();
        assert_eq!(loaded.metadata.agent_name, format!("agent-{i}"));
        assert_eq!(loaded.messages.len(), 2);
    }

    cleanup(&dir);
}

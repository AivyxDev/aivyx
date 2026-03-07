//! Integration tests for team session persistence and cross-crate boundaries.
//!
//! Exercises: aivyx-crypto (key derivation), aivyx-team (TeamSessionStore),
//! aivyx-audit (new events + metrics), aivyx-capability (attenuation),
//! aivyx-config (skill validation).

use std::collections::HashMap;

use aivyx_audit::{AuditEntry, AuditEvent, compute_summary};
use aivyx_core::CapabilityScope;
use aivyx_crypto::{MasterKey, derive_team_session_key};
use aivyx_llm::ChatMessage;
use aivyx_team::{PersistedTeamSession, TeamSessionStore};

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("aivyx-integ-{name}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Test 1: Team session roundtrip — save with lead + specialist conversations, load, verify.
#[test]
fn team_session_roundtrip() {
    let dir = temp_dir("ts-roundtrip");
    let master = MasterKey::generate();
    let ts_key = derive_team_session_key(&master);

    let store = TeamSessionStore::open(dir.join("ts.db")).unwrap();

    let mut specialist_convos = HashMap::new();
    specialist_convos.insert(
        "researcher".to_string(),
        vec![
            ChatMessage::user("find info".to_string()),
            ChatMessage::assistant("here are the results".to_string()),
        ],
    );
    specialist_convos.insert(
        "coder".to_string(),
        vec![
            ChatMessage::user("implement X".to_string()),
            ChatMessage::assistant("done, here's the code".to_string()),
        ],
    );

    let session = PersistedTeamSession {
        session_id: "sess-001".to_string(),
        team_name: "dev-team".to_string(),
        lead_conversation: vec![
            ChatMessage::user("build feature X".to_string()),
            ChatMessage::assistant("I'll coordinate the team".to_string()),
        ],
        specialist_conversations: specialist_convos,
        completed_work: vec![
            "Researched topic X".to_string(),
            "Implemented module Y".to_string(),
        ],
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
    };

    store.save(&session, &ts_key).unwrap();
    let loaded = store
        .load("dev-team", "sess-001", &ts_key)
        .unwrap()
        .unwrap();

    assert_eq!(loaded.session_id, "sess-001");
    assert_eq!(loaded.team_name, "dev-team");
    assert_eq!(loaded.lead_conversation.len(), 2);
    assert_eq!(loaded.specialist_conversations.len(), 2);
    assert_eq!(loaded.specialist_conversations["researcher"].len(), 2);
    assert_eq!(loaded.specialist_conversations["coder"].len(), 2);
    assert_eq!(loaded.completed_work.len(), 2);
    assert_eq!(loaded.completed_work[0], "Researched topic X");

    std::fs::remove_dir_all(&dir).ok();
}

/// Test 2: List and delete across multiple teams.
#[test]
fn team_session_list_and_delete() {
    let dir = temp_dir("ts-list-delete");
    let master = MasterKey::generate();
    let ts_key = derive_team_session_key(&master);
    let store = TeamSessionStore::open(dir.join("ts.db")).unwrap();

    // Save 3 sessions across 2 teams
    for (team, sid) in [("alpha", "s1"), ("alpha", "s2"), ("beta", "s3")] {
        let session = PersistedTeamSession {
            session_id: sid.to_string(),
            team_name: team.to_string(),
            lead_conversation: vec![ChatMessage::user("test".to_string())],
            specialist_conversations: HashMap::new(),
            completed_work: Vec::new(),
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        };
        store.save(&session, &ts_key).unwrap();
    }

    let alpha_list = store.list("alpha", &ts_key).unwrap();
    assert_eq!(alpha_list.len(), 2);

    let beta_list = store.list("beta", &ts_key).unwrap();
    assert_eq!(beta_list.len(), 1);
    assert_eq!(beta_list[0].session_id, "s3");

    // Delete one alpha session
    store.delete("alpha", "s1").unwrap();
    let alpha_list = store.list("alpha", &ts_key).unwrap();
    assert_eq!(alpha_list.len(), 1);
    assert_eq!(alpha_list[0].session_id, "s2");

    // Verify the deleted one is gone
    assert!(store.load("alpha", "s1", &ts_key).unwrap().is_none());

    std::fs::remove_dir_all(&dir).ok();
}

/// Test 3: Metrics summary handles new TeamSessionSaved/Resumed events gracefully.
#[test]
fn metrics_summary_with_team_events() {
    let now = chrono::Utc::now();
    let ts = now.to_rfc3339();

    let entries = vec![
        AuditEntry {
            sequence_number: 0,
            timestamp: ts.clone(),
            event: AuditEvent::TeamSessionSaved {
                team_name: "dev-team".into(),
                session_id: "s-1".into(),
            },
            hmac: String::new(),
        },
        AuditEntry {
            sequence_number: 1,
            timestamp: ts.clone(),
            event: AuditEvent::TeamSessionResumed {
                team_name: "dev-team".into(),
                session_id: "s-1".into(),
            },
            hmac: String::new(),
        },
        AuditEntry {
            sequence_number: 2,
            timestamp: ts.clone(),
            event: AuditEvent::LlmResponseReceived {
                agent_id: aivyx_core::AgentId::new(),
                provider: "claude".into(),
                input_tokens: 100,
                output_tokens: 50,
                stop_reason: "end_turn".into(),
            },
            hmac: String::new(),
        },
    ];

    let from = now - chrono::Duration::hours(1);
    let cost_fn = |input: u32, output: u32, _provider: &str| -> f64 {
        (input as f64 * 0.001) + (output as f64 * 0.002)
    };
    let summary = compute_summary(&entries, from, now + chrono::Duration::hours(1), &cost_fn);

    // Team session events pass through gracefully. LLM event is counted.
    assert_eq!(summary.llm_requests, 1);
    assert_eq!(summary.total_input_tokens, 100);
    assert_eq!(summary.total_output_tokens, 50);
}

/// Test 4: CapabilityScope::is_subset_of cross-crate verification.
#[test]
fn capability_scope_subset_rules() {
    use std::path::PathBuf;

    let fs_root = CapabilityScope::Filesystem {
        root: PathBuf::from("/home"),
    };
    let fs_sub = CapabilityScope::Filesystem {
        root: PathBuf::from("/home/user"),
    };
    let net = CapabilityScope::Network {
        hosts: vec!["example.com".into()],
        ports: vec![443],
    };

    // Filesystem subtree is subset of broader root
    assert!(fs_sub.is_subset_of(&fs_root));

    // Broader is NOT subset of narrower
    assert!(!fs_root.is_subset_of(&fs_sub));

    // Custom("memory") is subset of Custom("memory")
    assert!(
        CapabilityScope::Custom("memory".into())
            .is_subset_of(&CapabilityScope::Custom("memory".into()))
    );

    // Custom("memory") is NOT subset of Custom("network")
    assert!(
        !CapabilityScope::Custom("memory".into())
            .is_subset_of(&CapabilityScope::Custom("network".into()))
    );

    // Network is NOT subset of Filesystem (different variants)
    assert!(!net.is_subset_of(&fs_root));

    // Custom("mcp:tool") is subset of Custom("mcp:*") via prefix wildcard
    assert!(
        CapabilityScope::Custom("mcp:tool".into())
            .is_subset_of(&CapabilityScope::Custom("mcp:*".into()))
    );

    // Calendar is subset of Calendar
    assert!(CapabilityScope::Calendar.is_subset_of(&CapabilityScope::Calendar));

    // Calendar is NOT subset of Filesystem
    assert!(!CapabilityScope::Calendar.is_subset_of(&fs_root));
}

/// Test 5: Agent session persistence with conversation history.
#[test]
fn session_persistence_with_messages() {
    let dir = temp_dir("agent-session");
    let master = MasterKey::generate();

    let session_store = aivyx_agent::SessionStore::open(dir.join("sessions.db")).unwrap();

    let session_id = aivyx_core::SessionId::new();
    let persisted = aivyx_agent::session_store::PersistedSession {
        metadata: aivyx_agent::session_store::SessionMetadata {
            session_id,
            agent_name: "test-agent".to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            message_count: 4,
        },
        messages: vec![
            ChatMessage::user("hello".to_string()),
            ChatMessage::assistant("hi there".to_string()),
            ChatMessage::user("project context: my-app".to_string()),
            ChatMessage::assistant("I see the project".to_string()),
        ],
    };

    session_store.save(&persisted, &master).unwrap();
    let loaded = session_store
        .load(&session_id, &master, 0)
        .unwrap()
        .unwrap();

    assert_eq!(loaded.messages.len(), 4);
    assert_eq!(loaded.messages[2].content, "project context: my-app");
    assert_eq!(loaded.metadata.agent_name, "test-agent");

    std::fs::remove_dir_all(&dir).ok();
}

/// Test 6: Concurrent writes to TeamSessionStore.
#[tokio::test]
async fn concurrent_session_store_access() {
    let dir = temp_dir("ts-concurrent");
    let master = MasterKey::generate();
    let ts_key = derive_team_session_key(&master);
    let store = std::sync::Arc::new(TeamSessionStore::open(dir.join("ts.db")).unwrap());

    let mut handles = Vec::new();

    for i in 0..5 {
        let store = store.clone();
        let key_bytes: [u8; 32] = ts_key.expose_secret()[..32].try_into().unwrap();
        let key = MasterKey::from_bytes(key_bytes);

        handles.push(tokio::spawn(async move {
            let session = PersistedTeamSession {
                session_id: format!("concurrent-{i}"),
                team_name: "stress-team".to_string(),
                lead_conversation: vec![ChatMessage::user(format!("msg from {i}"))],
                specialist_conversations: HashMap::new(),
                completed_work: Vec::new(),
                created_at: chrono::Utc::now().to_rfc3339(),
                updated_at: chrono::Utc::now().to_rfc3339(),
            };
            store.save(&session, &key).unwrap();
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    // All 5 sessions should be saved
    let key_bytes: [u8; 32] = ts_key.expose_secret()[..32].try_into().unwrap();
    let verify_key = MasterKey::from_bytes(key_bytes);
    let list = store.list("stress-team", &verify_key).unwrap();
    assert_eq!(list.len(), 5);

    std::fs::remove_dir_all(&dir).ok();
}

/// Test 7: Skill validate on generated content.
#[test]
fn skill_validate_on_valid_content() {
    let content = r#"---
name: test-skill
description: A test skill for integration testing
allowed-tools: "file_read grep"
metadata:
  author: test
  version: "1.0"
  license: MIT
---

# Test Skill

## Instructions

This skill tests file operations.
"#;

    let report = aivyx_config::skill::validate_full(content);
    assert!(
        report.errors.is_empty(),
        "Expected no errors, got: {:?}",
        report.errors
    );
}

/// Test 7b: Skill validate catches errors on missing body.
#[test]
fn skill_validate_catches_missing_body() {
    let content = r#"---
name: empty-skill
description: A skill with no body
---
"#;

    let report = aivyx_config::skill::validate_full(content);
    assert!(
        !report.errors.is_empty(),
        "Expected errors for missing body"
    );
}

/// Test 8: Key derivation produces distinct keys across all subsystems.
#[test]
fn all_derived_keys_are_distinct() {
    let master = MasterKey::generate();

    let audit = aivyx_crypto::derive_audit_key(&master);
    let memory = aivyx_crypto::derive_memory_key(&master);
    let task = aivyx_crypto::derive_task_key(&master);
    let schedule = aivyx_crypto::derive_schedule_key(&master);
    let team_session = derive_team_session_key(&master);

    // All should be distinct from each other
    assert_ne!(audit.as_slice(), memory.expose_secret());
    assert_ne!(audit.as_slice(), task.expose_secret());
    assert_ne!(audit.as_slice(), schedule.expose_secret());
    assert_ne!(audit.as_slice(), team_session.expose_secret());
    assert_ne!(memory.expose_secret(), task.expose_secret());
    assert_ne!(memory.expose_secret(), schedule.expose_secret());
    assert_ne!(memory.expose_secret(), team_session.expose_secret());
    assert_ne!(task.expose_secret(), schedule.expose_secret());
    assert_ne!(task.expose_secret(), team_session.expose_secret());
    assert_ne!(schedule.expose_secret(), team_session.expose_secret());
}

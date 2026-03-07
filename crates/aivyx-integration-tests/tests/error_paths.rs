//! Integration tests for error paths, data isolation, and config roundtrips.

use std::path::PathBuf;

use aivyx_agent::{AgentSession, SessionStore};
use aivyx_audit::{AuditEvent, AuditLog};
use aivyx_capability::CapabilitySet;
use aivyx_config::{
    AivyxConfig, AivyxDirs, AutonomyPolicy, EmbeddingConfig, MemoryConfig, ProviderConfig,
    ServerConfig,
};
use aivyx_core::{AgentId, AutonomyTier, CapabilityScope, ToolId, ToolRegistry};
use aivyx_crypto::{EncryptedStore, MasterKey, derive_audit_key};
use aivyx_integration_tests::{MockProvider, create_test_agent};
use aivyx_task::store::TaskStore;
use aivyx_task::types::{Mission, Step, StepStatus, TaskStatus};
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
// 1. AgentSession with missing profile returns an error
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn agent_session_missing_profile_errors() {
    let dir = temp_dir("missing-profile");
    std::fs::create_dir_all(dir.join("agents")).unwrap();
    std::fs::create_dir_all(dir.join("keys")).unwrap();

    let dirs = AivyxDirs::new(&dir);
    let config = AivyxConfig::default();
    let master_key = MasterKey::from_bytes([42u8; 32]);

    let session = AgentSession::new(dirs, config, master_key);
    let result = session.create_agent("nonexistent").await;

    assert!(
        result.is_err(),
        "should error on missing profile, not panic"
    );
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("expected error"),
    };
    let err_msg = format!("{err}");
    assert!(
        err_msg.contains("nonexistent"),
        "error should mention the missing profile name"
    );

    cleanup(&dir);
}

// ────────────────────────────────────────────────────────────────────
// 2. SessionStore save/load/delete lifecycle
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn session_store_save_load_delete_lifecycle() {
    let dir = temp_dir("session-lifecycle");
    let key = MasterKey::from_bytes([10u8; 32]);

    // Build a PersistedSession from an agent
    let provider = MockProvider::simple("hello");
    let mut agent = create_test_agent(
        Box::new(provider),
        ToolRegistry::new(),
        CapabilitySet::new(),
        AutonomyTier::Trust,
    );
    agent.turn("hi there", None).await.unwrap();

    let persisted = agent.to_persisted_session();
    let session_id = persisted.metadata.session_id;

    // Save
    let store = SessionStore::open(dir.join("sessions.db")).unwrap();
    store.save(&persisted, &key).unwrap();

    // Load back and verify fields
    let loaded = store.load(&session_id, &key, 0).unwrap();
    assert!(loaded.is_some(), "saved session should be loadable");
    let loaded = loaded.unwrap();
    assert_eq!(loaded.metadata.session_id, session_id);
    assert_eq!(loaded.messages.len(), persisted.messages.len());
    assert_eq!(loaded.messages[0].content, "hi there");

    // Delete and verify gone
    store.delete(&session_id).unwrap();
    let after_delete = store.load(&session_id, &key, 0).unwrap();
    assert!(after_delete.is_none(), "deleted session should return None");

    cleanup(&dir);
}

// ────────────────────────────────────────────────────────────────────
// 3. EncryptedStore key isolation
// ────────────────────────────────────────────────────────────────────

#[test]
fn encrypted_store_key_isolation() {
    let dir = temp_dir("key-isolation");
    let store = EncryptedStore::open(dir.join("store.db")).unwrap();

    let key1 = MasterKey::from_bytes([1u8; 32]);
    let key2 = MasterKey::from_bytes([2u8; 32]);

    let plaintext = b"super secret data";
    store.put("mykey", plaintext, &key1).unwrap();

    // Same key reads back correctly
    let read = store.get("mykey", &key1).unwrap();
    assert!(read.is_some());
    assert_eq!(read.unwrap(), plaintext);

    // Different key should fail decryption
    let result = store.get("mykey", &key2);
    assert!(
        result.is_err(),
        "reading with wrong key should fail with decryption error"
    );

    cleanup(&dir);
}

// ────────────────────────────────────────────────────────────────────
// 4. Audit chain integrity with mixed events + tamper detection
// ────────────────────────────────────────────────────────────────────

#[test]
fn audit_chain_integrity_mixed_events() {
    let dir = temp_dir("audit-chain");
    let audit_key = derive_audit_key(&MasterKey::from_bytes([50u8; 32]));
    let log = AuditLog::new(dir.join("audit.log"), &audit_key);

    // Append 5 different event types
    let events: Vec<AuditEvent> = vec![
        AuditEvent::AgentCreated {
            agent_id: AgentId::new(),
            autonomy_tier: AutonomyTier::Trust,
        },
        AuditEvent::ToolExecuted {
            tool_id: ToolId::new(),
            agent_id: AgentId::new(),
            action: "file_read".into(),
            result_summary: "ok".into(),
        },
        AuditEvent::ToolDenied {
            tool_id: ToolId::new(),
            agent_id: AgentId::new(),
            action: "shell".into(),
            reason: "no capability".into(),
        },
        AuditEvent::HttpRequestReceived {
            method: "POST".into(),
            path: "/chat".into(),
            remote_addr: "127.0.0.1:9000".into(),
        },
        AuditEvent::SystemInit {
            timestamp: Utc::now(),
        },
    ];

    for event in events {
        log.append(event).unwrap();
    }

    // Verify chain
    let result = log.verify().unwrap();
    assert!(result.valid, "audit chain should be valid");
    assert_eq!(result.entries_checked, 5);

    // Tamper with the log file and verify chain detects it
    let log_path = dir.join("audit.log");
    let contents = std::fs::read_to_string(&log_path).unwrap();
    let lines: Vec<&str> = contents.lines().collect();
    assert_eq!(lines.len(), 5);

    // Modify the 3rd line (tamper with the event data)
    let mut tampered_lines: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    tampered_lines[2] = tampered_lines[2].replace("shell", "TAMPERED");
    let tampered = tampered_lines.join("\n") + "\n";
    std::fs::write(&log_path, tampered).unwrap();

    // Re-open and verify — should detect tampering
    let log2 = AuditLog::new(dir.join("audit.log"), &audit_key);
    let result2 = log2.verify().unwrap();
    assert!(
        !result2.valid,
        "tampered audit chain should be detected as invalid"
    );

    cleanup(&dir);
}

// ────────────────────────────────────────────────────────────────────
// 5. Config roundtrip with all sections
// ────────────────────────────────────────────────────────────────────

#[test]
fn config_roundtrip_all_sections() {
    let mut config = AivyxConfig::default();

    // Override provider
    config.provider = ProviderConfig::Ollama {
        base_url: "http://localhost:11434".into(),
        model: "llama3".into(),
    };

    // Set embedding
    config.embedding = Some(EmbeddingConfig::OpenAI {
        api_key_ref: "my-embed-key".into(),
        model: "text-embedding-3-small".into(),
        dimensions: 1536,
    });

    // Memory config
    config.memory = MemoryConfig {
        max_memories: 500,
        profile_extraction_threshold: 10,
        session_max_age_hours: 720,
    };

    // Autonomy policy
    config.autonomy = AutonomyPolicy {
        default_tier: AutonomyTier::Leash,
        max_tool_calls_per_minute: 30,
        max_cost_per_session_usd: 2.0,
        require_approval_for_destructive: true,
        max_retries: 5,
        retry_base_delay_ms: 2000,
    };

    // Server config
    config.server = Some(ServerConfig {
        bind_address: "0.0.0.0".into(),
        port: 8080,
        cors_origins: vec!["https://example.com".into()],
        ..Default::default()
    });

    // Named provider
    config.providers.insert(
        "fast".into(),
        ProviderConfig::Ollama {
            base_url: "http://localhost:11434".into(),
            model: "qwen2.5:7b".into(),
        },
    );

    // Serialize to TOML and back
    let toml_str = toml::to_string(&config).unwrap();
    let restored: AivyxConfig = toml::from_str(&toml_str).unwrap();

    // Verify all fields
    assert!(matches!(restored.provider, ProviderConfig::Ollama { .. }));
    assert_eq!(restored.provider.model_name(), "llama3");

    let embed = restored.embedding.unwrap();
    match embed {
        EmbeddingConfig::OpenAI {
            api_key_ref,
            model,
            dimensions,
        } => {
            assert_eq!(api_key_ref, "my-embed-key");
            assert_eq!(model, "text-embedding-3-small");
            assert_eq!(dimensions, 1536);
        }
        _ => panic!("expected OpenAI embedding config"),
    }

    assert_eq!(restored.memory.max_memories, 500);
    assert_eq!(restored.memory.profile_extraction_threshold, 10);

    assert_eq!(restored.autonomy.default_tier, AutonomyTier::Leash);
    assert_eq!(restored.autonomy.max_tool_calls_per_minute, 30);
    assert!((restored.autonomy.max_cost_per_session_usd - 2.0).abs() < f64::EPSILON);
    assert!(restored.autonomy.require_approval_for_destructive);
    assert_eq!(restored.autonomy.max_retries, 5);
    assert_eq!(restored.autonomy.retry_base_delay_ms, 2000);

    let server = restored.server.unwrap();
    assert_eq!(server.bind_address, "0.0.0.0");
    assert_eq!(server.port, 8080);
    assert_eq!(server.cors_origins, vec!["https://example.com"]);

    assert_eq!(restored.providers.len(), 1);
    assert_eq!(
        restored.providers.get("fast").unwrap().model_name(),
        "qwen2.5:7b"
    );
}

// ────────────────────────────────────────────────────────────────────
// 6. Cross-variant attenuation fails
// ────────────────────────────────────────────────────────────────────

#[test]
fn capability_attenuation_cross_variant_fails() {
    let fs = CapabilityScope::Filesystem {
        root: PathBuf::from("/home"),
    };
    let shell = CapabilityScope::Shell {
        allowed_commands: vec!["ls".into()],
    };

    let result = fs.attenuate(&shell);
    assert!(
        result.is_none(),
        "cross-variant attenuation (Filesystem -> Shell) should return None"
    );

    // Also verify the reverse
    let result2 = shell.attenuate(&fs);
    assert!(
        result2.is_none(),
        "cross-variant attenuation (Shell -> Filesystem) should return None"
    );
}

// ────────────────────────────────────────────────────────────────────
// 7. Narrowing attenuation succeeds
// ────────────────────────────────────────────────────────────────────

#[test]
fn capability_attenuation_narrowing_works() {
    let parent = CapabilityScope::Filesystem {
        root: PathBuf::from("/"),
    };
    let child = CapabilityScope::Filesystem {
        root: PathBuf::from("/home"),
    };

    let result = parent.attenuate(&child);
    assert!(result.is_some(), "narrowing / -> /home should succeed");

    let narrowed = result.unwrap();
    match narrowed {
        CapabilityScope::Filesystem { root } => {
            assert_eq!(root, PathBuf::from("/home"));
        }
        _ => panic!("expected Filesystem variant after attenuation"),
    }
}

// ────────────────────────────────────────────────────────────────────
// 8. TaskStore data isolation across different db files
// ────────────────────────────────────────────────────────────────────

#[test]
fn task_store_data_isolation() {
    let dir = temp_dir("task-store-iso");
    let key = MasterKey::from_bytes([77u8; 32]);

    let store1 = TaskStore::open(dir.join("tasks1.db")).unwrap();
    let store2 = TaskStore::open(dir.join("tasks2.db")).unwrap();

    // Save a mission in store1
    let mut mission = Mission::new("goal for store1", "agent");
    mission.status = TaskStatus::Planned;
    mission.steps = vec![Step {
        index: 0,
        description: "step 0".into(),
        tool_hints: vec![],
        status: StepStatus::Pending,
        prompt: None,
        result: None,
        retries: 0,
        started_at: None,
        completed_at: None,
    }];
    store1.save(&mission, &key).unwrap();

    // store1 can load it
    let loaded = store1.load(&mission.id, &key).unwrap();
    assert!(loaded.is_some());
    assert_eq!(loaded.unwrap().goal, "goal for store1");

    // store2 cannot find it
    let not_found = store2.load(&mission.id, &key).unwrap();
    assert!(
        not_found.is_none(),
        "store2 should not contain missions saved in store1"
    );

    // store2 list should be empty
    let list2 = store2.list(&key).unwrap();
    assert!(list2.is_empty());

    // store1 list should have 1
    let list1 = store1.list(&key).unwrap();
    assert_eq!(list1.len(), 1);

    cleanup(&dir);
}

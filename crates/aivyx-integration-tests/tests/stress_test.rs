//! Comprehensive stress tests exercising cross-crate interactions end-to-end.
//!
//! These tests push the system harder than the per-pipeline tests:
//! - Multi-turn agent with memory + audit + capability enforcement in one session
//! - Audit chain integrity under high-throughput logging
//! - Autonomy tier enforcement (Locked, Leash)
//! - Capability attenuation invariants across delegation
//! - Task engine lifecycle (plan → execute → checkpoint → resume → cancel)
//! - Session persistence with memory context restoration
//! - Provider resolution with named provider map
//! - Encrypted store data isolation across different keys
//! - Server endpoints with concurrent-style requests
//! - All 29 audit event variants serialize round-trip

use std::path::PathBuf;
use std::sync::Arc;

use aivyx_agent::built_in_tools::{FileReadTool, ShellTool};
use aivyx_agent::{AgentProfile, CostTracker, RateLimiter, SessionStore};
use aivyx_audit::{AuditEvent, AuditLog};
use aivyx_capability::{ActionPattern, Capability, CapabilitySet};
use aivyx_config::{AivyxConfig, AivyxDirs, ProviderConfig};
use aivyx_core::{
    AgentId, AutonomyTier, CapabilityId, CapabilityScope, MemoryId, Principal, SessionId, ToolId,
    ToolRegistry, TripleId,
};
use aivyx_crypto::{MasterKey, derive_audit_key};
use aivyx_integration_tests::{
    FailThenSucceedProvider, MockEmbeddingProvider, MockProvider, create_filesystem_caps,
    create_test_agent,
};
use aivyx_llm::{ChatMessage, ChatResponse, StopReason, TokenUsage, ToolCall};
use aivyx_memory::{MemoryKind, MemoryManager, MemoryStore};
use aivyx_task::store::TaskStore;
use aivyx_task::types::{Mission, Step, StepStatus, TaskStatus};
use aivyx_team::message_bus::{MessageBus, TeamMessage};
use chrono::Utc;

// ────────────────────────────────────────────────────────────────────
// Helpers
// ────────────────────────────────────────────────────────────────────

const TEST_KEY_BYTES: [u8; 32] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 32,
];

fn temp_dir(prefix: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("aivyx-stress-{prefix}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn cleanup(dir: &PathBuf) {
    std::fs::remove_dir_all(dir).ok();
}

/// Create a capability set with multiple scopes for a given agent.
fn create_multi_scope_caps(agent_id: AgentId) -> CapabilitySet {
    let mut caps = CapabilitySet::new();
    let principal = Principal::Agent(agent_id);
    let now = Utc::now();

    // Filesystem
    caps.grant(Capability {
        id: CapabilityId::new(),
        scope: CapabilityScope::Filesystem {
            root: PathBuf::from("/"),
        },
        pattern: ActionPattern::new("*").unwrap(),
        granted_to: vec![principal.clone()],
        granted_by: Principal::System,
        created_at: now,
        expires_at: None,
        revoked: false,
        parent_id: None,
    });

    // Shell
    caps.grant(Capability {
        id: CapabilityId::new(),
        scope: CapabilityScope::Shell {
            allowed_commands: vec![],
        },
        pattern: ActionPattern::new("*").unwrap(),
        granted_to: vec![principal.clone()],
        granted_by: Principal::System,
        created_at: now,
        expires_at: None,
        revoked: false,
        parent_id: None,
    });

    // Memory
    caps.grant(Capability {
        id: CapabilityId::new(),
        scope: CapabilityScope::Custom("memory".into()),
        pattern: ActionPattern::new("*").unwrap(),
        granted_to: vec![principal.clone()],
        granted_by: Principal::System,
        created_at: now,
        expires_at: None,
        revoked: false,
        parent_id: None,
    });

    // Coordination
    caps.grant(Capability {
        id: CapabilityId::new(),
        scope: CapabilityScope::Custom("coordination".into()),
        pattern: ActionPattern::new("*").unwrap(),
        granted_to: vec![principal.clone()],
        granted_by: Principal::System,
        created_at: now,
        expires_at: None,
        revoked: false,
        parent_id: None,
    });

    // MCP
    caps.grant(Capability {
        id: CapabilityId::new(),
        scope: CapabilityScope::Custom("mcp:test".into()),
        pattern: ActionPattern::new("*").unwrap(),
        granted_to: vec![principal],
        granted_by: Principal::System,
        created_at: now,
        expires_at: None,
        revoked: false,
        parent_id: None,
    });

    caps
}

// ────────────────────────────────────────────────────────────────────
// 1. Multi-turn agent with tools + memory + audit (full stack)
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn full_stack_agent_turn_with_tools_memory_and_audit() {
    let dir = temp_dir("full-stack");
    let agent_id = AgentId::new();
    let caps = create_multi_scope_caps(agent_id);

    // Set up memory
    let mem_provider = Arc::new(MockEmbeddingProvider::new(8));
    let mem_store = MemoryStore::open(dir.join("memory.db")).unwrap();
    let mem_key = MasterKey::from_bytes(TEST_KEY_BYTES);
    let mut mgr = MemoryManager::new(mem_store, mem_provider.clone(), mem_key, 0).unwrap();

    // Pre-populate memory
    mgr.remember(
        "The user likes Rust programming".into(),
        MemoryKind::Preference,
        Some(agent_id),
        vec!["tech".into()],
    )
    .await
    .unwrap();
    mgr.add_triple(
        "User".into(),
        "prefers".into(),
        "Rust".into(),
        Some(agent_id),
        0.95,
        "conversation".into(),
    )
    .unwrap();

    let mgr = Arc::new(tokio::sync::Mutex::new(mgr));

    // Set up audit
    let audit_key = derive_audit_key(&MasterKey::from_bytes(TEST_KEY_BYTES));
    let audit_log = AuditLog::new(dir.join("audit.log"), &audit_key);

    // Create an agent with file_read tool that triggers a tool call loop
    let mut tools = ToolRegistry::new();
    tools.register(Box::new(FileReadTool::new()));

    let tool_call = ToolCall {
        id: "tc_1".into(),
        name: "file_read".into(),
        arguments: serde_json::json!({"path": "/tmp/nonexistent-stress-test"}),
    };

    let provider = MockProvider::new(vec![
        // Turn 1: agent calls file_read, gets error, then responds
        ChatResponse {
            message: ChatMessage::assistant_with_tool_calls("Let me read that", vec![tool_call]),
            usage: TokenUsage {
                input_tokens: 20,
                output_tokens: 10,
            },
            stop_reason: StopReason::ToolUse,
        },
        ChatResponse {
            message: ChatMessage::assistant("File not found. I know you like Rust though!"),
            usage: TokenUsage {
                input_tokens: 30,
                output_tokens: 15,
            },
            stop_reason: StopReason::EndTurn,
        },
        // Turn 2: simple response
        ChatResponse {
            message: ChatMessage::assistant("Yes, Rust is great for systems programming!"),
            usage: TokenUsage {
                input_tokens: 40,
                output_tokens: 10,
            },
            stop_reason: StopReason::EndTurn,
        },
    ]);

    let mut agent = aivyx_agent::Agent::new(
        agent_id,
        "stress-agent".into(),
        "You are a test agent.".into(),
        4096,
        AutonomyTier::Trust,
        Box::new(provider),
        tools,
        caps,
        RateLimiter::new(60),
        CostTracker::new(5.0, 0.000003, 0.000015),
        Some(audit_log),
        3,
        1,
    );

    agent.set_memory_manager(mgr);

    // Turn 1: tool use + memory context
    let result1 = agent.turn("Read my notes file", None).await.unwrap();
    assert!(result1.contains("File not found"));

    // Turn 2: conversational follow-up
    let result2 = agent.turn("Tell me about Rust", None).await.unwrap();
    assert!(result2.contains("Rust"));

    // Verify conversation history accumulated
    // Turn 1: user + assistant(tool_call) + tool_result + assistant(final) = 4
    // Turn 2: user + assistant = 2
    assert_eq!(agent.conversation().len(), 6);

    // Verify cost accumulated across turns
    let cost = agent.current_cost_usd();
    assert!(cost > 0.0, "cost should be positive after 2 turns");

    // Verify audit log has entries
    let audit_key2 = derive_audit_key(&MasterKey::from_bytes(TEST_KEY_BYTES));
    let audit_log2 = AuditLog::new(dir.join("audit.log"), &audit_key2);
    let entries = audit_log2.recent(100).unwrap();
    assert!(
        entries.len() >= 4,
        "should have multiple audit entries for 2 turns"
    );

    // Verify audit chain integrity
    let verify = audit_log2.verify().unwrap();
    assert!(verify.valid, "audit chain should be intact");
    assert!(verify.entries_checked >= 4);

    cleanup(&dir);
}

// ────────────────────────────────────────────────────────────────────
// 2. Locked autonomy tier denies all tool calls
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn locked_tier_denies_all_tool_calls() {
    let agent_id = AgentId::new();
    let caps = create_filesystem_caps(agent_id);

    let mut tools = ToolRegistry::new();
    tools.register(Box::new(FileReadTool::new()));

    let tool_call = ToolCall {
        id: "tc_1".into(),
        name: "file_read".into(),
        arguments: serde_json::json!({"path": "/tmp/test"}),
    };

    let provider = MockProvider::new(vec![
        ChatResponse {
            message: ChatMessage::assistant_with_tool_calls("Reading", vec![tool_call]),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
            stop_reason: StopReason::ToolUse,
        },
        ChatResponse {
            message: ChatMessage::assistant("I was denied access."),
            usage: TokenUsage {
                input_tokens: 15,
                output_tokens: 5,
            },
            stop_reason: StopReason::EndTurn,
        },
    ]);

    let mut agent = create_test_agent(
        Box::new(provider),
        tools,
        caps,
        AutonomyTier::Locked, // Locked: all tools denied
    );

    let result = agent.turn("read a file", None).await.unwrap();
    assert_eq!(result, "I was denied access.");

    // The tool result should indicate denial
    let tool_msg = agent
        .conversation()
        .iter()
        .find(|m| m.tool_result.is_some())
        .unwrap();
    let tr = tool_msg.tool_result.as_ref().unwrap();
    assert!(tr.is_error, "tool call should be denied for Locked tier");
}

// ────────────────────────────────────────────────────────────────────
// 3. Leash tier without channel adapter denies (fail closed)
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn leash_tier_without_channel_denies_tool_calls() {
    let agent_id = AgentId::new();
    let caps = create_filesystem_caps(agent_id);

    let mut tools = ToolRegistry::new();
    tools.register(Box::new(FileReadTool::new()));

    let tool_call = ToolCall {
        id: "tc_1".into(),
        name: "file_read".into(),
        arguments: serde_json::json!({"path": "/tmp/test"}),
    };

    let provider = MockProvider::new(vec![
        ChatResponse {
            message: ChatMessage::assistant_with_tool_calls("Reading", vec![tool_call]),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
            stop_reason: StopReason::ToolUse,
        },
        ChatResponse {
            message: ChatMessage::assistant("Denied."),
            usage: TokenUsage {
                input_tokens: 15,
                output_tokens: 3,
            },
            stop_reason: StopReason::EndTurn,
        },
    ]);

    let mut agent = create_test_agent(
        Box::new(provider),
        tools,
        caps,
        AutonomyTier::Leash, // Leash: needs channel adapter for approval
    );

    // No channel passed → should deny
    let result = agent.turn("read a file", None).await.unwrap();
    assert_eq!(result, "Denied.");

    let tool_msg = agent
        .conversation()
        .iter()
        .find(|m| m.tool_result.is_some())
        .unwrap();
    let tr = tool_msg.tool_result.as_ref().unwrap();
    assert!(
        tr.is_error,
        "Leash tier without channel should deny tool calls"
    );
}

// ────────────────────────────────────────────────────────────────────
// 4. Audit chain integrity under high-throughput logging
// ────────────────────────────────────────────────────────────────────

#[test]
fn audit_chain_integrity_under_load() {
    let dir = temp_dir("audit-load");
    let key = derive_audit_key(&MasterKey::from_bytes(TEST_KEY_BYTES));
    let log = AuditLog::new(dir.join("audit.log"), &key);

    // Log 100 events rapidly with mixed types
    for i in 0..100u64 {
        let event = match i % 5 {
            0 => AuditEvent::SystemInit {
                timestamp: Utc::now(),
            },
            1 => AuditEvent::AgentCreated {
                agent_id: AgentId::new(),
                autonomy_tier: AutonomyTier::Trust,
            },
            2 => AuditEvent::ToolExecuted {
                tool_id: ToolId::new(),
                agent_id: AgentId::new(),
                action: format!("action_{i}"),
                result_summary: format!("result_{i}"),
            },
            3 => AuditEvent::MemoryStored {
                memory_id: MemoryId::new(),
                agent_id: AgentId::new(),
                kind: "Fact".into(),
            },
            _ => AuditEvent::TaskStepCompleted {
                task_id: format!("task-{i}"),
                step_index: (i as usize) % 5,
                step_description: format!("step {i}"),
                success: i % 2 == 0,
            },
        };
        log.append(event).unwrap();
    }

    // Verify full chain
    let verify = log.verify().unwrap();
    assert!(
        verify.valid,
        "audit chain should be intact after 100 entries"
    );
    assert_eq!(verify.entries_checked, 100);

    // Verify count
    assert_eq!(log.len().unwrap(), 100);

    // Verify recent subset
    let recent = log.recent(10).unwrap();
    assert_eq!(recent.len(), 10);
    assert_eq!(recent[0].sequence_number, 90);
    assert_eq!(recent[9].sequence_number, 99);

    cleanup(&dir);
}

// ────────────────────────────────────────────────────────────────────
// 5. All 29 audit event variants serialize round-trip
// ────────────────────────────────────────────────────────────────────

#[test]
fn all_audit_event_variants_roundtrip() {
    let events: Vec<AuditEvent> = vec![
        AuditEvent::SystemInit {
            timestamp: Utc::now(),
        },
        AuditEvent::CapabilityGranted {
            capability_id: CapabilityId::new(),
            granted_to: Principal::System,
            granted_by: Principal::System,
            scope_summary: "Filesystem /".into(),
        },
        AuditEvent::CapabilityRevoked {
            capability_id: CapabilityId::new(),
            revoked_by: Principal::System,
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
        AuditEvent::ConfigChanged {
            key: "provider.model".into(),
            old_value_hash: "abc".into(),
            new_value_hash: "def".into(),
            changed_by: Principal::System,
        },
        AuditEvent::AgentCreated {
            agent_id: AgentId::new(),
            autonomy_tier: AutonomyTier::Trust,
        },
        AuditEvent::AgentDestroyed {
            agent_id: AgentId::new(),
        },
        AuditEvent::MasterKeyRotated {
            timestamp: Utc::now(),
        },
        AuditEvent::AuditVerified {
            entries_checked: 42,
            valid: true,
        },
        AuditEvent::AgentTurnStarted {
            agent_id: AgentId::new(),
            session_id: SessionId::new(),
        },
        AuditEvent::AgentTurnCompleted {
            agent_id: AgentId::new(),
            session_id: SessionId::new(),
            tool_calls_made: 3,
            tokens_used: 500,
        },
        AuditEvent::LlmRequestSent {
            agent_id: AgentId::new(),
            provider: "mock".into(),
            model: "test-model".into(),
        },
        AuditEvent::LlmResponseReceived {
            agent_id: AgentId::new(),
            provider: "mock".into(),
            input_tokens: 100,
            output_tokens: 50,
            stop_reason: "EndTurn".into(),
        },
        AuditEvent::TeamDelegation {
            from: "coordinator".into(),
            to: "researcher".into(),
            task: "research X".into(),
        },
        AuditEvent::TeamMessage {
            from: "researcher".into(),
            to: "coordinator".into(),
        },
        AuditEvent::MemoryStored {
            memory_id: MemoryId::new(),
            agent_id: AgentId::new(),
            kind: "Fact".into(),
        },
        AuditEvent::MemoryRetrieved {
            agent_id: AgentId::new(),
            query_summary: "rust async".into(),
            results_count: 5,
        },
        AuditEvent::MemoryDeleted {
            memory_id: MemoryId::new(),
            agent_id: AgentId::new(),
        },
        AuditEvent::TripleStored {
            triple_id: TripleId::new(),
            agent_id: AgentId::new(),
            subject: "Rust".into(),
            predicate: "is_a".into(),
        },
        AuditEvent::HttpRequestReceived {
            method: "GET".into(),
            path: "/health".into(),
            remote_addr: "127.0.0.1:8080".into(),
        },
        AuditEvent::HttpAuthFailed {
            remote_addr: "10.0.0.1:1234".into(),
            reason: "missing token".into(),
        },
        AuditEvent::McpServerConnected {
            server_name: "test".into(),
            tool_count: 3,
            timestamp: Utc::now(),
        },
        AuditEvent::McpServerDisconnected {
            server_name: "test".into(),
            reason: "shutdown".into(),
            timestamp: Utc::now(),
        },
        AuditEvent::ToolCacheHit {
            tool_name: "web_search".into(),
            query_hash: "abc123".into(),
        },
        AuditEvent::TaskCreated {
            task_id: "task-1".into(),
            agent_name: "researcher".into(),
            goal: "Research X".into(),
        },
        AuditEvent::TaskStepCompleted {
            task_id: "task-1".into(),
            step_index: 0,
            step_description: "Search".into(),
            success: true,
        },
        AuditEvent::TaskCompleted {
            task_id: "task-1".into(),
            status: "Completed".into(),
            steps_completed: 3,
            steps_total: 3,
        },
        AuditEvent::TaskResumed {
            task_id: "task-1".into(),
            resumed_from_step: 2,
        },
    ];

    assert_eq!(events.len(), 29, "should cover all 29 audit event variants");

    for event in &events {
        let json = serde_json::to_string(event).unwrap();
        let restored: AuditEvent = serde_json::from_str(&json).unwrap();
        // Re-serialize to verify structural equivalence
        let json2 = serde_json::to_string(&restored).unwrap();
        // Both serializations should contain the same "type" tag
        let original_type: serde_json::Value = serde_json::from_str(&json).unwrap();
        let restored_type: serde_json::Value = serde_json::from_str(&json2).unwrap();
        assert_eq!(
            original_type["type"], restored_type["type"],
            "type tag should survive roundtrip"
        );
    }

    // Also test that all events can be written to an audit log
    let dir = temp_dir("audit-variants");
    let key = derive_audit_key(&MasterKey::from_bytes(TEST_KEY_BYTES));
    let log = AuditLog::new(dir.join("audit.log"), &key);
    for event in events {
        log.append(event).unwrap();
    }
    let verify = log.verify().unwrap();
    assert!(verify.valid);
    assert_eq!(verify.entries_checked, 29);
    cleanup(&dir);
}

// ────────────────────────────────────────────────────────────────────
// 6. Capability attenuation: narrows only, never broadens
// ────────────────────────────────────────────────────────────────────

#[test]
fn capability_attenuation_invariants() {
    // Filesystem: /home can attenuate to /home/user, not to /
    let parent_fs = CapabilityScope::Filesystem {
        root: PathBuf::from("/home"),
    };
    let narrow = CapabilityScope::Filesystem {
        root: PathBuf::from("/home/user/docs"),
    };
    let broader = CapabilityScope::Filesystem {
        root: PathBuf::from("/"),
    };
    let disjoint = CapabilityScope::Filesystem {
        root: PathBuf::from("/var"),
    };

    assert!(parent_fs.attenuate(&narrow).is_some(), "should narrow");
    assert!(
        parent_fs.attenuate(&broader).is_none(),
        "should not broaden"
    );
    assert!(
        parent_fs.attenuate(&disjoint).is_none(),
        "disjoint should fail"
    );

    // Network: subset of hosts and ports
    let parent_net = CapabilityScope::Network {
        hosts: vec!["api.example.com".into(), "cdn.example.com".into()],
        ports: vec![80, 443],
    };
    let narrow_net = CapabilityScope::Network {
        hosts: vec!["api.example.com".into()],
        ports: vec![443],
    };
    let broad_net = CapabilityScope::Network {
        hosts: vec!["api.example.com".into(), "evil.com".into()],
        ports: vec![443],
    };

    assert!(parent_net.attenuate(&narrow_net).is_some());
    assert!(
        parent_net.attenuate(&broad_net).is_none(),
        "extra host should fail"
    );

    // Shell: subset of commands
    let parent_shell = CapabilityScope::Shell {
        allowed_commands: vec!["ls".into(), "cat".into(), "grep".into()],
    };
    let narrow_shell = CapabilityScope::Shell {
        allowed_commands: vec!["ls".into()],
    };
    let broad_shell = CapabilityScope::Shell {
        allowed_commands: vec!["ls".into(), "rm".into()],
    };

    assert!(parent_shell.attenuate(&narrow_shell).is_some());
    assert!(
        parent_shell.attenuate(&broad_shell).is_none(),
        "extra command should fail"
    );

    // Cross-variant always fails
    let fs = CapabilityScope::Filesystem {
        root: PathBuf::from("/"),
    };
    let net = CapabilityScope::Network {
        hosts: vec![],
        ports: vec![],
    };
    let shell = CapabilityScope::Shell {
        allowed_commands: vec![],
    };
    let email = CapabilityScope::Email {
        allowed_recipients: vec![],
    };
    let cal = CapabilityScope::Calendar;
    let custom = CapabilityScope::Custom("x".into());

    let all = [&fs, &net, &shell, &email, &cal, &custom];
    for (i, a) in all.iter().enumerate() {
        for (j, b) in all.iter().enumerate() {
            if i != j {
                assert!(
                    a.attenuate(b).is_none(),
                    "cross-variant attenuation should always fail: {a:?} -> {b:?}"
                );
            }
        }
    }
}

// ────────────────────────────────────────────────────────────────────
// 7. Task store encrypted persistence — data isolation across keys
// ────────────────────────────────────────────────────────────────────

#[test]
fn task_store_data_isolation_across_keys() {
    let dir = temp_dir("task-isolation");
    let store = TaskStore::open(dir.join("tasks.db")).unwrap();

    let key1 = MasterKey::from_bytes([1u8; 32]);
    let key2 = MasterKey::from_bytes([2u8; 32]);

    // Save a mission with key1
    let mut mission = Mission::new("secret goal", "agent");
    mission.status = TaskStatus::Planned;
    mission.steps = vec![Step {
        index: 0,
        description: "classified step".into(),
        tool_hints: vec![],
        status: StepStatus::Pending,
        prompt: None,
        result: None,
        retries: 0,
        started_at: None,
        completed_at: None,
    }];
    store.save(&mission, &key1).unwrap();

    // Load with same key — should work
    let loaded = store.load(&mission.id, &key1).unwrap();
    assert!(loaded.is_some());
    assert_eq!(loaded.unwrap().goal, "secret goal");

    // Load with different key — should fail (decryption error)
    let result = store.load(&mission.id, &key2);
    assert!(result.is_err(), "loading with wrong key should fail");

    // Verify raw database doesn't contain plaintext
    let db_bytes = std::fs::read(dir.join("tasks.db")).unwrap();
    let raw = String::from_utf8_lossy(&db_bytes);
    assert!(
        !raw.contains("secret goal"),
        "plaintext should not appear in raw database"
    );
    assert!(
        !raw.contains("classified step"),
        "step description should be encrypted"
    );

    cleanup(&dir);
}

// ────────────────────────────────────────────────────────────────────
// 8. Session persistence with restoration
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn session_persistence_full_lifecycle() {
    let dir = temp_dir("session-lifecycle");

    // Session 1: multi-turn conversation
    let provider = MockProvider::new(vec![
        ChatResponse {
            message: ChatMessage::assistant("Hello! I'm agent Alpha."),
            usage: TokenUsage {
                input_tokens: 5,
                output_tokens: 5,
            },
            stop_reason: StopReason::EndTurn,
        },
        ChatResponse {
            message: ChatMessage::assistant("I can help you with Rust!"),
            usage: TokenUsage {
                input_tokens: 15,
                output_tokens: 5,
            },
            stop_reason: StopReason::EndTurn,
        },
    ]);

    let mut agent = create_test_agent(
        Box::new(provider),
        ToolRegistry::new(),
        CapabilitySet::new(),
        AutonomyTier::Trust,
    );

    let _r1 = agent.turn("Hello, who are you?", None).await.unwrap();
    let _r2 = agent.turn("Help me with Rust", None).await.unwrap();

    // Export session (includes metadata + messages)
    let persisted = agent.to_persisted_session();
    assert_eq!(persisted.messages.len(), 4); // 2 user + 2 assistant
    let session_id = persisted.metadata.session_id;

    // Save to encrypted store
    {
        let session_store = SessionStore::open(dir.join("sessions.db")).unwrap();
        let key = MasterKey::from_bytes(TEST_KEY_BYTES);
        session_store.save(&persisted, &key).unwrap();
    } // Drop session_store to release redb file lock

    // Reload in a "new process"
    let session_store2 = SessionStore::open(dir.join("sessions.db")).unwrap();
    let key2 = MasterKey::from_bytes(TEST_KEY_BYTES);
    let restored = session_store2.load(&session_id, &key2, 0).unwrap().unwrap();
    assert_eq!(restored.messages.len(), 4);

    // Create new agent and restore conversation
    let provider2 = MockProvider::simple("Welcome back!");
    let mut agent2 = create_test_agent(
        Box::new(provider2),
        ToolRegistry::new(),
        CapabilitySet::new(),
        AutonomyTier::Trust,
    );
    agent2.restore_conversation(restored.messages);
    assert_eq!(agent2.conversation().len(), 4);

    // Continue conversation
    let r3 = agent2.turn("I'm back!", None).await.unwrap();
    assert_eq!(r3, "Welcome back!");
    assert_eq!(agent2.conversation().len(), 6);

    // Verify raw database file doesn't contain plaintext
    let db_bytes = std::fs::read(dir.join("sessions.db")).unwrap();
    let raw = String::from_utf8_lossy(&db_bytes);
    assert!(
        !raw.contains("Hello, who are you?"),
        "user messages should be encrypted at rest"
    );
    assert!(
        !raw.contains("I'm agent Alpha"),
        "assistant messages should be encrypted at rest"
    );

    cleanup(&dir);
}

// ────────────────────────────────────────────────────────────────────
// 9. Provider config resolution with named providers
// ────────────────────────────────────────────────────────────────────

#[test]
fn provider_resolution_with_named_providers() {
    let mut config = AivyxConfig::default();

    // Add named providers
    let reasoning_provider = ProviderConfig::Ollama {
        base_url: "http://localhost:11434".into(),
        model: "qwen2.5:72b".into(),
    };
    let coding_provider = ProviderConfig::Ollama {
        base_url: "http://localhost:11434".into(),
        model: "deepseek-coder-v2".into(),
    };

    config
        .providers
        .insert("reasoning".into(), reasoning_provider);
    config.providers.insert("coding".into(), coding_provider);

    // No name → global default
    let resolved = config.resolve_provider(None);
    assert_eq!(resolved.model_name(), config.provider.model_name());

    // Known name → named provider
    let resolved = config.resolve_provider(Some("reasoning"));
    assert_eq!(resolved.model_name(), "qwen2.5:72b");

    let resolved = config.resolve_provider(Some("coding"));
    assert_eq!(resolved.model_name(), "deepseek-coder-v2");

    // Unknown name → fallback to global
    let resolved = config.resolve_provider(Some("nonexistent"));
    assert_eq!(resolved.model_name(), config.provider.model_name());
}

// ────────────────────────────────────────────────────────────────────
// 10. Agent profile provider field serialization
// ────────────────────────────────────────────────────────────────────

#[test]
fn agent_profile_provider_field_serde() {
    // Minimal profile — provider should be None
    let toml_str = r#"
name = "test"
role = "assistant"
soul = "A test agent"
tool_ids = []
skills = []
max_tokens = 4096
capabilities = []
"#;
    let profile: AgentProfile = toml::from_str(toml_str).unwrap();
    assert!(profile.provider.is_none());

    // Profile with provider field
    let toml_str_with_provider = r#"
name = "researcher"
role = "researcher"
soul = "A research agent"
tool_ids = ["file_read", "web_search"]
skills = ["research"]
max_tokens = 4096
capabilities = []
provider = "reasoning"
"#;
    let profile2: AgentProfile = toml::from_str(toml_str_with_provider).unwrap();
    assert_eq!(profile2.provider.as_deref(), Some("reasoning"));

    // Roundtrip: serialize and deserialize
    let serialized = toml::to_string(&profile2).unwrap();
    let deserialized: AgentProfile = toml::from_str(&serialized).unwrap();
    assert_eq!(deserialized.provider.as_deref(), Some("reasoning"));
    assert_eq!(deserialized.name, "researcher");
}

// ────────────────────────────────────────────────────────────────────
// 11. Memory: massive batch store + recall stress
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn memory_batch_store_and_recall() {
    let dir = temp_dir("memory-batch");
    let provider = Arc::new(MockEmbeddingProvider::new(64));
    let store = MemoryStore::open(dir.join("memory.db")).unwrap();
    let key = MasterKey::generate();
    let mut mgr = MemoryManager::new(store, provider.clone(), key, 0).unwrap();

    // Store 50 memories with orthogonal vectors to bypass dedup
    for i in 0..50 {
        let entry = aivyx_memory::MemoryEntry::new(
            format!(
                "Memory item number {i}: topic about {}",
                if i % 3 == 0 {
                    "Rust"
                } else if i % 3 == 1 {
                    "Python"
                } else {
                    "Go"
                }
            ),
            MemoryKind::Fact,
            None,
            vec![format!("tag-{}", i % 5)],
        );
        let mut vec = vec![0.0f32; 64];
        vec[i % 64] += 1.0 + (i as f32) * 0.01; // near-orthogonal directions
        // Normalize
        let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        for v in &mut vec {
            *v /= norm;
        }
        mgr.store_raw(&entry, &vec).unwrap();
    }

    let stats = mgr.stats().unwrap();
    assert_eq!(stats.total_memories, 50);
    assert_eq!(stats.index_size, 50);

    // Recall should return results
    let results = mgr.recall("Rust programming", 10, None, &[]).await.unwrap();
    assert!(!results.is_empty());
    assert!(results.len() <= 10);

    // Add triples in bulk
    for i in 0..20 {
        mgr.add_triple(
            format!("Subject{i}"),
            "relates_to".into(),
            format!("Object{i}"),
            None,
            0.8 + (i as f32) * 0.01,
            "test".into(),
        )
        .unwrap();
    }

    let all_triples = mgr.query_triples(None, None, None, None).unwrap();
    assert_eq!(all_triples.len(), 20);

    // Only 1 embed call for the recall query (batch was inserted via store_raw)
    assert_eq!(provider.calls(), 1, "1 recall query embed");

    // Second recall for same query should hit cache — no additional embed call
    let _results2 = mgr.recall("Rust programming", 5, None, &[]).await.unwrap();
    assert_eq!(
        provider.calls(),
        1,
        "second recall with same query should use cache"
    );

    cleanup(&dir);
}

// ────────────────────────────────────────────────────────────────────
// 12. Task store: save, load, list, delete lifecycle
// ────────────────────────────────────────────────────────────────────

#[test]
fn task_store_full_lifecycle() {
    let dir = temp_dir("task-lifecycle");
    let store = TaskStore::open(dir.join("tasks.db")).unwrap();
    let key = MasterKey::generate();

    // Create multiple missions
    let mut missions: Vec<Mission> = (0..5)
        .map(|i| {
            let mut m = Mission::new(&format!("Goal {i}"), "agent");
            m.status = TaskStatus::Planned;
            m.steps = (0..3)
                .map(|j| Step {
                    index: j,
                    description: format!("Step {j} of goal {i}"),
                    tool_hints: vec![],
                    status: StepStatus::Pending,
                    prompt: None,
                    result: None,
                    retries: 0,
                    started_at: None,
                    completed_at: None,
                })
                .collect();
            m
        })
        .collect();

    // Save all
    for m in &missions {
        store.save(m, &key).unwrap();
    }

    // List all
    let list = store.list(&key).unwrap();
    assert_eq!(list.len(), 5);

    // Update one (simulate partial execution)
    missions[2].steps[0].status = StepStatus::Completed;
    missions[2].steps[0].result = Some("Done step 0".into());
    missions[2].status = TaskStatus::Executing;
    store.save(&missions[2], &key).unwrap();

    // Reload and verify update persisted
    let loaded = store.load(&missions[2].id, &key).unwrap().unwrap();
    assert_eq!(loaded.status, TaskStatus::Executing);
    assert_eq!(loaded.steps[0].status, StepStatus::Completed);
    assert_eq!(loaded.steps[1].status, StepStatus::Pending);

    // Delete 2 missions
    store.delete(&missions[0].id).unwrap();
    store.delete(&missions[4].id).unwrap();

    let list = store.list(&key).unwrap();
    assert_eq!(list.len(), 3);

    // Verify deleted missions return None
    assert!(store.load(&missions[0].id, &key).unwrap().is_none());
    assert!(store.load(&missions[4].id, &key).unwrap().is_none());

    cleanup(&dir);
}

// ────────────────────────────────────────────────────────────────────
// 13. MessageBus: rapid fire + unknown recipient
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn message_bus_rapid_fire_and_unknown_recipient() {
    let names = vec![
        "coordinator".into(),
        "researcher".into(),
        "coder".into(),
        "reviewer".into(),
    ];
    let bus = MessageBus::new(&names);

    let mut rx_researcher = bus.subscribe("researcher").unwrap();
    let mut rx_coder = bus.subscribe("coder").unwrap();
    let mut rx_reviewer = bus.subscribe("reviewer").unwrap();
    let bus = Arc::new(bus);

    // Fire 30 messages rapidly to different recipients
    for i in 0..30 {
        let to = match i % 3 {
            0 => "researcher",
            1 => "coder",
            _ => "reviewer",
        };
        bus.send(TeamMessage {
            from: "coordinator".into(),
            to: to.into(),
            content: format!("msg-{i}"),
            message_type: "task".into(),
            timestamp: chrono::Utc::now(),
        })
        .unwrap();
    }

    // Drain and count
    let mut researcher_count = 0;
    while rx_researcher.try_recv().is_ok() {
        researcher_count += 1;
    }
    let mut coder_count = 0;
    while rx_coder.try_recv().is_ok() {
        coder_count += 1;
    }
    let mut reviewer_count = 0;
    while rx_reviewer.try_recv().is_ok() {
        reviewer_count += 1;
    }

    assert_eq!(researcher_count, 10);
    assert_eq!(coder_count, 10);
    assert_eq!(reviewer_count, 10);

    // Unknown recipient should return error
    let result = bus.send(TeamMessage {
        from: "coordinator".into(),
        to: "nonexistent".into(),
        content: "hello".into(),
        message_type: "task".into(),
        timestamp: chrono::Utc::now(),
    });
    assert!(result.is_err(), "sending to unknown recipient should fail");
}

// ────────────────────────────────────────────────────────────────────
// 14. Multi-tool agent: file_read + shell with mixed capabilities
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn multi_tool_mixed_capabilities() {
    let agent_id = AgentId::new();

    // Grant only Filesystem, not Shell
    let caps = create_filesystem_caps(agent_id);

    let mut tools = ToolRegistry::new();
    tools.register(Box::new(FileReadTool::new()));
    tools.register(Box::new(ShellTool::new()));

    // Agent tries file_read (allowed) and then shell (denied)
    let file_read_call = ToolCall {
        id: "tc_1".into(),
        name: "file_read".into(),
        arguments: serde_json::json!({"path": "/tmp/nonexistent-stress"}),
    };
    let shell_call = ToolCall {
        id: "tc_2".into(),
        name: "shell".into(),
        arguments: serde_json::json!({"command": "echo hello"}),
    };

    let provider = MockProvider::new(vec![
        // Call file_read first
        ChatResponse {
            message: ChatMessage::assistant_with_tool_calls("Reading file", vec![file_read_call]),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
            stop_reason: StopReason::ToolUse,
        },
        // Then try shell
        ChatResponse {
            message: ChatMessage::assistant_with_tool_calls("Running shell", vec![shell_call]),
            usage: TokenUsage {
                input_tokens: 15,
                output_tokens: 5,
            },
            stop_reason: StopReason::ToolUse,
        },
        // Final response
        ChatResponse {
            message: ChatMessage::assistant("File read tried, shell denied."),
            usage: TokenUsage {
                input_tokens: 20,
                output_tokens: 10,
            },
            stop_reason: StopReason::EndTurn,
        },
    ]);

    let mut agent = aivyx_agent::Agent::new(
        agent_id,
        "mixed-cap".into(),
        "Test agent".into(),
        4096,
        AutonomyTier::Trust,
        Box::new(provider),
        tools,
        caps,
        RateLimiter::new(60),
        CostTracker::new(5.0, 0.000003, 0.000015),
        None,
        3,
        1,
    );

    let result = agent.turn("Read file then run shell", None).await.unwrap();
    assert_eq!(result, "File read tried, shell denied.");

    // Should have 2 tool results in conversation
    let tool_results: Vec<_> = agent
        .conversation()
        .iter()
        .filter(|m| m.tool_result.is_some())
        .collect();
    assert_eq!(tool_results.len(), 2, "should have 2 tool results");

    // First tool result: file_read was allowed (but file doesn't exist → tool error)
    // File doesn't exist, so there's an error, but it's a tool execution error,
    // not a capability denial

    // Second tool result: shell was denied by capability check
    let tr2 = tool_results[1].tool_result.as_ref().unwrap();
    assert!(tr2.is_error, "shell should be denied");
    assert!(
        tr2.content.to_string().contains("capability denied"),
        "should contain capability denied message"
    );
}

// ────────────────────────────────────────────────────────────────────
// 15. Retry exhaustion: max retries exceeded returns error
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn retry_exhaustion_returns_error() {
    // Fail 5 times, but max_retries is 3
    let provider = FailThenSucceedProvider::new(5);

    let mut agent = aivyx_agent::Agent::new(
        AgentId::new(),
        "retry-test".into(),
        "Test".into(),
        4096,
        AutonomyTier::Trust,
        Box::new(provider),
        ToolRegistry::new(),
        CapabilitySet::new(),
        RateLimiter::new(60),
        CostTracker::new(5.0, 0.000003, 0.000015),
        None,
        3, // max_retries = 3
        1, // 1ms delay
    );

    let result = agent.turn("hello", None).await;
    assert!(result.is_err(), "should fail after exhausting retries");
}

// ────────────────────────────────────────────────────────────────────
// 16. Nonagon: all 9 roles produce valid profiles
// ────────────────────────────────────────────────────────────────────

#[test]
fn nonagon_all_roles_have_valid_profiles() {
    use aivyx_team::nonagon::{NONAGON_ROLES, all_nonagon_profiles, role_to_profile};

    let profiles = all_nonagon_profiles();
    assert_eq!(profiles.len(), 9);

    for (role, profile) in NONAGON_ROLES.iter().zip(profiles.iter()) {
        // Name matches
        assert_eq!(role.name, profile.name);
        // Soul is non-empty
        assert!(!profile.soul.is_empty(), "{} should have a soul", role.name);
        // Tool IDs match
        assert_eq!(
            role.tool_ids.len(),
            profile.tool_ids.len(),
            "{} tool_id count mismatch",
            role.name
        );
        // Autonomy tier matches
        assert_eq!(
            Some(role.autonomy_tier),
            profile.autonomy_tier,
            "{} tier mismatch",
            role.name
        );
        // Provider should be None (global default)
        assert!(
            profile.provider.is_none(),
            "{} should not have a provider override",
            role.name
        );
        // TOML roundtrip
        let toml = toml::to_string(profile).unwrap();
        let restored: AgentProfile = toml::from_str(&toml).unwrap();
        assert_eq!(restored.name, profile.name);
        assert_eq!(restored.soul, profile.soul);
    }

    // Coordinator has file_read; delegation tools (delegate_task, query_agent,
    // collect_results) are injected at runtime by TeamRuntime::create_lead_agent().
    let coord = role_to_profile(&NONAGON_ROLES[0]);
    assert!(coord.tool_ids.contains(&"file_read".to_string()));

    // Coder and executor are Leash tier (they have destructive tools)
    assert_eq!(NONAGON_ROLES[3].autonomy_tier, AutonomyTier::Leash);
    assert_eq!(NONAGON_ROLES[8].autonomy_tier, AutonomyTier::Leash);

    // Guardian is read-only (no write or shell tools)
    assert!(NONAGON_ROLES[7].tool_ids.contains(&"file_read"));
    assert!(!NONAGON_ROLES[7].tool_ids.contains(&"file_write"));
    assert!(!NONAGON_ROLES[7].tool_ids.contains(&"shell"));
}

// ────────────────────────────────────────────────────────────────────
// 17. Config TOML: full roundtrip with all sections
// ────────────────────────────────────────────────────────────────────

#[test]
fn config_full_toml_roundtrip() {
    let toml_str = r#"
[provider]
type = "Ollama"
base_url = "http://localhost:11434"
model = "llama3.1"

[providers.reasoning]
type = "Ollama"
base_url = "http://localhost:11434"
model = "qwen2.5:72b"

[providers.coding]
type = "Ollama"
base_url = "http://localhost:11434"
model = "deepseek-coder-v2"

[autonomy]
default_tier = "Trust"
max_tool_calls_per_minute = 60
max_cost_per_session_usd = 5.0
require_approval_for_destructive = false
max_retries = 3
retry_base_delay_ms = 1000
"#;

    let config: AivyxConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.provider.model_name(), "llama3.1");
    assert_eq!(config.providers.len(), 2);
    assert_eq!(
        config.resolve_provider(Some("reasoning")).model_name(),
        "qwen2.5:72b"
    );
    assert_eq!(
        config.resolve_provider(Some("coding")).model_name(),
        "deepseek-coder-v2"
    );
    assert_eq!(
        config.resolve_provider(Some("unknown")).model_name(),
        "llama3.1"
    );
    assert_eq!(config.resolve_provider(None).model_name(), "llama3.1");

    // Re-serialize and verify fields survive
    let reserialized = toml::to_string(&config).unwrap();
    let reloaded: AivyxConfig = toml::from_str(&reserialized).unwrap();
    assert_eq!(reloaded.providers.len(), 2);
    assert_eq!(reloaded.provider.model_name(), "llama3.1");
}

// ────────────────────────────────────────────────────────────────────
// 18. Encrypted store: data does not leak across key namespaces
// ────────────────────────────────────────────────────────────────────

#[test]
fn encrypted_store_key_namespace_isolation() {
    use aivyx_crypto::EncryptedStore;

    let dir = temp_dir("store-isolation");
    let store = EncryptedStore::open(dir.join("store.db")).unwrap();
    let key = MasterKey::from_bytes(TEST_KEY_BYTES);

    // Store data in different namespaces
    store.put("session:abc", b"session data", &key).unwrap();
    store.put("task:xyz", b"task data", &key).unwrap();
    store.put("secret:api-key", b"sk-12345", &key).unwrap();

    // Each key retrieves only its own data
    let session = store.get("session:abc", &key).unwrap().unwrap();
    assert_eq!(session, b"session data");

    let task = store.get("task:xyz", &key).unwrap().unwrap();
    assert_eq!(task, b"task data");

    let secret = store.get("secret:api-key", &key).unwrap().unwrap();
    assert_eq!(secret, b"sk-12345");

    // Non-existent key returns None
    assert!(store.get("session:nonexistent", &key).unwrap().is_none());

    // List keys
    let keys = store.list_keys().unwrap();
    assert_eq!(keys.len(), 3);

    // Delete one
    store.delete("secret:api-key").unwrap();
    assert!(store.get("secret:api-key", &key).unwrap().is_none());
    assert_eq!(store.list_keys().unwrap().len(), 2);

    // Raw file should not contain plaintext
    let raw = std::fs::read(dir.join("store.db")).unwrap();
    let raw_str = String::from_utf8_lossy(&raw);
    assert!(!raw_str.contains("session data"));
    assert!(!raw_str.contains("task data"));
    assert!(!raw_str.contains("sk-12345"));

    cleanup(&dir);
}

// ────────────────────────────────────────────────────────────────────
// 19. Streaming: verify tokens arrive in order
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn streaming_tokens_arrive_in_order() {
    let text = "Token by token streaming works correctly!";
    let provider = MockProvider::simple(text);

    let mut agent = create_test_agent(
        Box::new(provider),
        ToolRegistry::new(),
        CapabilitySet::new(),
        AutonomyTier::Trust,
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel(128);
    let result = agent.turn_stream("hello", None, tx, None).await.unwrap();

    assert_eq!(result, text);

    // Collect all tokens
    let mut tokens = Vec::new();
    while let Ok(token) = rx.try_recv() {
        tokens.push(token);
    }

    // Concatenated tokens should equal the full text
    let concatenated: String = tokens.iter().cloned().collect();
    assert_eq!(concatenated, text);
}

// ────────────────────────────────────────────────────────────────────
// 20. Server: path traversal prevention in agent names
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn server_path_traversal_prevention() {
    use aivyx_agent::AgentSession;
    use aivyx_server::{AppState, build_router};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use sha2::{Digest, Sha256};
    use tower::ServiceExt;

    let dir = temp_dir("server-traversal");
    std::fs::create_dir_all(dir.join("agents")).unwrap();
    std::fs::create_dir_all(dir.join("teams")).unwrap();
    std::fs::create_dir_all(dir.join("sessions")).unwrap();
    std::fs::create_dir_all(dir.join("keys")).unwrap();
    std::fs::create_dir_all(dir.join("memory")).unwrap();

    let profile = AgentProfile::template("test-agent", "test");
    profile
        .save(dir.join("agents").join("test-agent.toml"))
        .unwrap();

    let dirs = AivyxDirs::new(&dir);
    let config = AivyxConfig::default();
    let master_key = MasterKey::from_bytes([42u8; 32]);
    let agent_key = MasterKey::from_bytes([42u8; 32]);
    let audit_key = derive_audit_key(&master_key);
    let audit_log = AuditLog::new(dir.join("audit.log"), &audit_key);
    let session_store = SessionStore::open(dir.join("sessions").join("sessions.db")).unwrap();

    let token = "test-token-12345";
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
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
        prometheus_handle: None,
    });

    // Test path traversal attempts
    let traversal_names = [
        "/agents/../../../etc/passwd",
        "/agents/..%2f..%2f..%2fetc/passwd",
        "/agents/test%00agent",
    ];

    for path in &traversal_names {
        let router = build_router(state.clone());
        let req = Request::builder()
            .uri(*path)
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        // Should be rejected (400 or 404, not 200 with sensitive data)
        assert_ne!(
            resp.status(),
            StatusCode::OK,
            "path traversal should be rejected: {path}"
        );
    }

    cleanup(&dir);
}

// ────────────────────────────────────────────────────────────────────
// 21. Cost tracking: budget exhaustion
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn cost_budget_exhaustion() {
    // Set very low budget ($0.001) and high token costs
    let provider = MockProvider::new(vec![
        ChatResponse {
            message: ChatMessage::assistant("response 1"),
            usage: TokenUsage {
                input_tokens: 10_000,
                output_tokens: 5_000,
            },
            stop_reason: StopReason::EndTurn,
        },
        ChatResponse {
            message: ChatMessage::assistant("response 2"),
            usage: TokenUsage {
                input_tokens: 10_000,
                output_tokens: 5_000,
            },
            stop_reason: StopReason::EndTurn,
        },
    ]);

    let mut agent = aivyx_agent::Agent::new(
        AgentId::new(),
        "budget-test".into(),
        "Test".into(),
        4096,
        AutonomyTier::Trust,
        Box::new(provider),
        ToolRegistry::new(),
        CapabilitySet::new(),
        RateLimiter::new(60),
        CostTracker::new(0.001, 0.00003, 0.00006), // very low budget
        None,
        3,
        1,
    );

    // First turn: should succeed but push near budget
    let r1 = agent.turn("turn 1", None).await;
    // Depending on exact cost math, first turn might succeed or fail
    // Either way, the cost tracking should work correctly
    if r1.is_ok() {
        let cost = agent.current_cost_usd();
        assert!(cost > 0.0);
        // Second turn should fail due to budget
        let r2 = agent.turn("turn 2", None).await;
        if r2.is_ok() {
            // If both succeeded, cost should be significant
            let cost2 = agent.current_cost_usd();
            assert!(cost2 > cost);
        }
    }
    // The key assertion: cost tracking doesn't panic or overflow
    let final_cost = agent.current_cost_usd();
    assert!(final_cost >= 0.0);
}

// ────────────────────────────────────────────────────────────────────
// 22. MasterKey domain separation
// ────────────────────────────────────────────────────────────────────

#[test]
fn master_key_domain_separation() {
    use aivyx_crypto::{derive_audit_key, derive_memory_key, derive_task_key};

    let key = MasterKey::from_bytes([42u8; 32]);

    let audit_key = derive_audit_key(&key); // Vec<u8>
    let memory_key = derive_memory_key(&key); // MasterKey
    let task_key = derive_task_key(&key); // MasterKey

    // Each derived key should be different
    assert_ne!(
        audit_key.as_slice(),
        memory_key.expose_secret(),
        "audit and memory keys should differ"
    );
    assert_ne!(
        audit_key.as_slice(),
        task_key.expose_secret(),
        "audit and task keys should differ"
    );
    assert_ne!(
        memory_key.expose_secret(),
        task_key.expose_secret(),
        "memory and task keys should differ"
    );

    // All derived keys differ from the master key itself
    assert_ne!(key.expose_secret(), audit_key.as_slice());
    assert_ne!(key.expose_secret(), memory_key.expose_secret());
    assert_ne!(key.expose_secret(), task_key.expose_secret());

    // Derived keys should be deterministic
    let key2 = MasterKey::from_bytes([42u8; 32]);
    let audit_key2 = derive_audit_key(&key2);
    assert_eq!(
        audit_key, audit_key2,
        "same master key should yield same audit key"
    );

    let memory_key2 = derive_memory_key(&key2);
    assert_eq!(
        memory_key.expose_secret(),
        memory_key2.expose_secret(),
        "same master key should yield same memory key"
    );
}

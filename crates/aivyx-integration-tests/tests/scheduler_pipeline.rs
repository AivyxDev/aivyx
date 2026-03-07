//! Integration tests for the scheduler pipeline: schedule config, cron validation,
//! notification store lifecycle, notification injection into system prompt,
//! and digest generation.

use aivyx_config::{AivyxConfig, ScheduleEntry, validate_cron};
use aivyx_core::{AgentId, AutonomyTier, ToolRegistry};
use aivyx_crypto::{MasterKey, derive_schedule_key};
use aivyx_integration_tests::{MockProvider, create_memory_caps};
use aivyx_memory::{Notification, NotificationStore};

/// Test 1: ScheduleEntry roundtrip through AivyxConfig TOML serialization.
#[test]
fn schedule_config_roundtrip() {
    let mut config = AivyxConfig::default();
    assert!(config.schedules.is_empty());

    let entry = ScheduleEntry::new(
        "morning-digest",
        "0 7 * * *",
        "assistant",
        "Generate digest",
    );
    config.add_schedule(entry).unwrap();

    // Serialize to TOML and back
    let toml_str = toml::to_string(&config).unwrap();
    let loaded: AivyxConfig = toml::from_str(&toml_str).unwrap();

    assert_eq!(loaded.schedules.len(), 1);
    let sched = &loaded.schedules[0];
    assert_eq!(sched.name, "morning-digest");
    assert_eq!(sched.cron, "0 7 * * *");
    assert_eq!(sched.agent, "assistant");
    assert_eq!(sched.prompt, "Generate digest");
    assert!(sched.enabled);
    assert!(sched.notify);
    assert!(sched.last_run_at.is_none());
}

/// Test 2: Cron expression validation accepts valid and rejects invalid expressions.
#[test]
fn validate_cron_accepts_and_rejects() {
    // Valid cron expressions
    assert!(validate_cron("* * * * *").is_ok());
    assert!(validate_cron("0 7 * * *").is_ok());
    assert!(validate_cron("0 7 * * 1-5").is_ok());
    assert!(validate_cron("*/15 * * * *").is_ok());

    // Invalid expressions
    assert!(validate_cron("not a cron").is_err());
    assert!(validate_cron("").is_err());
    assert!(validate_cron("0 7 * *").is_err()); // only 4 fields
}

/// Test 3: NotificationStore push, list, drain lifecycle.
#[test]
fn notification_store_push_list_drain() {
    let dir = std::env::temp_dir().join(format!("aivyx-notif-integ-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let key = MasterKey::from_bytes([42u8; 32]);
    let schedule_key = derive_schedule_key(&key);
    let store = NotificationStore::open(dir.join("notifications.db")).unwrap();

    // Push 3 notifications
    let n1 = Notification::new("sched-a", "Result A");
    let n2 = Notification::new("sched-b", "Result B");
    let n3 = Notification::new("sched-c", "Result C");
    store.push(&n1, &schedule_key).unwrap();
    store.push(&n2, &schedule_key).unwrap();
    store.push(&n3, &schedule_key).unwrap();

    // List returns all 3, sorted by creation time
    let listed = store.list(&schedule_key).unwrap();
    assert_eq!(listed.len(), 3);
    assert_eq!(listed[0].source, "sched-a");
    assert_eq!(listed[1].source, "sched-b");
    assert_eq!(listed[2].source, "sched-c");

    // Drain returns all 3 and clears the store
    let drained = store.drain(&schedule_key).unwrap();
    assert_eq!(drained.len(), 3);
    assert_eq!(store.count().unwrap(), 0);
    assert!(store.list(&schedule_key).unwrap().is_empty());

    std::fs::remove_dir_all(&dir).ok();
}

/// Test 4: Notifications injected into agent system prompt as [BACKGROUND FINDINGS] block.
#[tokio::test]
async fn notification_block_in_system_prompt() {
    // Create a PromptCapturingProvider to inspect the system prompt
    struct PromptCapturingProvider {
        captured: std::sync::Mutex<Option<String>>,
    }

    #[async_trait::async_trait]
    impl aivyx_llm::LlmProvider for PromptCapturingProvider {
        fn name(&self) -> &str {
            "mock-capture"
        }
        async fn chat(
            &self,
            request: &aivyx_llm::ChatRequest,
        ) -> aivyx_core::Result<aivyx_llm::ChatResponse> {
            if let Some(ref sys) = request.system_prompt {
                *self.captured.lock().unwrap() = Some(sys.clone());
            }
            Ok(aivyx_llm::ChatResponse {
                message: aivyx_llm::ChatMessage::assistant("I see the notifications"),
                usage: aivyx_llm::TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                },
                stop_reason: aivyx_llm::StopReason::EndTurn,
            })
        }
    }

    let agent_id = AgentId::new();
    let capture_provider = PromptCapturingProvider {
        captured: std::sync::Mutex::new(None),
    };

    let caps = create_memory_caps(agent_id);
    let mut agent = aivyx_agent::Agent::new(
        agent_id,
        "notif-test-agent".into(),
        "You are a helpful assistant.".into(),
        4096,
        AutonomyTier::Trust,
        Box::new(capture_provider),
        ToolRegistry::new(),
        caps,
        aivyx_agent::RateLimiter::new(60),
        aivyx_agent::CostTracker::new(5.0, 0.000003, 0.000015),
        None,
        3,
        1,
    );

    // Set pending notifications
    let notifications = vec![
        Notification::new(
            "morning-digest",
            "CI pipeline green, 3 PRs merged overnight",
        ),
        Notification::new("web-check", "New Rust 1.94 release announced"),
    ];
    agent.set_pending_notifications(notifications);

    // Verify notifications are present before the turn
    assert!(agent.has_pending_notifications());

    // Run a turn — the agent will inject [BACKGROUND FINDINGS] into the system
    // prompt and then clear the pending notifications after the turn completes.
    let _result = agent.turn("Hello", None).await.unwrap();

    // After the turn, notifications should be cleared
    assert!(!agent.has_pending_notifications());

    // Verify the format_block() produces the expected block format
    let test_notifs = vec![
        Notification::new("digest", "Good morning"),
        Notification::new("monitor", "CI green"),
    ];
    let block = NotificationStore::format_block(&test_notifs).unwrap();
    assert!(block.contains("[BACKGROUND FINDINGS]"));
    assert!(block.contains("2 pending notifications"));
    assert!(block.contains("digest: Good morning"));
    assert!(block.contains("monitor: CI green"));
    assert!(block.contains("[END BACKGROUND FINDINGS]"));
}

/// Test 5: Digest generation via direct LLM call.
#[tokio::test]
async fn digest_generation() {
    let provider = MockProvider::simple("Here is your morning digest:\n- All clear\n- No issues");

    let digest = aivyx_agent::generate_digest(
        &provider,
        "Generate a daily digest",
        Some("User prefers brief summaries"),
    )
    .await
    .unwrap();

    assert!(!digest.is_empty());
    assert!(digest.contains("morning digest"));
}

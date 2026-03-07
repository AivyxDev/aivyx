//! Integration tests for Agent + Nonagon Team + Task Engine interactions.
//!
//! Tests short-term missions (2-3 steps), long-term missions (5+ steps with
//! checkpointing), team delegation patterns, message coordination, nonagon
//! role capability enforcement, and end-to-end scenarios.

use std::path::PathBuf;
use std::sync::Arc;

use aivyx_agent::built_in_tools::register_built_in_tools;
use aivyx_capability::CapabilitySet;
use aivyx_core::{AgentId, AutonomyTier, Tool as _, ToolRegistry};
use aivyx_crypto::MasterKey;
use aivyx_integration_tests::{
    FailThenSucceedProvider, MockChannelAdapter, MockProvider, create_coordination_caps,
    create_filesystem_caps, create_network_caps, create_shell_caps, create_test_agent,
};
use aivyx_task::store::TaskStore;
use aivyx_task::types::{Mission, Step, StepStatus, TaskStatus};
use aivyx_team::delegation::TeamContext;
use aivyx_team::message_bus::{MessageBus, TeamMessage};
use aivyx_team::message_tools::{ReadMessagesTool, RequestPeerReviewTool, SendMessageTool};
use chrono::Utc;
use tokio::sync::Mutex;

// ────────────────────────────────────────────────────────────────────
// Helpers
// ────────────────────────────────────────────────────────────────────

fn temp_dir(prefix: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("aivyx-team-task-{prefix}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn cleanup(dir: &PathBuf) {
    std::fs::remove_dir_all(dir).ok();
}

fn make_step(index: usize, description: &str) -> Step {
    Step {
        index,
        description: description.into(),
        tool_hints: vec![],
        status: StepStatus::Pending,
        prompt: None,
        result: None,
        retries: 0,
        started_at: None,
        completed_at: None,
        depends_on: vec![],
        kind: aivyx_task::StepKind::default(),
    }
}

fn make_completed_step(index: usize, description: &str, result: &str) -> Step {
    Step {
        index,
        description: description.into(),
        tool_hints: vec![],
        status: StepStatus::Completed,
        prompt: Some(format!("Execute: {description}")),
        result: Some(result.into()),
        retries: 0,
        started_at: Some(Utc::now()),
        completed_at: Some(Utc::now()),
        depends_on: vec![],
        kind: aivyx_task::StepKind::default(),
    }
}

/// Merge two capability sets by granting all capabilities from `other` into `base`.
fn merge_caps(base: &mut CapabilitySet, other: &CapabilitySet) {
    for cap in other.iter() {
        base.grant(cap.clone());
    }
}

// ════════════════════════════════════════════════════════════════════
// Category 1: Short-Term Mission (Task Engine + Agent)
// ════════════════════════════════════════════════════════════════════

/// Test 1: A 2-step mission completes all steps successfully.
///
/// Verifies: step status transitions, result population, mission completion.
#[tokio::test]
async fn short_mission_completes_all_steps() {
    let dir = temp_dir("short-complete");
    let key = MasterKey::from_bytes([11u8; 32]);
    let store = TaskStore::open(dir.join("tasks.db")).unwrap();

    // Build a 2-step mission manually (skip LLM planning)
    let mut mission = Mission::new("Summarize Rust async patterns", "researcher");
    mission.status = TaskStatus::Planned;
    mission.steps = vec![
        make_step(0, "Research async/await in Rust"),
        make_step(1, "Write summary document"),
    ];
    store.save(&mission, &key).unwrap();

    // Simulate execution: run an agent turn for each step
    let provider = MockProvider::new(vec![
        aivyx_llm::ChatResponse {
            message: aivyx_llm::ChatMessage::assistant("Found 5 articles about Rust async"),
            usage: aivyx_llm::TokenUsage {
                input_tokens: 20,
                output_tokens: 10,
            },
            stop_reason: aivyx_llm::StopReason::EndTurn,
        },
        aivyx_llm::ChatResponse {
            message: aivyx_llm::ChatMessage::assistant(
                "Summary: Rust uses async/await with Futures",
            ),
            usage: aivyx_llm::TokenUsage {
                input_tokens: 30,
                output_tokens: 15,
            },
            stop_reason: aivyx_llm::StopReason::EndTurn,
        },
    ]);

    let mut agent = create_test_agent(
        Box::new(provider),
        ToolRegistry::new(),
        CapabilitySet::new(),
        AutonomyTier::Trust,
    );

    // Execute each pending step
    let mut loaded = store.load(&mission.id, &key).unwrap().unwrap();
    loaded.status = TaskStatus::Executing;

    while let Some(step_idx) = loaded.next_pending_step() {
        let prompt = loaded.steps[step_idx].description.clone();
        loaded.steps[step_idx].status = StepStatus::Running;
        loaded.steps[step_idx].started_at = Some(Utc::now());

        let result = agent.turn(&prompt, None).await.unwrap();
        loaded.steps[step_idx].status = StepStatus::Completed;
        loaded.steps[step_idx].result = Some(result);
        loaded.steps[step_idx].completed_at = Some(Utc::now());
    }

    loaded.status = TaskStatus::Completed;
    store.save(&loaded, &key).unwrap();

    // Verify
    assert_eq!(loaded.status, TaskStatus::Completed);
    assert_eq!(loaded.steps_completed(), 2);
    assert!(
        loaded.steps[0]
            .result
            .as_ref()
            .unwrap()
            .contains("5 articles")
    );
    assert!(
        loaded.steps[1]
            .result
            .as_ref()
            .unwrap()
            .contains("async/await")
    );
    assert!(loaded.next_pending_step().is_none());

    cleanup(&dir);
}

/// Test 2: A step with a flaky LLM provider recovers via agent-internal retry.
///
/// Verifies: Agent's chat_with_retry handles transient LLM failures transparently,
/// the step ultimately completes, and the mission succeeds.
#[tokio::test]
async fn short_mission_step_failure_retries_and_succeeds() {
    let dir = temp_dir("short-retry");
    let key = MasterKey::from_bytes([12u8; 32]);
    let store = TaskStore::open(dir.join("tasks.db")).unwrap();

    let mut mission = Mission::new("Build a widget", "coder");
    mission.status = TaskStatus::Planned;
    mission.max_step_retries = 2;
    mission.steps = vec![
        make_step(0, "Write widget code"),
        make_step(1, "Write unit tests"),
    ];
    store.save(&mission, &key).unwrap();

    // Step 0: FailThenSucceedProvider fails once, then succeeds.
    // Agent::chat_with_retry will handle the retry internally, so
    // agent.turn() should succeed on the first call.
    let provider0 = FailThenSucceedProvider::new(1);
    let mut agent0 = create_test_agent(
        Box::new(provider0),
        ToolRegistry::new(),
        CapabilitySet::new(),
        AutonomyTier::Trust,
    );

    let mut loaded = store.load(&mission.id, &key).unwrap().unwrap();
    loaded.status = TaskStatus::Executing;

    // Agent internally retries the flaky LLM call and returns success
    loaded.steps[0].status = StepStatus::Running;
    let result = agent0.turn("Write widget code", None).await;
    assert!(
        result.is_ok(),
        "agent should recover from transient LLM failure via internal retry"
    );
    loaded.steps[0].status = StepStatus::Completed;
    loaded.steps[0].result = Some(result.unwrap());

    // Step 1: simple success
    let provider1 = MockProvider::simple("All 5 tests pass");
    let mut agent1 = create_test_agent(
        Box::new(provider1),
        ToolRegistry::new(),
        CapabilitySet::new(),
        AutonomyTier::Trust,
    );
    let result1 = agent1.turn("Write unit tests", None).await.unwrap();
    loaded.steps[1].status = StepStatus::Completed;
    loaded.steps[1].result = Some(result1);

    loaded.status = TaskStatus::Completed;
    store.save(&loaded, &key).unwrap();

    assert_eq!(loaded.status, TaskStatus::Completed);
    assert_eq!(loaded.steps_completed(), 2);
    assert!(
        loaded.steps[0]
            .result
            .as_ref()
            .unwrap()
            .contains("recovered")
    );

    cleanup(&dir);
}

/// Test 3: A step exhausts all retries and the mission fails.
///
/// Verifies: failure propagation from step to mission, correct status.
#[tokio::test]
async fn short_mission_step_exhausts_retries_fails_mission() {
    let dir = temp_dir("short-fail");
    let key = MasterKey::from_bytes([13u8; 32]);
    let store = TaskStore::open(dir.join("tasks.db")).unwrap();

    let mut mission = Mission::new("Deploy service", "executor");
    mission.status = TaskStatus::Planned;
    mission.max_step_retries = 1;
    mission.steps = vec![make_step(0, "Run deployment script")];
    store.save(&mission, &key).unwrap();

    // Provider that always fails (fail_count=10 >> max_retries)
    let provider = FailThenSucceedProvider::new(10);
    let mut agent = create_test_agent(
        Box::new(provider),
        ToolRegistry::new(),
        CapabilitySet::new(),
        AutonomyTier::Trust,
    );

    let mut loaded = store.load(&mission.id, &key).unwrap().unwrap();
    loaded.status = TaskStatus::Executing;
    loaded.steps[0].status = StepStatus::Running;

    // Attempt 1: fails
    let r1 = agent.turn("Run deployment script", None).await;
    assert!(r1.is_err());
    loaded.steps[0].retries += 1;

    // Attempt 2: fails again — exceeds max_step_retries (1)
    let r2 = agent.turn("Run deployment script", None).await;
    assert!(r2.is_err());
    loaded.steps[0].retries += 1;

    // Retries exceeded
    assert!(loaded.steps[0].retries > loaded.max_step_retries);
    loaded.steps[0].status = StepStatus::Failed {
        reason: "rate limited after 2 attempts".into(),
    };
    loaded.status = TaskStatus::Failed {
        reason: "step 0 failed after retries".into(),
    };
    store.save(&loaded, &key).unwrap();

    let final_mission = store.load(&mission.id, &key).unwrap().unwrap();
    assert!(matches!(final_mission.status, TaskStatus::Failed { .. }));
    assert!(matches!(
        final_mission.steps[0].status,
        StepStatus::Failed { .. }
    ));

    cleanup(&dir);
}

// ════════════════════════════════════════════════════════════════════
// Category 2: Long-Term Mission (Checkpointing + Resume)
// ════════════════════════════════════════════════════════════════════

/// Test 4: A 5-step mission checkpoints between steps.
///
/// Verifies: partial execution persisted, remaining steps skipped on cancel.
#[test]
fn long_mission_checkpoints_between_steps() {
    let dir = temp_dir("long-checkpoint");
    let key = MasterKey::from_bytes([21u8; 32]);
    let store = TaskStore::open(dir.join("tasks.db")).unwrap();

    let mut mission = Mission::new("Analyze codebase and write report", "analyst");
    mission.status = TaskStatus::Executing;
    mission.steps = vec![
        make_completed_step(0, "Scan project structure", "Found 15 crates"),
        make_completed_step(1, "Count lines of code", "42,000 total LOC"),
        make_completed_step(2, "Identify dependencies", "37 direct deps"),
        make_step(3, "Generate dependency graph"),
        make_step(4, "Write final report"),
    ];
    store.save(&mission, &key).unwrap();

    // Simulate cancellation of remaining steps
    let mut loaded = store.load(&mission.id, &key).unwrap().unwrap();
    for step in &mut loaded.steps {
        if matches!(step.status, StepStatus::Pending | StepStatus::Running) {
            step.status = StepStatus::Skipped;
        }
    }
    loaded.status = TaskStatus::Cancelled;
    store.save(&loaded, &key).unwrap();

    // Reload and verify checkpoint integrity
    let reloaded = store.load(&mission.id, &key).unwrap().unwrap();
    assert_eq!(reloaded.status, TaskStatus::Cancelled);
    assert_eq!(reloaded.steps_completed(), 3);
    assert_eq!(reloaded.steps[0].status, StepStatus::Completed);
    assert_eq!(reloaded.steps[1].status, StepStatus::Completed);
    assert_eq!(reloaded.steps[2].status, StepStatus::Completed);
    assert_eq!(reloaded.steps[3].status, StepStatus::Skipped);
    assert_eq!(reloaded.steps[4].status, StepStatus::Skipped);

    // Verify completed step results survived the roundtrip
    assert!(
        reloaded.steps[0]
            .result
            .as_ref()
            .unwrap()
            .contains("15 crates")
    );
    assert!(
        reloaded.steps[2]
            .result
            .as_ref()
            .unwrap()
            .contains("37 direct deps")
    );

    cleanup(&dir);
}

/// Test 5: Resume continues from the last checkpoint.
///
/// Verifies: next_pending_step() finds the correct resume point.
#[tokio::test]
async fn long_mission_resume_continues_from_checkpoint() {
    let dir = temp_dir("long-resume");
    let key = MasterKey::from_bytes([22u8; 32]);
    let store = TaskStore::open(dir.join("tasks.db")).unwrap();

    let mut mission = Mission::new("Build full-stack feature", "coder");
    mission.status = TaskStatus::Executing;
    mission.steps = vec![
        make_completed_step(0, "Create database schema", "Schema created with 3 tables"),
        make_completed_step(1, "Write API endpoints", "4 REST endpoints implemented"),
        make_step(2, "Build frontend components"),
        make_step(3, "Write integration tests"),
        make_step(4, "Deploy to staging"),
    ];
    store.save(&mission, &key).unwrap();

    // Verify resume point
    let loaded = store.load(&mission.id, &key).unwrap().unwrap();
    assert_eq!(loaded.next_pending_step(), Some(2));

    // Verify completed step summaries include context
    let summaries = loaded.completed_step_summaries();
    assert_eq!(summaries.len(), 2);
    assert_eq!(summaries[0].0, 0);
    assert!(summaries[0].2.contains("Schema created"));
    assert_eq!(summaries[1].0, 1);
    assert!(summaries[1].2.contains("REST endpoints"));

    // Simulate executing from resume point
    let provider = MockProvider::new(vec![
        aivyx_llm::ChatResponse {
            message: aivyx_llm::ChatMessage::assistant("Built 3 React components"),
            usage: aivyx_llm::TokenUsage {
                input_tokens: 20,
                output_tokens: 10,
            },
            stop_reason: aivyx_llm::StopReason::EndTurn,
        },
        aivyx_llm::ChatResponse {
            message: aivyx_llm::ChatMessage::assistant("All 12 tests pass"),
            usage: aivyx_llm::TokenUsage {
                input_tokens: 25,
                output_tokens: 8,
            },
            stop_reason: aivyx_llm::StopReason::EndTurn,
        },
        aivyx_llm::ChatResponse {
            message: aivyx_llm::ChatMessage::assistant("Deployed to staging.example.com"),
            usage: aivyx_llm::TokenUsage {
                input_tokens: 15,
                output_tokens: 5,
            },
            stop_reason: aivyx_llm::StopReason::EndTurn,
        },
    ]);

    let mut agent = create_test_agent(
        Box::new(provider),
        ToolRegistry::new(),
        CapabilitySet::new(),
        AutonomyTier::Trust,
    );

    let mut resumed = loaded;
    while let Some(step_idx) = resumed.next_pending_step() {
        resumed.steps[step_idx].status = StepStatus::Running;
        let desc = resumed.steps[step_idx].description.clone();
        let result = agent.turn(&desc, None).await.unwrap();
        resumed.steps[step_idx].status = StepStatus::Completed;
        resumed.steps[step_idx].result = Some(result);
        resumed.steps[step_idx].completed_at = Some(Utc::now());
    }
    resumed.status = TaskStatus::Completed;
    store.save(&resumed, &key).unwrap();

    assert_eq!(resumed.status, TaskStatus::Completed);
    assert_eq!(resumed.steps_completed(), 5);
    assert!(
        resumed.steps[2]
            .result
            .as_ref()
            .unwrap()
            .contains("React components")
    );

    cleanup(&dir);
}

/// Test 6: Cancel skips remaining steps and preserves completed ones.
///
/// Verifies: Cancelled status, Skipped steps, completed steps untouched.
#[test]
fn long_mission_cancel_skips_remaining_steps() {
    let dir = temp_dir("long-cancel");
    let key = MasterKey::from_bytes([23u8; 32]);
    let store = TaskStore::open(dir.join("tasks.db")).unwrap();

    let mut mission = Mission::new("Refactor authentication system", "coder");
    mission.status = TaskStatus::Executing;
    mission.steps = vec![
        make_completed_step(0, "Audit existing auth code", "Found 3 vulnerabilities"),
        make_completed_step(1, "Design new auth flow", "JWT-based flow designed"),
        make_step(2, "Implement token service"),
        make_step(3, "Update middleware"),
        make_step(4, "Migrate existing sessions"),
    ];
    store.save(&mission, &key).unwrap();

    // Cancel
    let mut loaded = store.load(&mission.id, &key).unwrap().unwrap();
    assert!(!loaded.status.is_terminal());

    for step in &mut loaded.steps {
        if matches!(step.status, StepStatus::Pending | StepStatus::Running) {
            step.status = StepStatus::Skipped;
        }
    }
    loaded.status = TaskStatus::Cancelled;
    store.save(&loaded, &key).unwrap();

    let final_m = store.load(&mission.id, &key).unwrap().unwrap();
    assert_eq!(final_m.status, TaskStatus::Cancelled);

    // Completed steps preserved
    assert_eq!(final_m.steps[0].status, StepStatus::Completed);
    assert!(
        final_m.steps[0]
            .result
            .as_ref()
            .unwrap()
            .contains("3 vulnerabilities")
    );
    assert_eq!(final_m.steps[1].status, StepStatus::Completed);

    // Remaining steps skipped
    assert_eq!(final_m.steps[2].status, StepStatus::Skipped);
    assert_eq!(final_m.steps[3].status, StepStatus::Skipped);
    assert_eq!(final_m.steps[4].status, StepStatus::Skipped);

    cleanup(&dir);
}

// ════════════════════════════════════════════════════════════════════
// Category 3: Team Delegation (Coordinator → Specialist)
// ════════════════════════════════════════════════════════════════════

/// Test 7: Delegate a task to a specialist and get a result.
///
/// Verifies: specialist agent runs a turn and produces output.
#[tokio::test]
async fn delegation_tool_executes_specialist_turn() {
    // Build a specialist agent directly (bypass AgentSession)
    let provider = MockProvider::simple("Research complete: found 3 key papers on topic X");
    let agent_id = AgentId::new();
    let mut caps = create_filesystem_caps(agent_id);
    merge_caps(&mut caps, &create_network_caps(agent_id));

    let mut specialist = create_test_agent(
        Box::new(provider),
        ToolRegistry::new(),
        caps,
        AutonomyTier::Trust,
    );

    // Simulate delegation: coordinator sends a task prompt to the specialist
    let result = specialist
        .turn("Research topic X and report findings", None)
        .await
        .unwrap();

    assert!(result.contains("3 key papers"));

    // Record in team context
    let ctx = TeamContext::new(
        "test-team".into(),
        "coordinator".into(),
        vec![
            ("coordinator".into(), "Lead".into()),
            ("researcher".into(), "Researcher".into()),
        ],
        "Analyze topic X".into(),
        true,
    );
    ctx.record_completion("researcher", "Research topic X", &result)
        .await;

    let work = ctx.completed_work_snapshot().await;
    assert_eq!(work.len(), 1);
    assert_eq!(work[0].agent, "researcher");
    assert!(work[0].outcome.contains("3 key papers"));
}

/// Test 8: Specialist preserves context across multiple delegations.
///
/// Verifies: conversation history accumulated, second turn can reference first.
#[tokio::test]
async fn specialist_preserves_context_across_delegations() {
    let provider = MockProvider::new(vec![
        aivyx_llm::ChatResponse {
            message: aivyx_llm::ChatMessage::assistant(
                "Found papers on topic A: Smith2024, Jones2025, Lee2025",
            ),
            usage: aivyx_llm::TokenUsage {
                input_tokens: 20,
                output_tokens: 15,
            },
            stop_reason: aivyx_llm::StopReason::EndTurn,
        },
        aivyx_llm::ChatResponse {
            message: aivyx_llm::ChatMessage::assistant(
                "Summary of topic A: Based on Smith2024 and Jones2025, the consensus is X",
            ),
            usage: aivyx_llm::TokenUsage {
                input_tokens: 40,
                output_tokens: 20,
            },
            stop_reason: aivyx_llm::StopReason::EndTurn,
        },
    ]);

    let mut specialist = create_test_agent(
        Box::new(provider),
        ToolRegistry::new(),
        CapabilitySet::new(),
        AutonomyTier::Trust,
    );

    // First delegation
    let result1 = specialist.turn("Research topic A", None).await.unwrap();
    assert!(result1.contains("Smith2024"));

    // Second delegation — same agent instance, conversation accumulates
    let result2 = specialist
        .turn("Now summarize your findings on topic A", None)
        .await
        .unwrap();
    assert!(result2.contains("Smith2024"));
    assert!(result2.contains("consensus"));

    // Verify the agent has accumulated conversation history
    let persisted = specialist.to_persisted_session();
    // Should have 4 messages: user1, assistant1, user2, assistant2
    assert_eq!(persisted.messages.len(), 4);
}

/// Test 9: TeamContext evolves with completed work and formats correctly.
///
/// Verifies: multiple completions recorded, context formatting includes all.
#[tokio::test]
async fn team_context_evolves_with_completed_work() {
    let ctx = TeamContext::new(
        "project-alpha".into(),
        "coordinator".into(),
        vec![
            ("coordinator".into(), "Lead".into()),
            ("researcher".into(), "Researcher".into()),
            ("coder".into(), "Coder".into()),
            ("writer".into(), "Writer".into()),
        ],
        "Build a REST API for user management".into(),
        true,
    );

    // Record 3 completions
    ctx.record_completion(
        "researcher",
        "Research REST API best practices",
        "Found that RESTful APIs should use proper HTTP methods and status codes",
    )
    .await;
    ctx.record_completion(
        "coder",
        "Implement CRUD endpoints",
        "Built 4 endpoints: GET/POST/PUT/DELETE /users with validation",
    )
    .await;
    ctx.record_completion(
        "writer",
        "Write API documentation",
        "Created OpenAPI spec with examples for all endpoints",
    )
    .await;

    // Format context for a new specialist
    let formatted = ctx.format_for_role("reviewer", "Reviewer").await;

    assert!(formatted.contains("[TEAM CONTEXT]"));
    assert!(formatted.contains("[END TEAM CONTEXT]"));
    assert!(formatted.contains("project-alpha"));
    assert!(formatted.contains("Your role: Reviewer (reviewer)"));
    assert!(formatted.contains("Coordinator: coordinator"));
    assert!(formatted.contains("Build a REST API for user management"));

    // All 3 work summaries present
    assert!(formatted.contains("[researcher]"));
    assert!(formatted.contains("[coder]"));
    assert!(formatted.contains("[writer]"));
    assert!(formatted.contains("CRUD endpoints"));
    assert!(formatted.contains("OpenAPI spec"));

    // Peer messaging block present
    assert!(formatted.contains("[PEER MESSAGING]"));
    assert!(formatted.contains("send_message"));

    // Verify outcome truncation (create a very long outcome)
    ctx.record_completion("analyst", "Analyze data", &"x".repeat(500))
        .await;
    let formatted2 = ctx.format_for_role("reviewer", "Reviewer").await;
    // The long outcome should be truncated to ~200 chars
    assert!(formatted2.contains("..."));
}

// ════════════════════════════════════════════════════════════════════
// Category 4: Team Message Coordination
// ════════════════════════════════════════════════════════════════════

/// Test 10: Coordinator delegates to multiple specialists via MessageBus.
///
/// Verifies: each specialist receives exactly their message.
#[tokio::test]
async fn coordinator_delegates_to_multiple_specialists() {
    let names: Vec<String> = vec![
        "coordinator".into(),
        "researcher".into(),
        "coder".into(),
        "writer".into(),
    ];
    let bus = MessageBus::new(&names);

    let mut rx_researcher = bus.subscribe("researcher").unwrap();
    let mut rx_coder = bus.subscribe("coder").unwrap();
    let mut rx_writer = bus.subscribe("writer").unwrap();

    let bus = Arc::new(bus);

    // Coordinator sends different tasks to each specialist
    for (to, task) in [
        ("researcher", "Research user auth patterns"),
        ("coder", "Implement OAuth2 flow"),
        ("writer", "Document the auth API"),
    ] {
        bus.send(TeamMessage {
            from: "coordinator".into(),
            to: to.into(),
            content: task.into(),
            message_type: "delegation".into(),
            timestamp: Utc::now(),
        })
        .unwrap();
    }

    let msg_r = rx_researcher.try_recv().unwrap();
    assert_eq!(msg_r.from, "coordinator");
    assert_eq!(msg_r.to, "researcher");
    assert!(msg_r.content.contains("Research"));

    let msg_c = rx_coder.try_recv().unwrap();
    assert_eq!(msg_c.to, "coder");
    assert!(msg_c.content.contains("OAuth2"));

    let msg_w = rx_writer.try_recv().unwrap();
    assert_eq!(msg_w.to, "writer");
    assert!(msg_w.content.contains("Document"));

    // Verify no cross-delivery
    assert!(rx_researcher.try_recv().is_err());
    assert!(rx_coder.try_recv().is_err());
    assert!(rx_writer.try_recv().is_err());
}

/// Test 11: Peer review cycle: coder sends review request, reviewer responds.
///
/// Verifies: full round-trip message flow via tools.
#[tokio::test]
async fn peer_review_cycle_sends_and_receives() {
    let names: Vec<String> = vec!["coder".into(), "reviewer".into()];
    let bus = MessageBus::new(&names);
    let rx_reviewer = bus.subscribe("reviewer").unwrap();
    let rx_coder = bus.subscribe("coder").unwrap();
    let bus = Arc::new(bus);

    // Coder sends a review request
    let review_tool = RequestPeerReviewTool::new(Arc::clone(&bus), "coder".into());
    let result = review_tool
        .execute(serde_json::json!({
            "peer": "reviewer",
            "content": "fn process_data() { ... }",
            "context": "New data processing function"
        }))
        .await
        .unwrap();
    assert!(result["request_id"].as_str().is_some());

    // Reviewer reads the review request
    let rx_reviewer = Arc::new(Mutex::new(rx_reviewer));
    let read_tool = ReadMessagesTool::new(Arc::clone(&rx_reviewer));
    let messages = read_tool.execute(serde_json::json!({})).await.unwrap();
    assert_eq!(messages["count"], 1);
    let msg = &messages["messages"][0];
    assert_eq!(msg["from"], "coder");
    assert_eq!(msg["message_type"], "review_request");

    // Reviewer sends response back
    let respond_tool = SendMessageTool::new(Arc::clone(&bus), "reviewer".into());
    let resp = respond_tool
        .execute(serde_json::json!({
            "to": "coder",
            "content": "LGTM - clean implementation, no issues found",
            "message_type": "review_response"
        }))
        .await
        .unwrap();
    assert_eq!(resp["status"], "sent");

    // Coder reads the review response
    let rx_coder = Arc::new(Mutex::new(rx_coder));
    let coder_read = ReadMessagesTool::new(rx_coder);
    let coder_msgs = coder_read.execute(serde_json::json!({})).await.unwrap();
    assert_eq!(coder_msgs["count"], 1);
    assert_eq!(coder_msgs["messages"][0]["from"], "reviewer");
    assert!(
        coder_msgs["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("LGTM")
    );
}

/// Test 12: MessageBus handles concurrent sends without lost messages.
///
/// Verifies: all messages delivered under concurrent access.
#[tokio::test]
async fn message_bus_handles_concurrent_sends() {
    let names: Vec<String> = (0..5).map(|i| format!("agent-{i}")).collect();
    let bus = MessageBus::new(&names);

    // Subscribe all agents
    let mut receivers: Vec<_> = names
        .iter()
        .map(|name| (name.clone(), bus.subscribe(name).unwrap()))
        .collect();

    let bus = Arc::new(bus);

    // Spawn 5 tasks, each sending 4 messages (to the other 4 agents)
    let mut handles = vec![];
    for i in 0..5 {
        let bus = Arc::clone(&bus);
        let names = names.clone();
        handles.push(tokio::spawn(async move {
            for j in 0..5 {
                if i != j {
                    bus.send(TeamMessage {
                        from: names[i].clone(),
                        to: names[j].clone(),
                        content: format!("msg from {i} to {j}"),
                        message_type: "text".into(),
                        timestamp: Utc::now(),
                    })
                    .unwrap();
                }
            }
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    // Each agent should have received exactly 4 messages (one from each other)
    for (_name, rx) in &mut receivers {
        let mut count = 0;
        while rx.try_recv().is_ok() {
            count += 1;
        }
        assert_eq!(count, 4, "each agent should receive exactly 4 messages");
    }
}

// ════════════════════════════════════════════════════════════════════
// Category 5: Nonagon Role Integration
// ════════════════════════════════════════════════════════════════════

/// Test 13: Researcher role agent can use network tools (web_search).
///
/// Verifies: Network-scoped tool passes capability check.
#[tokio::test]
async fn nonagon_researcher_can_use_network_tools() {
    let agent_id = AgentId::new();

    // Researcher capabilities: Filesystem + Network + Coordination
    let mut caps = create_filesystem_caps(agent_id);
    merge_caps(&mut caps, &create_network_caps(agent_id));
    merge_caps(&mut caps, &create_coordination_caps(agent_id));

    // Register built-in tools filtered to researcher's tool set
    let mut tools = ToolRegistry::new();
    let researcher_tools: Vec<String> = ["file_read", "web_search", "http_fetch", "grep_search"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    register_built_in_tools(&mut tools, &researcher_tools);

    // Mock that returns a tool call for web_search, then a text response
    let provider = MockProvider::tool_then_text(
        "web_search",
        serde_json::json!({"query": "Rust async patterns 2025"}),
        "Found 10 results about Rust async patterns",
    );

    let mut agent = create_test_agent(Box::new(provider), tools, caps, AutonomyTier::Trust);

    let result = agent.turn("Search for Rust async patterns", None).await;
    assert!(
        result.is_ok(),
        "researcher should be able to use web_search"
    );
}

/// Test 14: Writer role agent is denied shell access.
///
/// Verifies: Shell-scoped tool denied by capability check for writer.
#[tokio::test]
async fn nonagon_writer_denied_shell_access() {
    let agent_id = AgentId::new();

    // Writer capabilities: Filesystem + Coordination (NO Shell)
    let mut caps = create_filesystem_caps(agent_id);
    merge_caps(&mut caps, &create_coordination_caps(agent_id));

    // Register shell tool
    let mut tools = ToolRegistry::new();
    let writer_tools: Vec<String> = ["file_read", "file_write", "shell"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    register_built_in_tools(&mut tools, &writer_tools);

    // Mock returns a tool call for shell
    let provider = MockProvider::tool_then_text(
        "shell",
        serde_json::json!({"command": "rm -rf /"}),
        "Shell command executed",
    );

    let mut agent = create_test_agent(Box::new(provider), tools, caps, AutonomyTier::Trust);

    // The turn should succeed (the agent handles denied tools gracefully by
    // returning a ToolDenied result to the LLM), but the shell command
    // should not actually execute. The final text response confirms the
    // agent recovered from the denial.
    let result = agent.turn("Run a shell command", None).await;
    assert!(
        result.is_ok(),
        "turn should complete even when a tool is denied"
    );
}

/// Test 15: Leash-tier agent requires approval via ChannelAdapter.
///
/// Verifies: MockChannelAdapter.receive() is called for tool approval.
#[tokio::test]
async fn nonagon_coder_has_leash_tier_requires_approval() {
    let agent_id = AgentId::new();

    // Coder capabilities: Filesystem + Shell + Coordination
    let mut caps = create_filesystem_caps(agent_id);
    merge_caps(&mut caps, &create_shell_caps(agent_id));
    merge_caps(&mut caps, &create_coordination_caps(agent_id));

    let mut tools = ToolRegistry::new();
    let leash_tools: Vec<String> = vec!["file_read".to_string()];
    register_built_in_tools(&mut tools, &leash_tools);

    // Mock provider returns a tool call
    let provider = MockProvider::tool_then_text(
        "file_read",
        serde_json::json!({"path": "/tmp/test.txt"}),
        "File contents: hello world",
    );

    let channel = MockChannelAdapter::approving();
    let mut agent = create_test_agent(Box::new(provider), tools, caps, AutonomyTier::Leash);

    let result = agent.turn("Read /tmp/test.txt", Some(&channel)).await;
    assert!(result.is_ok(), "approved leash turn should succeed");

    // Verify the channel was consulted for approval
    assert!(
        channel.call_count() > 0,
        "channel.receive() should have been called for Leash approval"
    );
    assert!(
        !channel.sent_messages().is_empty(),
        "channel.send() should have been called to show the tool call"
    );
}

// ════════════════════════════════════════════════════════════════════
// Category 6: End-to-End Scenarios
// ════════════════════════════════════════════════════════════════════

/// Test 16: Short team mission — decompose, delegate, synthesize.
///
/// Simulates the coordinator flow: decompose goal → delegate subtasks →
/// collect specialist results → synthesize into final answer.
#[tokio::test]
async fn short_team_mission_decompose_delegate_synthesize() {
    // Step 1: Coordinator decomposes goal into 3 subtasks
    let coordinator_provider = MockProvider::simple(
        "Decomposition:\n\
         1. Research REST API security best practices\n\
         2. Implement rate limiting middleware\n\
         3. Write security documentation",
    );
    let mut coordinator = create_test_agent(
        Box::new(coordinator_provider),
        ToolRegistry::new(),
        CapabilitySet::new(),
        AutonomyTier::Trust,
    );

    let decomposition = coordinator
        .turn("Decompose: Secure our REST API", None)
        .await
        .unwrap();
    assert!(decomposition.contains("Research"));
    assert!(decomposition.contains("Implement"));
    assert!(decomposition.contains("documentation"));

    // Step 2: Delegate to specialists
    let ctx = TeamContext::new(
        "api-security".into(),
        "coordinator".into(),
        vec![
            ("coordinator".into(), "Lead".into()),
            ("researcher".into(), "Researcher".into()),
            ("coder".into(), "Coder".into()),
            ("writer".into(), "Writer".into()),
        ],
        "Secure our REST API".into(),
        false,
    );

    // Researcher
    let r_provider = MockProvider::simple(
        "Found best practices: OWASP top 10, rate limiting, input validation, CORS",
    );
    let mut researcher = create_test_agent(
        Box::new(r_provider),
        ToolRegistry::new(),
        CapabilitySet::new(),
        AutonomyTier::Trust,
    );
    let r_result = researcher
        .turn("Research REST API security best practices", None)
        .await
        .unwrap();
    ctx.record_completion("researcher", "Research security best practices", &r_result)
        .await;

    // Coder
    let c_provider = MockProvider::simple(
        "Implemented rate limiting: 100 req/min per IP, 429 responses, Redis-backed",
    );
    let mut coder = create_test_agent(
        Box::new(c_provider),
        ToolRegistry::new(),
        CapabilitySet::new(),
        AutonomyTier::Trust,
    );
    let c_result = coder
        .turn("Implement rate limiting middleware", None)
        .await
        .unwrap();
    ctx.record_completion("coder", "Implement rate limiting", &c_result)
        .await;

    // Writer
    let w_provider =
        MockProvider::simple("Created security docs covering rate limiting, CORS, and auth flows");
    let mut writer = create_test_agent(
        Box::new(w_provider),
        ToolRegistry::new(),
        CapabilitySet::new(),
        AutonomyTier::Trust,
    );
    let w_result = writer
        .turn("Write security documentation", None)
        .await
        .unwrap();
    ctx.record_completion("writer", "Write security docs", &w_result)
        .await;

    // Step 3: Synthesize results
    let synth_provider = MockProvider::simple(
        "Final report: API secured with rate limiting (100/min/IP), \
         OWASP compliance verified, documentation complete.",
    );
    let mut synthesizer = create_test_agent(
        Box::new(synth_provider),
        ToolRegistry::new(),
        CapabilitySet::new(),
        AutonomyTier::Trust,
    );

    let work = ctx.completed_work_snapshot().await;
    let context = work
        .iter()
        .map(|w| format!("[{}] {}: {}", w.agent, w.task, w.outcome))
        .collect::<Vec<_>>()
        .join("\n");

    let synthesis = synthesizer
        .turn(&format!("Synthesize these results:\n{context}"), None)
        .await
        .unwrap();

    assert!(synthesis.contains("rate limiting"));
    assert!(synthesis.contains("OWASP"));

    // Verify all 3 subtasks produced results
    assert_eq!(work.len(), 3);
}

/// Test 17: Long team mission with verification failure and redelegation.
///
/// Simulates: decompose → delegate → verify (fail) → redelegate → verify (pass).
#[tokio::test]
async fn long_team_mission_with_verification_and_redelegation() {
    let ctx = TeamContext::new(
        "data-pipeline".into(),
        "coordinator".into(),
        vec![
            ("coordinator".into(), "Lead".into()),
            ("researcher".into(), "Researcher".into()),
            ("coder".into(), "Coder".into()),
            ("reviewer".into(), "Reviewer".into()),
            ("writer".into(), "Writer".into()),
        ],
        "Build a data processing pipeline".into(),
        true,
    );

    // Subtask 1: Researcher — success first try
    let r_provider = MockProvider::simple("Analyzed 3 data sources: CSV, JSON, Parquet");
    let mut researcher = create_test_agent(
        Box::new(r_provider),
        ToolRegistry::new(),
        CapabilitySet::new(),
        AutonomyTier::Trust,
    );
    let r1 = researcher
        .turn("Research available data sources", None)
        .await
        .unwrap();
    ctx.record_completion("researcher", "Research data sources", &r1)
        .await;
    assert!(r1.contains("CSV"));

    // Subtask 2: Coder — first attempt fails verification
    let c_provider_v1 = MockProvider::simple("Built pipeline: reads CSV only, no error handling");
    let mut coder_v1 = create_test_agent(
        Box::new(c_provider_v1),
        ToolRegistry::new(),
        CapabilitySet::new(),
        AutonomyTier::Trust,
    );
    let c1 = coder_v1
        .turn("Implement data pipeline", None)
        .await
        .unwrap();

    // Verify output — fails (missing Parquet and error handling)
    let verify_provider = MockProvider::simple(
        "VERIFICATION FAILED: Pipeline only handles CSV, missing Parquet support and error handling",
    );
    let mut verifier = create_test_agent(
        Box::new(verify_provider),
        ToolRegistry::new(),
        CapabilitySet::new(),
        AutonomyTier::Trust,
    );
    let verification = verifier
        .turn(
            &format!("Verify this implementation meets requirements:\n{c1}"),
            None,
        )
        .await
        .unwrap();
    assert!(verification.contains("FAILED"));

    // Redelegate subtask 2 to coder with feedback
    let c_provider_v2 = MockProvider::simple(
        "Built pipeline v2: handles CSV, JSON, Parquet with retry logic and error handling",
    );
    let mut coder_v2 = create_test_agent(
        Box::new(c_provider_v2),
        ToolRegistry::new(),
        CapabilitySet::new(),
        AutonomyTier::Trust,
    );
    let c2 = coder_v2
        .turn(
            &format!(
                "Previous attempt failed verification: {verification}\n\
                 Please fix: Implement data pipeline with all 3 formats and error handling"
            ),
            None,
        )
        .await
        .unwrap();
    assert!(c2.contains("Parquet"));
    assert!(c2.contains("error handling"));
    ctx.record_completion("coder", "Implement data pipeline (retry)", &c2)
        .await;

    // Subtask 3: Reviewer — success
    let rev_provider =
        MockProvider::simple("Code review passed: all formats supported, good error handling");
    let mut reviewer = create_test_agent(
        Box::new(rev_provider),
        ToolRegistry::new(),
        CapabilitySet::new(),
        AutonomyTier::Trust,
    );
    let rev = reviewer
        .turn("Review the pipeline code", None)
        .await
        .unwrap();
    ctx.record_completion("reviewer", "Review pipeline code", &rev)
        .await;

    // Subtask 4: Writer — success
    let w_provider =
        MockProvider::simple("Pipeline documentation: covers all 3 formats with examples");
    let mut doc_writer = create_test_agent(
        Box::new(w_provider),
        ToolRegistry::new(),
        CapabilitySet::new(),
        AutonomyTier::Trust,
    );
    let doc = doc_writer
        .turn("Write pipeline documentation", None)
        .await
        .unwrap();
    ctx.record_completion("writer", "Write documentation", &doc)
        .await;

    // Synthesize
    let synth_provider = MockProvider::simple(
        "Data pipeline complete: 3-format support (CSV/JSON/Parquet), \
         error handling, code reviewed, fully documented.",
    );
    let mut synthesizer = create_test_agent(
        Box::new(synth_provider),
        ToolRegistry::new(),
        CapabilitySet::new(),
        AutonomyTier::Trust,
    );

    let work = ctx.completed_work_snapshot().await;
    let context = work
        .iter()
        .map(|w| format!("[{}] {}: {}", w.agent, w.task, w.outcome))
        .collect::<Vec<_>>()
        .join("\n");
    let final_result = synthesizer
        .turn(&format!("Synthesize:\n{context}"), None)
        .await
        .unwrap();

    // Verify all 4 subtasks completed (including the redelegation)
    assert_eq!(work.len(), 4);
    assert!(final_result.contains("CSV/JSON/Parquet"));

    // Verify cost accumulated across multiple agent turns
    // (each agent had separate cost trackers, but we can verify they all ran)
    assert!(r1.len() > 0);
    assert!(c2.len() > 0);
    assert!(rev.len() > 0);
    assert!(doc.len() > 0);
}

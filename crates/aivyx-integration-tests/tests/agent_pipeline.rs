//! Integration tests for the agent pipeline: turn loop, tool execution,
//! streaming, capability enforcement, retry, cost tracking, and session persistence.

use aivyx_agent::built_in_tools::FileReadTool;
use aivyx_capability::CapabilitySet;
use aivyx_core::{AgentId, AutonomyTier, ToolRegistry};
use aivyx_integration_tests::{
    FailThenSucceedProvider, MockProvider, create_filesystem_caps, create_test_agent,
};
use aivyx_llm::{ChatMessage, ChatResponse, StopReason, TokenUsage, ToolCall};

#[tokio::test]
async fn full_turn_with_tool_use() {
    // Agent calls file_read, gets content, then produces a final text response.
    // We can't actually read a real file in tests, so the tool will error on
    // a non-existent file — but the agent should handle the error tool result
    // and produce a final response.
    let agent_id = AgentId::new();
    let caps = create_filesystem_caps(agent_id);

    let mut tools = ToolRegistry::new();
    tools.register(Box::new(FileReadTool::new()));

    let tool_call = ToolCall {
        id: "tc_1".into(),
        name: "file_read".into(),
        arguments: serde_json::json!({"path": "/tmp/nonexistent-aivyx-test-file"}),
    };

    let provider = MockProvider::new(vec![
        ChatResponse {
            message: ChatMessage::assistant_with_tool_calls("Reading file", vec![tool_call]),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
            stop_reason: StopReason::ToolUse,
        },
        ChatResponse {
            message: ChatMessage::assistant("File not found, sorry."),
            usage: TokenUsage {
                input_tokens: 20,
                output_tokens: 8,
            },
            stop_reason: StopReason::EndTurn,
        },
    ]);

    let mut agent = aivyx_agent::Agent::new(
        agent_id,
        "test".into(),
        "Test agent".into(),
        4096,
        AutonomyTier::Trust,
        Box::new(provider),
        tools,
        caps,
        aivyx_agent::RateLimiter::new(60),
        aivyx_agent::CostTracker::new(5.0, 0.000003, 0.000015),
        None,
        3,
        1,
    );

    let result = agent.turn("Read a file for me", None).await.unwrap();
    assert_eq!(result, "File not found, sorry.");

    // Should have: user msg, assistant (tool call), tool result, assistant (final)
    assert_eq!(agent.conversation().len(), 4);
}

#[tokio::test]
async fn multi_turn_conversation_preserves_context() {
    let provider = MockProvider::new(vec![
        ChatResponse {
            message: ChatMessage::assistant("Hello! I'm ready."),
            usage: TokenUsage {
                input_tokens: 5,
                output_tokens: 4,
            },
            stop_reason: StopReason::EndTurn,
        },
        ChatResponse {
            message: ChatMessage::assistant("I remember you greeted me."),
            usage: TokenUsage {
                input_tokens: 15,
                output_tokens: 6,
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

    let r1 = agent.turn("Hello!", None).await.unwrap();
    assert_eq!(r1, "Hello! I'm ready.");
    assert_eq!(agent.conversation().len(), 2);

    let r2 = agent.turn("What did I say?", None).await.unwrap();
    assert_eq!(r2, "I remember you greeted me.");
    assert_eq!(agent.conversation().len(), 4);
}

#[tokio::test]
async fn cost_tracking_accumulates_across_turns() {
    let provider = MockProvider::new(vec![ChatResponse {
        message: ChatMessage::assistant("ok"),
        usage: TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
        },
        stop_reason: StopReason::EndTurn,
    }]);

    let mut agent = create_test_agent(
        Box::new(provider),
        ToolRegistry::new(),
        CapabilitySet::new(),
        AutonomyTier::Trust,
    );

    agent.turn("turn 1", None).await.unwrap();
    let cost1 = agent.current_cost_usd();
    assert!(cost1 > 0.0);

    agent.turn("turn 2", None).await.unwrap();
    let cost2 = agent.current_cost_usd();
    assert!(cost2 > cost1, "cost should accumulate across turns");
}

#[tokio::test]
async fn capability_enforcement_blocks_unauthorized_tool() {
    let mut tools = ToolRegistry::new();
    tools.register(Box::new(FileReadTool::new()));

    // Agent has Filesystem tool but NO capabilities
    let tool_call = ToolCall {
        id: "tc_1".into(),
        name: "file_read".into(),
        arguments: serde_json::json!({"path": "/tmp/test"}),
    };

    let provider = MockProvider::new(vec![
        ChatResponse {
            message: ChatMessage::assistant_with_tool_calls("Read", vec![tool_call]),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
            stop_reason: StopReason::ToolUse,
        },
        ChatResponse {
            message: ChatMessage::assistant("Denied"),
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
        CapabilitySet::new(), // empty — should deny
        AutonomyTier::Trust,
    );

    let result = agent.turn("read file", None).await.unwrap();
    assert_eq!(result, "Denied");

    // The tool result should contain a capability denied error
    let tool_msg = agent
        .conversation()
        .iter()
        .find(|m| m.tool_result.is_some())
        .unwrap();
    let tr = tool_msg.tool_result.as_ref().unwrap();
    assert!(tr.is_error);
    assert!(tr.content.to_string().contains("capability denied"));
}

#[tokio::test]
async fn retry_recovers_from_transient_failures() {
    let provider = FailThenSucceedProvider::new(2);

    let mut agent = create_test_agent(
        Box::new(provider),
        ToolRegistry::new(),
        CapabilitySet::new(),
        AutonomyTier::Trust,
    );

    let result = agent.turn("hi", None).await.unwrap();
    assert_eq!(result, "recovered");
}

#[tokio::test]
async fn streaming_delivers_same_text_as_non_streaming() {
    let text = "Hello from stream!";

    let provider1 = MockProvider::simple(text);
    let provider2 = MockProvider::simple(text);

    let mut agent1 = create_test_agent(
        Box::new(provider1),
        ToolRegistry::new(),
        CapabilitySet::new(),
        AutonomyTier::Trust,
    );
    let mut agent2 = create_test_agent(
        Box::new(provider2),
        ToolRegistry::new(),
        CapabilitySet::new(),
        AutonomyTier::Trust,
    );

    let non_stream_result = agent1.turn("hi", None).await.unwrap();

    let (token_tx, mut token_rx) = tokio::sync::mpsc::channel(64);
    let stream_result = agent2
        .turn_stream("hi", None, token_tx, None)
        .await
        .unwrap();

    assert_eq!(non_stream_result, stream_result);

    // Collect streamed tokens
    let mut collected = String::new();
    while let Ok(token) = token_rx.try_recv() {
        collected.push_str(&token);
    }
    assert_eq!(collected, text);
}

#[tokio::test]
async fn session_save_load_roundtrip() {
    let provider = MockProvider::new(vec![
        ChatResponse {
            message: ChatMessage::assistant("First response"),
            usage: TokenUsage {
                input_tokens: 5,
                output_tokens: 3,
            },
            stop_reason: StopReason::EndTurn,
        },
        ChatResponse {
            message: ChatMessage::assistant("Second response"),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 3,
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

    agent.turn("msg1", None).await.unwrap();
    agent.turn("msg2", None).await.unwrap();

    // Export session
    let persisted = agent.to_persisted_session();
    assert_eq!(persisted.messages.len(), 4); // 2 user + 2 assistant

    // Create a new agent and restore
    let provider2 = MockProvider::simple("Restored");
    let mut agent2 = create_test_agent(
        Box::new(provider2),
        ToolRegistry::new(),
        CapabilitySet::new(),
        AutonomyTier::Trust,
    );

    agent2.restore_conversation(persisted.messages.clone());
    assert_eq!(agent2.conversation().len(), 4);

    // New turn builds on restored conversation
    let result = agent2.turn("msg3", None).await.unwrap();
    assert_eq!(result, "Restored");
    assert_eq!(agent2.conversation().len(), 6); // 4 restored + 1 user + 1 assistant
}

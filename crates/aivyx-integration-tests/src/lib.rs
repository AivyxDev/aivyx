//! Shared test helpers for cross-crate integration tests.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use aivyx_agent::{Agent, CostTracker, RateLimiter};
use aivyx_capability::{ActionPattern, Capability, CapabilitySet};
use aivyx_core::{
    AgentId, AutonomyTier, CapabilityId, CapabilityScope, ChannelAdapter, Principal, ProgressSink,
    Result, ToolRegistry,
};
use aivyx_llm::{
    ChatMessage, ChatRequest, ChatResponse, LlmProvider, StopReason, TokenUsage, ToolCall,
};
use aivyx_task::progress::ProgressEvent;
use chrono::Utc;

/// A mock LLM provider that returns a fixed sequence of responses.
pub struct MockProvider {
    responses: Mutex<Vec<ChatResponse>>,
}

impl MockProvider {
    /// Create a mock that cycles through the given responses.
    /// When only one response remains, it will be returned for all subsequent calls.
    pub fn new(responses: Vec<ChatResponse>) -> Self {
        Self {
            responses: Mutex::new(responses),
        }
    }

    /// Create a mock that returns a single text response.
    pub fn simple(text: &str) -> Self {
        Self::new(vec![ChatResponse {
            message: ChatMessage::assistant(text),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
            stop_reason: StopReason::EndTurn,
        }])
    }

    /// Create a mock that returns a tool call on first invocation,
    /// then a text response on the second.
    pub fn tool_then_text(tool_name: &str, tool_args: serde_json::Value, text: &str) -> Self {
        let tool_call = ToolCall {
            id: "tc_1".into(),
            name: tool_name.to_string(),
            arguments: tool_args,
        };
        Self::new(vec![
            ChatResponse {
                message: ChatMessage::assistant_with_tool_calls("", vec![tool_call]),
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                },
                stop_reason: StopReason::ToolUse,
            },
            ChatResponse {
                message: ChatMessage::assistant(text),
                usage: TokenUsage {
                    input_tokens: 15,
                    output_tokens: 5,
                },
                stop_reason: StopReason::EndTurn,
            },
        ])
    }
}

#[async_trait::async_trait]
impl LlmProvider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }

    async fn chat(&self, _request: &ChatRequest) -> Result<ChatResponse> {
        let mut responses = self.responses.lock().expect("MockProvider mutex poisoned");
        if responses.len() > 1 {
            Ok(responses.remove(0))
        } else {
            Ok(responses[0].clone())
        }
    }
}

/// A mock LLM provider that fails a configurable number of times, then succeeds.
pub struct FailThenSucceedProvider {
    call_count: AtomicU32,
    fail_count: u32,
    success_response: ChatResponse,
}

impl FailThenSucceedProvider {
    pub fn new(fail_count: u32) -> Self {
        Self {
            call_count: AtomicU32::new(0),
            fail_count,
            success_response: ChatResponse {
                message: ChatMessage::assistant("recovered"),
                usage: TokenUsage {
                    input_tokens: 5,
                    output_tokens: 2,
                },
                stop_reason: StopReason::EndTurn,
            },
        }
    }
}

#[async_trait::async_trait]
impl LlmProvider for FailThenSucceedProvider {
    fn name(&self) -> &str {
        "mock-flaky"
    }

    async fn chat(&self, _request: &ChatRequest) -> Result<ChatResponse> {
        let n = self.call_count.fetch_add(1, Ordering::SeqCst);
        if n < self.fail_count {
            Err(aivyx_core::AivyxError::RateLimit("rate limited".into()))
        } else {
            Ok(self.success_response.clone())
        }
    }
}

/// Create a test agent with a mock provider and optional tools/capabilities.
pub fn create_test_agent(
    provider: Box<dyn LlmProvider>,
    tools: ToolRegistry,
    capabilities: CapabilitySet,
    autonomy_tier: AutonomyTier,
) -> Agent {
    Agent::new(
        AgentId::new(),
        "test-agent".into(),
        "You are a helpful test agent.".into(),
        4096,
        autonomy_tier,
        provider,
        tools,
        capabilities,
        RateLimiter::new(60),
        CostTracker::new(5.0, 0.000003, 0.000015),
        None,
        3,
        1, // 1ms retry delay for fast tests
    )
}

/// Create a filesystem capability set granted to the given agent ID.
pub fn create_filesystem_caps(agent_id: AgentId) -> CapabilitySet {
    let mut caps = CapabilitySet::new();
    caps.grant(Capability {
        id: CapabilityId::new(),
        scope: CapabilityScope::Filesystem {
            root: PathBuf::from("/"),
        },
        pattern: ActionPattern::new("*").expect("wildcard pattern should always parse"),
        granted_to: vec![Principal::Agent(agent_id)],
        granted_by: Principal::System,
        created_at: Utc::now(),
        expires_at: None,
        revoked: false,
        parent_id: None,
    });
    caps
}

/// Create a memory capability set for memory tools.
pub fn create_memory_caps(agent_id: AgentId) -> CapabilitySet {
    let mut caps = CapabilitySet::new();
    caps.grant(Capability {
        id: CapabilityId::new(),
        scope: CapabilityScope::Custom("memory".into()),
        pattern: ActionPattern::new("*").expect("wildcard pattern should always parse"),
        granted_to: vec![Principal::Agent(agent_id)],
        granted_by: Principal::System,
        created_at: Utc::now(),
        expires_at: None,
        revoked: false,
        parent_id: None,
    });
    caps
}

/// Create a coordination capability set for team delegation tools.
pub fn create_coordination_caps(agent_id: AgentId) -> CapabilitySet {
    let mut caps = CapabilitySet::new();
    caps.grant(Capability {
        id: CapabilityId::new(),
        scope: CapabilityScope::Custom("coordination".into()),
        pattern: ActionPattern::new("*").expect("wildcard pattern should always parse"),
        granted_to: vec![Principal::Agent(agent_id)],
        granted_by: Principal::System,
        created_at: Utc::now(),
        expires_at: None,
        revoked: false,
        parent_id: None,
    });
    caps
}

/// A mock embedding provider that returns deterministic vectors based on
/// a simple hash of the input text. Tracks call count for cache verification.
pub struct MockEmbeddingProvider {
    dims: usize,
    call_count: AtomicUsize,
}

impl MockEmbeddingProvider {
    /// Create a new mock embedding provider with the given dimensions.
    pub fn new(dims: usize) -> Self {
        Self {
            dims,
            call_count: AtomicUsize::new(0),
        }
    }

    /// Get the number of embed calls made.
    pub fn calls(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl aivyx_llm::EmbeddingProvider for MockEmbeddingProvider {
    fn name(&self) -> &str {
        "mock"
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    async fn embed(&self, text: &str) -> aivyx_core::Result<aivyx_llm::Embedding> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        // Generate a deterministic vector from the text
        let mut vector = vec![0.0_f32; self.dims];
        for (i, byte) in text.bytes().enumerate() {
            vector[i % self.dims] += byte as f32 / 255.0;
        }
        // Normalize
        let norm: f32 = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in &mut vector {
                *v /= norm;
            }
        }
        Ok(aivyx_llm::Embedding {
            vector,
            dimensions: self.dims,
        })
    }
}

/// A mock channel adapter that auto-approves or auto-denies tool calls.
///
/// Used to test the Leash autonomy tier where the agent must prompt the user
/// for approval before executing tools.
pub struct MockChannelAdapter {
    approve: bool,
    calls: AtomicUsize,
    /// Messages sent through the channel (for verification).
    sent: Mutex<Vec<String>>,
}

impl MockChannelAdapter {
    /// Create a mock that auto-approves all tool execution requests.
    pub fn approving() -> Self {
        Self {
            approve: true,
            calls: AtomicUsize::new(0),
            sent: Mutex::new(Vec::new()),
        }
    }

    /// Create a mock that auto-denies all tool execution requests.
    pub fn denying() -> Self {
        Self {
            approve: false,
            calls: AtomicUsize::new(0),
            sent: Mutex::new(Vec::new()),
        }
    }

    /// Number of times `receive()` was called.
    pub fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// Messages sent via `send()`.
    pub fn sent_messages(&self) -> Vec<String> {
        self.sent.lock().expect("lock").clone()
    }
}

#[async_trait::async_trait]
impl ChannelAdapter for MockChannelAdapter {
    async fn send(&self, message: &str) -> Result<()> {
        self.sent.lock().expect("lock").push(message.to_string());
        Ok(())
    }

    async fn receive(&self) -> Result<String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(if self.approve { "y" } else { "n" }.into())
    }
}

/// Collects progress events for test assertions.
///
/// Implements `ProgressSink<ProgressEvent>` and stores all emitted events
/// in an internal vec for later inspection.
pub struct MockProgressSink {
    events: tokio::sync::Mutex<Vec<ProgressEvent>>,
}

impl Default for MockProgressSink {
    fn default() -> Self {
        Self::new()
    }
}

impl MockProgressSink {
    /// Create a new empty progress sink.
    pub fn new() -> Self {
        Self {
            events: tokio::sync::Mutex::new(Vec::new()),
        }
    }

    /// Return a snapshot of all collected events.
    pub async fn events(&self) -> Vec<ProgressEvent> {
        self.events.lock().await.clone()
    }
}

#[async_trait::async_trait]
impl ProgressSink<ProgressEvent> for MockProgressSink {
    async fn emit(&self, event: ProgressEvent) -> Result<()> {
        self.events.lock().await.push(event);
        Ok(())
    }
}

/// Create a shell capability set granted to the given agent ID.
pub fn create_shell_caps(agent_id: AgentId) -> CapabilitySet {
    let mut caps = CapabilitySet::new();
    caps.grant(Capability {
        id: CapabilityId::new(),
        scope: CapabilityScope::Shell {
            allowed_commands: vec![],
        },
        pattern: ActionPattern::new("*").expect("wildcard pattern should always parse"),
        granted_to: vec![Principal::Agent(agent_id)],
        granted_by: Principal::System,
        created_at: Utc::now(),
        expires_at: None,
        revoked: false,
        parent_id: None,
    });
    caps
}

/// Create a network capability set granted to the given agent ID.
pub fn create_network_caps(agent_id: AgentId) -> CapabilitySet {
    let mut caps = CapabilitySet::new();
    caps.grant(Capability {
        id: CapabilityId::new(),
        scope: CapabilityScope::Network {
            hosts: vec![],
            ports: vec![],
        },
        pattern: ActionPattern::new("*").expect("wildcard pattern should always parse"),
        granted_to: vec![Principal::Agent(agent_id)],
        granted_by: Principal::System,
        created_at: Utc::now(),
        expires_at: None,
        revoked: false,
        parent_id: None,
    });
    caps
}

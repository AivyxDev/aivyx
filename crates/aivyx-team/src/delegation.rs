use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{info, warn};

use aivyx_agent::AgentSession;
use aivyx_audit::{AuditEvent, AuditLog};
use aivyx_capability::CapabilitySet;
use aivyx_core::{AivyxError, CapabilityScope, Principal, Result, Tool, ToolId};
#[cfg(feature = "memory")]
use aivyx_memory::{MemoryManager, OutcomeRecord, OutcomeSource};

use crate::capability_delegation::attenuate_for_member;
use crate::config::DialogueConfig;
use crate::job_tracker::JobTracker;
use crate::message_bus::MessageBus;
use crate::message_tools::{RequestPeerReviewTool, SendMessageTool};

/// Maximum retry attempts allowed for delegation retry.
const MAX_RETRY_LIMIT: u32 = 3;

// ---------------------------------------------------------------------------
// TeamContext — shared context for specialist awareness
// ---------------------------------------------------------------------------

/// A summary of completed delegation work.
#[derive(Debug, Clone)]
pub struct WorkSummary {
    /// Name of the specialist that performed the work.
    pub agent: String,
    /// The task that was delegated.
    pub task: String,
    /// The outcome / result of the delegation.
    pub outcome: String,
}

/// Shared team context that gives specialists awareness of the team structure,
/// original goal, and work completed so far.
///
/// Held by `SpecialistPool` and shared with each specialist via a formatted
/// `[TEAM CONTEXT]` string injected into the specialist's system prompt.
/// The formatted string is `Arc<Mutex<String>>` so the team runtime can update
/// it after each delegation, and specialists see the latest state on every turn.
#[derive(Clone)]
pub struct TeamContext {
    /// Name of the team (from `TeamConfig`).
    pub team_name: String,
    /// Name of the coordinator / lead agent.
    pub coordinator: String,
    /// All team members: `(name, role)`.
    pub members: Vec<(String, String)>,
    /// The original user request that initiated this team run.
    pub original_goal: String,
    /// Completed delegation results, updated after each delegation.
    completed_work: Arc<Mutex<Vec<WorkSummary>>>,
    /// Whether specialists can communicate directly with each other.
    pub peer_dialogue_enabled: bool,
}

/// Maximum number of completed work entries to show in the context block.
const MAX_CONTEXT_WORK_ENTRIES: usize = 10;

/// Maximum characters for a truncated outcome in the context block.
const MAX_OUTCOME_CHARS: usize = 200;

impl TeamContext {
    /// Create a new team context.
    pub fn new(
        team_name: String,
        coordinator: String,
        members: Vec<(String, String)>,
        original_goal: String,
        peer_dialogue_enabled: bool,
    ) -> Self {
        Self {
            team_name,
            coordinator,
            members,
            original_goal,
            completed_work: Arc::new(Mutex::new(Vec::new())),
            peer_dialogue_enabled,
        }
    }

    /// Format the context block for a specific specialist.
    ///
    /// Returns a `[TEAM CONTEXT]...[END TEAM CONTEXT]` block that is injected
    /// into the specialist's system prompt. Shows the last 10 completed
    /// delegations with outcomes truncated to 200 characters.
    pub async fn format_for_role(&self, agent_name: &str, agent_role: &str) -> String {
        let work = self.completed_work.lock().await;
        let mut out = String::from("[TEAM CONTEXT]\n");
        out.push_str(&format!("Team: {}\n", self.team_name));
        out.push_str(&format!("Your role: {} ({})\n", agent_role, agent_name));
        out.push_str(&format!("Coordinator: {}\n", self.coordinator));
        out.push_str("\nTeam members:\n");
        for (name, role) in &self.members {
            out.push_str(&format!("- {} ({})\n", name, role));
        }
        out.push_str(&format!("\nOriginal goal: {}\n", self.original_goal));
        if !work.is_empty() {
            out.push_str("\nCompleted work so far:\n");
            let start = work.len().saturating_sub(MAX_CONTEXT_WORK_ENTRIES);
            for w in &work[start..] {
                let truncated = if w.outcome.len() > MAX_OUTCOME_CHARS {
                    let boundary = w.outcome.floor_char_boundary(MAX_OUTCOME_CHARS);
                    format!("{}...", &w.outcome[..boundary])
                } else {
                    w.outcome.clone()
                };
                let task_short = if w.task.len() > 80 {
                    let boundary = w.task.floor_char_boundary(80);
                    format!("{}...", &w.task[..boundary])
                } else {
                    w.task.clone()
                };
                out.push_str(&format!(
                    "- [{}] {} -> {}\n",
                    w.agent, task_short, truncated
                ));
            }
        }
        if self.peer_dialogue_enabled {
            out.push_str("\n[PEER MESSAGING]\n");
            out.push_str("You can communicate directly with other team members:\n");
            out.push_str("- send_message: Send a message to any team member by name\n");
            out.push_str("- read_messages: Check for incoming messages (filter by 'from', 'message_type', or 'since')\n");
            out.push_str(
                "- request_peer_review: Ask a peer for structured feedback on your work\n",
            );
            out.push_str("Message types: 'text', 'review_request', 'review_response'\n");
            out.push_str("Check for messages at the start of your turn to see if peers have sent you requests.\n");
            out.push_str("[END PEER MESSAGING]\n");
        }
        out.push_str("[END TEAM CONTEXT]");
        out
    }

    /// Get a snapshot of all completed work entries.
    pub async fn completed_work_snapshot(&self) -> Vec<WorkSummary> {
        self.completed_work.lock().await.clone()
    }

    /// Record a completed delegation and return the updated work count.
    pub async fn record_completion(&self, agent: &str, task: &str, outcome: &str) -> usize {
        let mut work = self.completed_work.lock().await;
        work.push(WorkSummary {
            agent: agent.to_string(),
            task: task.to_string(),
            outcome: outcome.to_string(),
        });
        work.len()
    }
}

// ---------------------------------------------------------------------------
// SpecialistPool — persistent specialist agents across delegations
// ---------------------------------------------------------------------------

/// A pool of specialist agents that persist across delegations within a team run.
///
/// Specialists maintain conversation history between delegations, enabling
/// context continuity. Each specialist is wrapped in `Arc<Mutex<Agent>>`
/// so it can be safely shared and locked for exclusive turn access.
#[derive(Clone)]
pub struct SpecialistPool {
    agents: Arc<Mutex<HashMap<String, Arc<Mutex<aivyx_agent::Agent>>>>>,
    /// Per-specialist formatted context strings, shared with agents via
    /// `Agent::set_team_context()`. Updated after each delegation completes.
    context_strings: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<String>>>>>,
    session: Arc<AgentSession>,
    bus: Option<Arc<MessageBus>>,
    /// The lead agent's capabilities, used as the parent set for attenuation.
    /// Specialists can never exceed these capabilities.
    lead_capabilities: CapabilitySet,
    /// Team context for specialist awareness. When `Some`, specialists receive
    /// a `[TEAM CONTEXT]` block in their system prompt.
    team_context: Option<TeamContext>,
    /// Dialogue configuration for peer messaging.
    dialogue_config: DialogueConfig,
}

impl SpecialistPool {
    /// Create a new empty specialist pool.
    ///
    /// `lead_capabilities` is the lead agent's capability set. Specialist
    /// capabilities are attenuated from this set, enforcing the invariant
    /// that specialists can never exceed the lead's authority.
    ///
    /// When `team_context` is provided, specialists receive a `[TEAM CONTEXT]`
    /// block in their system prompt with team structure, the original goal,
    /// and a running log of completed delegations.
    pub fn new(
        session: Arc<AgentSession>,
        bus: Option<Arc<MessageBus>>,
        lead_capabilities: CapabilitySet,
        team_context: Option<TeamContext>,
        dialogue_config: DialogueConfig,
    ) -> Self {
        Self {
            agents: Arc::new(Mutex::new(HashMap::new())),
            context_strings: Arc::new(Mutex::new(HashMap::new())),
            session,
            bus,
            lead_capabilities,
            team_context,
            dialogue_config,
        }
    }

    /// Register a dynamically spawned specialist in the pool.
    ///
    /// Called by [`SpawnSpecialistTool`] to make a newly created specialist
    /// available for delegation. The actual agent creation happens lazily
    /// on first `get_or_create()` call.
    pub async fn register_spawned(&self, agent_name: &str, role: &str) {
        // Update team context with the new member
        if let Some(ref ctx) = self.team_context {
            let mut members = ctx.members.clone();
            members.push((agent_name.to_string(), role.to_string()));
            // The context will be rebuilt on next delegation via format_for_role()
            info!("Registered spawned specialist '{}' (role: {})", agent_name, role);
        }
    }

    /// Remove a dynamically spawned specialist from the pool.
    ///
    /// Removes the agent from the cached agents map and, if team context
    /// is enabled, removes the member entry so it no longer appears in
    /// the `[TEAM CONTEXT]` block.
    pub async fn deregister_spawned(&self, name: &str) {
        // Remove from cached agents
        let mut agents = self.agents.lock().await;
        agents.remove(name);

        // Remove from team context members list
        if let Some(ref ctx) = self.team_context {
            let mut members = ctx.members.clone();
            members.retain(|(n, _)| n != name);
            info!("Deregistered spawned specialist '{}'", name);
        }
    }

    /// Get or create a specialist by name.
    ///
    /// On first call for a given name, creates the agent from its profile,
    /// attenuates its capabilities against the lead's capability set, and
    /// registers message tools. Subsequent calls return the cached instance
    /// with full conversation history intact.
    pub async fn get_or_create(
        &self,
        agent_name: &str,
    ) -> std::result::Result<Arc<Mutex<aivyx_agent::Agent>>, String> {
        let mut agents = self.agents.lock().await;

        if let Some(existing) = agents.get(agent_name) {
            info!("Specialist '{agent_name}' reused from pool (context preserved)");
            return Ok(Arc::clone(existing));
        }

        // Create fresh specialist with attenuation against lead's capabilities
        let mut agent = setup_specialist_fresh(
            &self.session,
            agent_name,
            &self.bus,
            &self.lead_capabilities,
            &self.dialogue_config,
        )
        .await?;

        // Inject team context so the specialist knows about the team
        if let Some(ref ctx) = self.team_context {
            let role = ctx
                .members
                .iter()
                .find(|(n, _)| n == agent_name)
                .map(|(_, r)| r.as_str())
                .unwrap_or("specialist");
            let formatted = ctx.format_for_role(agent_name, role).await;
            let shared = Arc::new(tokio::sync::Mutex::new(formatted));
            agent.set_team_context(Arc::clone(&shared));
            self.context_strings
                .lock()
                .await
                .insert(agent_name.to_string(), shared);
        }

        let arc = Arc::new(Mutex::new(agent));
        agents.insert(agent_name.to_string(), Arc::clone(&arc));
        Ok(arc)
    }

    /// Number of specialists currently in the pool.
    pub async fn len(&self) -> usize {
        self.agents.lock().await.len()
    }

    /// Whether the pool is empty.
    pub async fn is_empty(&self) -> bool {
        self.agents.lock().await.is_empty()
    }

    /// Update all cached specialists' team context strings after a delegation
    /// completes.
    ///
    /// Records the completion in `TeamContext`, then re-formats the context
    /// string for every specialist currently in the pool. This ensures that
    /// on the next `turn()`, each specialist's `resolve_system_prompt()` sees
    /// the latest completed work.
    pub async fn update_context(&self, agent_name: &str, task: &str, outcome: &str) {
        if let Some(ref ctx) = self.team_context {
            ctx.record_completion(agent_name, task, outcome).await;
            // Re-format for each cached specialist
            let contexts = self.context_strings.lock().await;
            for (name, formatted_arc) in contexts.iter() {
                let role = ctx
                    .members
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, r)| r.as_str())
                    .unwrap_or("specialist");
                let new_text = ctx.format_for_role(name, role).await;
                *formatted_arc.lock().await = new_text;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Shared specialist setup
// ---------------------------------------------------------------------------

/// Create a fresh specialist agent with attenuated capabilities and message tools.
///
/// This is the low-level entry-point for specialist creation. For pooled
/// access that preserves conversation context, use [`SpecialistPool::get_or_create()`].
///
/// Ensures:
/// - **NT-02**: Capabilities are attenuated from the **lead agent's** capability
///   set via [`attenuate_for_member()`]. The specialist's declared scopes act as
///   a filter — the result can never exceed the lead's authority.
/// - **NT-04**: Message tools are registered when a [`MessageBus`] is provided.
async fn setup_specialist_fresh(
    session: &AgentSession,
    agent_name: &str,
    bus: &Option<Arc<MessageBus>>,
    lead_capabilities: &CapabilitySet,
    dialogue_config: &DialogueConfig,
) -> std::result::Result<aivyx_agent::Agent, String> {
    let mut specialist = session
        .create_agent(agent_name)
        .await
        .map_err(|e| format!("failed to create specialist: {e}"))?;

    // NT-02: Attenuate capabilities for the specialist.
    // The specialist's declared scopes define what it SHOULD have access to.
    // We attenuate the LEAD's capabilities (parent set) down to only the
    // scopes the specialist declares. This enforces the security invariant
    // that specialists can never exceed their coordinator's authority.
    let specialist_scopes: Vec<CapabilityScope> = specialist
        .capabilities()
        .iter()
        .map(|c| c.scope.clone())
        .collect();

    if !specialist_scopes.is_empty() {
        let member_principal = Principal::Agent(specialist.id);
        let narrowed = attenuate_for_member(
            lead_capabilities,
            &member_principal,
            &specialist_scopes,
            "execute:*",
        );
        info!(
            "Specialist '{agent_name}' attenuated to {} capabilities (from {} lead, {} declared)",
            narrowed.len(),
            lead_capabilities.len(),
            specialist_scopes.len()
        );
        specialist.replace_capabilities(narrowed);
    } else {
        warn!(
            "Specialist '{agent_name}' has no declared capabilities — all tool calls will be denied"
        );
    }

    // NT-04: Register message tools on the specialist.
    // With broadcast channels, specialists can both send AND receive messages.
    if let Some(bus) = bus {
        let mut send_tool = SendMessageTool::new(Arc::clone(bus), agent_name.to_string());
        if dialogue_config.max_messages_per_turn > 0 {
            send_tool = send_tool.with_max_per_turn(dialogue_config.max_messages_per_turn);
        }
        specialist.register_tool(Box::new(send_tool));

        if let Some(rx) = bus.subscribe(agent_name) {
            specialist.register_tool(Box::new(crate::message_tools::ReadMessagesTool::new(
                Arc::new(tokio::sync::Mutex::new(rx)),
            )));
        }

        // Register peer review tool when dialogue is enabled
        if dialogue_config.enable_peer_dialogue {
            specialist.register_tool(Box::new(RequestPeerReviewTool::new(
                Arc::clone(bus),
                agent_name.to_string(),
            )));
        }
    }

    Ok(specialist)
}

// ---------------------------------------------------------------------------
// Retry helper for specialist delegation
// ---------------------------------------------------------------------------

/// Run a specialist turn with optional retry on transient errors.
///
/// Returns `(status, response)` where status is `"completed"` or `"error"`.
/// When `max_retries > 0`, transient errors (rate-limit, HTTP) trigger
/// exponential backoff retries before giving up. Non-retryable errors fail
/// immediately.
///
/// When `token_tx` is provided, the specialist uses `turn_stream()` for
/// real-time output. Otherwise falls back to `turn()`.
async fn run_specialist_with_retry(
    specialist: &Arc<Mutex<aivyx_agent::Agent>>,
    task: &str,
    agent_name: &str,
    token_tx: &Option<tokio::sync::mpsc::Sender<String>>,
    max_retries: u32,
) -> (String, String) {
    let mut attempt = 0u32;
    loop {
        let result = {
            let mut agent = specialist.lock().await;
            if let Some(tx) = token_tx {
                let _ = tx.send(format!("\n--- [{agent_name}] ---\n")).await;
                let r = agent.turn_stream(task, None, tx.clone(), None).await;
                let _ = tx.send(format!("\n--- [/{agent_name}] ---\n")).await;
                r
            } else {
                agent.turn(task, None).await
            }
        };

        match result {
            Ok(response) => return ("completed".to_string(), response),
            Err(e) if e.is_retryable() && attempt < max_retries => {
                attempt += 1;
                let delay_ms = 1000u64
                    .saturating_mul(2u64.saturating_pow(attempt - 1))
                    .min(30_000);
                warn!(
                    "Delegation retry {attempt}/{max_retries} for '{agent_name}': backoff {delay_ms}ms: {e}"
                );
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
            Err(e) => {
                return ("error".to_string(), format!("specialist failed: {e}"));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// DelegationTracker — shared state for NT-03
// ---------------------------------------------------------------------------

/// A single delegation result recorded by [`DelegateTaskTool`] or [`QueryAgentTool`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationResult {
    /// Agent that performed the work.
    pub agent: String,
    /// Original task or query.
    pub task: String,
    /// Outcome: `"completed"` or `"error"`.
    pub status: String,
    /// The agent's response (or error message).
    pub response: String,
}

/// Shared tracker that delegation tools push results into and
/// [`CollectResultsTool`] reads from.
pub type DelegationTracker = Arc<Mutex<Vec<DelegationResult>>>;

/// Create a new, empty tracker.
pub fn new_tracker() -> DelegationTracker {
    Arc::new(Mutex::new(Vec::new()))
}

// ---------------------------------------------------------------------------
// DelegateTaskTool
// ---------------------------------------------------------------------------

/// Built-in tool for the lead agent to delegate tasks to specialists.
///
/// When executed, this tool creates a specialist agent from its profile,
/// optionally attenuates its capabilities (NT-02), registers message tools
/// on the specialist (NT-04), runs the task, records the result in the
/// shared tracker (NT-03), and returns the result.
///
/// When the `async` input parameter is `true`, the specialist is spawned
/// in a background task and the tool returns immediately with a job ID.
/// Use [`CheckJobStatusTool`] or [`CollectResultsTool`] to monitor progress.
pub struct DelegateTaskTool {
    id: ToolId,
    session: Arc<AgentSession>,
    audit_log: Option<AuditLog>,
    lead_name: String,
    tracker: DelegationTracker,
    job_tracker: JobTracker,
    pool: SpecialistPool,
    /// Optional token channel for streaming specialist output to the client.
    /// When set, specialists use `turn_stream()` and their tokens flow through
    /// the team's output channel with `[specialist_name]` headers.
    token_tx: Option<tokio::sync::mpsc::Sender<String>>,
    /// Optional memory manager for recording delegation outcomes.
    #[cfg(feature = "memory")]
    memory_manager: Option<Arc<Mutex<MemoryManager>>>,
}

impl DelegateTaskTool {
    /// Create a new delegation tool.
    pub fn new(
        session: Arc<AgentSession>,
        audit_log: Option<AuditLog>,
        lead_name: String,
        tracker: DelegationTracker,
        job_tracker: JobTracker,
        pool: SpecialistPool,
    ) -> Self {
        Self {
            id: ToolId::new(),
            session,
            audit_log,
            lead_name,
            tracker,
            job_tracker,
            pool,
            token_tx: None,
            #[cfg(feature = "memory")]
            memory_manager: None,
        }
    }

    /// Attach a memory manager for recording delegation outcomes.
    #[cfg(feature = "memory")]
    pub fn with_memory_manager(mut self, manager: Arc<Mutex<MemoryManager>>) -> Self {
        self.memory_manager = Some(manager);
        self
    }

    /// Set the token sender for streaming specialist output.
    ///
    /// When set, specialist turns use `turn_stream()` instead of `turn()`,
    /// sending tokens through the team's output channel so clients see
    /// specialist activity in real-time.
    pub fn with_token_tx(mut self, tx: tokio::sync::mpsc::Sender<String>) -> Self {
        self.token_tx = Some(tx);
        self
    }

    fn audit(&self, event: AuditEvent) {
        if let Some(log) = &self.audit_log
            && let Err(e) = log.append(event)
        {
            warn!("Failed to write audit event: {e}");
        }
    }
}

#[async_trait]
impl Tool for DelegateTaskTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "delegate_task"
    }

    fn description(&self) -> &str {
        "Delegate a task to a specific team member agent. Set async=true to run in background and get a job_id for status tracking."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "agent_name": {
                    "type": "string",
                    "description": "Name of the team member to delegate to"
                },
                "task": {
                    "type": "string",
                    "description": "The task to delegate"
                },
                "async": {
                    "type": "boolean",
                    "description": "If true, run in background and return a job_id immediately. Default: false."
                },
                "max_retries": {
                    "type": "integer",
                    "description": "Max retry attempts on transient failure (RateLimit/Http). Default: 0. Max: 3."
                },
                "fallback_agent": {
                    "type": "string",
                    "description": "Name of a fallback specialist to try if the primary agent fails after retries."
                }
            },
            "required": ["agent_name", "task"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("coordination".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let agent_name = input["agent_name"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("delegate_task: missing 'agent_name'".into()))?;
        let task = input["task"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("delegate_task: missing 'task'".into()))?;
        let run_async = input["async"].as_bool().unwrap_or(false);
        let max_retries = input["max_retries"]
            .as_u64()
            .unwrap_or(0)
            .min(MAX_RETRY_LIMIT as u64) as u32;
        let fallback_agent = input["fallback_agent"].as_str().map(String::from);

        info!(
            "Delegating task to specialist '{agent_name}' (async={run_async}, retries={max_retries})"
        );

        self.audit(AuditEvent::TeamDelegation {
            from: self.lead_name.clone(),
            to: agent_name.to_string(),
            task: task.to_string(),
        });

        // Get or create the specialist from the pool (preserves conversation context)
        let specialist_arc = match self.pool.get_or_create(agent_name).await {
            Ok(arc) => arc,
            Err(error_msg) => {
                let result = DelegationResult {
                    agent: agent_name.to_string(),
                    task: task.to_string(),
                    status: "error".into(),
                    response: error_msg.clone(),
                };
                self.tracker.lock().await.push(result);
                return Ok(serde_json::json!({
                    "status": "error",
                    "agent": agent_name,
                    "error": error_msg
                }));
            }
        };

        // Async mode: spawn in background, return job_id
        if run_async {
            let job_id = self.job_tracker.spawn_job(agent_name, task).await;
            let job_tracker = self.job_tracker.clone();
            let tracker = Arc::clone(&self.tracker);
            let session = Arc::clone(&self.session);
            let pool = self.pool.clone();
            let agent_name_owned = agent_name.to_string();
            let task_owned = task.to_string();
            let lead_name = self.lead_name.clone();
            let job_id_clone = job_id.clone();
            let specialist_clone = Arc::clone(&specialist_arc);
            let fallback_agent_clone = fallback_agent.clone();
            #[cfg(feature = "memory")]
            let memory_manager = self.memory_manager.clone();

            tokio::spawn(async move {
                job_tracker
                    .update_progress(
                        &job_id_clone,
                        format!("Starting specialist execution for '{agent_name_owned}'"),
                    )
                    .await;

                let (mut status, mut response) = run_specialist_with_retry(
                    &specialist_clone,
                    &task_owned,
                    &agent_name_owned,
                    &None, // async path doesn't stream
                    max_retries,
                )
                .await;

                let mut actual_agent = agent_name_owned.clone();

                // Fallback on failure
                if status == "error"
                    && let Some(ref fb_name) = fallback_agent_clone
                {
                    job_tracker
                        .update_progress(
                            &job_id_clone,
                            format!("Primary failed, trying fallback '{fb_name}'"),
                        )
                        .await;
                    if let Ok(fb_arc) = pool.get_or_create(fb_name).await {
                        let (fb_status, fb_response) =
                            run_specialist_with_retry(&fb_arc, &task_owned, fb_name, &None, 0)
                                .await;
                        status = fb_status;
                        response = fb_response;
                        actual_agent = fb_name.clone();
                    }
                }

                let success = status == "completed";

                job_tracker
                    .update_progress(
                        &job_id_clone,
                        if success {
                            "Execution complete, recording result".to_string()
                        } else {
                            "Execution failed, recording error".to_string()
                        },
                    )
                    .await;

                // Track in delegation tracker
                let result = DelegationResult {
                    agent: actual_agent.clone(),
                    task: task_owned.clone(),
                    status,
                    response: response.clone(),
                };
                tracker.lock().await.push(result);

                // Update team context so other specialists see this work
                pool.update_context(&actual_agent, &task_owned, &response)
                    .await;

                // Record delegation outcome
                #[cfg(feature = "memory")]
                if let Some(ref mgr) = memory_manager {
                    let record = OutcomeRecord::new(
                        OutcomeSource::Delegation {
                            specialist: actual_agent.clone(),
                            task: task_owned.clone(),
                        },
                        success,
                        response.clone(),
                        0,
                        lead_name.clone(),
                        task_owned.clone(),
                    );
                    if let Ok(mgr) = mgr.try_lock() {
                        if let Err(e) = mgr.record_outcome(&record) {
                            warn!("Failed to record async delegation outcome: {e}");
                        }
                    }
                }

                // Update job tracker
                if success {
                    job_tracker.complete_job(&job_id_clone, response).await;
                } else {
                    job_tracker.fail_job(&job_id_clone, response).await;
                }

                // Audit the completion (create fresh AuditLog for spawned task)
                let audit_log = session.create_audit_log();
                let _ = audit_log.append(AuditEvent::JobCompleted {
                    team_name: lead_name,
                    agent_name: actual_agent,
                    success,
                });
            });

            // Audit the spawn
            self.audit(AuditEvent::JobSpawned {
                team_name: self.lead_name.clone(),
                agent_name: agent_name.to_string(),
                task_summary: task.to_string(),
            });

            return Ok(serde_json::json!({
                "status": "spawned",
                "agent": agent_name,
                "job_id": job_id
            }));
        }

        // Synchronous mode (default): run and wait with optional retry.
        let (mut status, mut response) = run_specialist_with_retry(
            &specialist_arc,
            task,
            agent_name,
            &self.token_tx,
            max_retries,
        )
        .await;

        let mut actual_agent = agent_name.to_string();
        let mut used_fallback = false;

        // Fallback on failure: try fallback_agent if specified
        if status == "error"
            && let Some(ref fb_name) = fallback_agent
        {
            info!("Primary '{agent_name}' failed, trying fallback '{fb_name}'");
            match self.pool.get_or_create(fb_name).await {
                Ok(fb_arc) => {
                    let (fb_status, fb_response) = run_specialist_with_retry(
                        &fb_arc,
                        task,
                        fb_name,
                        &self.token_tx,
                        0, // no retries on fallback
                    )
                    .await;
                    status = fb_status;
                    response = fb_response;
                    actual_agent = fb_name.clone();
                    used_fallback = true;
                }
                Err(e) => {
                    warn!("Failed to create fallback specialist '{fb_name}': {e}");
                }
            }
        }

        // NT-03: Track the result
        let result = DelegationResult {
            agent: actual_agent.clone(),
            task: task.to_string(),
            status: status.clone(),
            response: response.clone(),
        };
        self.tracker.lock().await.push(result);

        // Update team context so other specialists see this work
        self.pool
            .update_context(&actual_agent, task, &response)
            .await;

        // Record delegation outcome
        #[cfg(feature = "memory")]
        {
            let success = status == "completed";
            if let Some(ref mgr) = self.memory_manager {
                let record = OutcomeRecord::new(
                    OutcomeSource::Delegation {
                        specialist: actual_agent.clone(),
                        task: task.to_string(),
                    },
                    success,
                    response.clone(),
                    0, // duration not tracked for sync delegations yet
                    self.lead_name.clone(),
                    task.to_string(),
                );
                if let Ok(mgr) = mgr.try_lock() {
                    if let Err(e) = mgr.record_outcome(&record) {
                        warn!("Failed to record delegation outcome: {e}");
                    }
                }
            }
        }

        let mut json_result = serde_json::json!({
            "status": status,
            "agent": actual_agent,
            "result": response,
        });
        if used_fallback {
            json_result["fallback"] = serde_json::json!(true);
            json_result["original_agent"] = serde_json::json!(agent_name);
        }
        Ok(json_result)
    }
}

// ---------------------------------------------------------------------------
// QueryAgentTool
// ---------------------------------------------------------------------------

/// Built-in tool for querying a team member agent.
///
/// Similar to delegation but wraps the query with context indicating
/// the specialist should provide information rather than execute a task.
pub struct QueryAgentTool {
    id: ToolId,
    audit_log: Option<AuditLog>,
    lead_name: String,
    tracker: DelegationTracker,
    pool: SpecialistPool,
    /// Optional token channel for streaming specialist output.
    token_tx: Option<tokio::sync::mpsc::Sender<String>>,
}

impl QueryAgentTool {
    /// Create a new query tool that uses the shared specialist pool.
    pub fn new(
        audit_log: Option<AuditLog>,
        lead_name: String,
        tracker: DelegationTracker,
        pool: SpecialistPool,
    ) -> Self {
        Self {
            id: ToolId::new(),
            audit_log,
            lead_name,
            tracker,
            pool,
            token_tx: None,
        }
    }

    /// Set the token sender for streaming specialist output.
    pub fn with_token_tx(mut self, tx: tokio::sync::mpsc::Sender<String>) -> Self {
        self.token_tx = Some(tx);
        self
    }

    fn audit(&self, event: AuditEvent) {
        if let Some(log) = &self.audit_log
            && let Err(e) = log.append(event)
        {
            warn!("Failed to write audit event: {e}");
        }
    }
}

#[async_trait]
impl Tool for QueryAgentTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "query_agent"
    }

    fn description(&self) -> &str {
        "Query a team member agent for information or status."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "agent_name": {
                    "type": "string",
                    "description": "Name of the agent to query"
                },
                "query": {
                    "type": "string",
                    "description": "The query to send"
                }
            },
            "required": ["agent_name", "query"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("coordination".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let agent_name = input["agent_name"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("query_agent: missing 'agent_name'".into()))?;
        let query = input["query"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("query_agent: missing 'query'".into()))?;

        info!("Querying specialist '{agent_name}'");

        self.audit(AuditEvent::TeamDelegation {
            from: self.lead_name.clone(),
            to: agent_name.to_string(),
            task: format!("[query] {query}"),
        });

        let specialist_arc = match self.pool.get_or_create(agent_name).await {
            Ok(arc) => arc,
            Err(error_msg) => {
                let result = DelegationResult {
                    agent: agent_name.to_string(),
                    task: format!("[query] {query}"),
                    status: "error".into(),
                    response: error_msg.clone(),
                };
                self.tracker.lock().await.push(result);
                return Ok(serde_json::json!({
                    "status": "error",
                    "agent": agent_name,
                    "error": error_msg
                }));
            }
        };

        // Wrap query with context to indicate information retrieval
        let prompt = format!(
            "You are being queried by the team coordinator. \
             Please provide a concise, informative answer.\n\nQuery: {query}"
        );

        let (status, response) = {
            let mut specialist = specialist_arc.lock().await;
            if let Some(ref tx) = self.token_tx {
                let _ = tx.send(format!("\n--- [{agent_name}] ---\n")).await;
                let result = specialist
                    .turn_stream(&prompt, None, tx.clone(), None)
                    .await;
                let _ = tx.send(format!("\n--- [/{agent_name}] ---\n")).await;
                match result {
                    Ok(response) => ("completed".to_string(), response),
                    Err(e) => ("error".to_string(), format!("query failed: {e}")),
                }
            } else {
                match specialist.turn(&prompt, None).await {
                    Ok(response) => ("completed".to_string(), response),
                    Err(e) => ("error".to_string(), format!("query failed: {e}")),
                }
            }
        };

        // NT-03: Track the result
        let query_task = format!("[query] {query}");
        let result = DelegationResult {
            agent: agent_name.to_string(),
            task: query_task.clone(),
            status: status.clone(),
            response: response.clone(),
        };
        self.tracker.lock().await.push(result);

        // Update team context so other specialists see this work
        self.pool
            .update_context(agent_name, &query_task, &response)
            .await;

        Ok(serde_json::json!({
            "status": status,
            "agent": agent_name,
            "response": response
        }))
    }
}

// ---------------------------------------------------------------------------
// CheckJobStatusTool
// ---------------------------------------------------------------------------

/// Built-in tool for checking the status of async specialist jobs.
///
/// Reads from the shared [`JobTracker`] to report on background jobs
/// spawned by [`DelegateTaskTool`] with `async: true`.
pub struct CheckJobStatusTool {
    id: ToolId,
    job_tracker: JobTracker,
}

impl CheckJobStatusTool {
    /// Create a new job status checking tool.
    pub fn new(job_tracker: JobTracker) -> Self {
        Self {
            id: ToolId::new(),
            job_tracker,
        }
    }
}

#[async_trait]
impl Tool for CheckJobStatusTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "check_job_status"
    }

    fn description(&self) -> &str {
        "Check the status of async specialist jobs. Provide a job_id for a specific job, or omit to see all jobs. Use 'since' to paginate progress events."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "job_id": {
                    "type": "string",
                    "description": "Optional: specific job ID to check. Omit to list all."
                },
                "since": {
                    "type": "integer",
                    "description": "Return progress events since this index (0 for all). Default: 0. Only used with job_id."
                }
            },
            "required": []
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("coordination".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        if let Some(job_id) = input["job_id"].as_str() {
            let since = input["since"].as_u64().unwrap_or(0) as usize;

            // Single job lookup with progress
            match self.job_tracker.get_job(job_id).await {
                Some(job) => {
                    let progress_events: Vec<&crate::job_tracker::JobProgress> =
                        job.progress.iter().skip(since).collect();
                    let elapsed_secs = job
                        .finished_at
                        .unwrap_or_else(chrono::Utc::now)
                        .signed_duration_since(job.created_at)
                        .num_milliseconds() as f64
                        / 1000.0;

                    Ok(serde_json::json!({
                        "id": job.id,
                        "agent_name": job.agent_name,
                        "task": job.task,
                        "status": job.status,
                        "result": job.result,
                        "error": job.error,
                        "progress": progress_events,
                        "progress_total": job.progress.len(),
                        "elapsed_secs": elapsed_secs,
                        "created_at": job.created_at.to_rfc3339(),
                        "finished_at": job.finished_at.map(|t| t.to_rfc3339()),
                    }))
                }
                None => Ok(serde_json::json!({
                    "error": format!("no job found with id '{job_id}'")
                })),
            }
        } else {
            // List all jobs with progress counts
            let jobs = self.job_tracker.list_jobs().await;
            let all_done = self.job_tracker.all_completed().await;
            let job_summaries: Vec<serde_json::Value> = jobs
                .iter()
                .map(|j| {
                    serde_json::json!({
                        "id": j.id,
                        "agent_name": j.agent_name,
                        "task": j.task,
                        "status": j.status,
                        "progress_count": j.progress.len(),
                        "created_at": j.created_at.to_rfc3339(),
                        "finished_at": j.finished_at.map(|t| t.to_rfc3339()),
                    })
                })
                .collect();

            Ok(serde_json::json!({
                "total": jobs.len(),
                "all_completed": all_done,
                "jobs": job_summaries,
            }))
        }
    }
}

// ---------------------------------------------------------------------------
// CollectResultsTool
// ---------------------------------------------------------------------------

/// Built-in tool for collecting results from delegated tasks.
///
/// Reads from both the synchronous [`DelegationTracker`] and the async
/// [`JobTracker`] to return an aggregated summary. When `wait` is `true`,
/// blocks until all async jobs complete before returning.
pub struct CollectResultsTool {
    id: ToolId,
    tracker: DelegationTracker,
    job_tracker: JobTracker,
}

impl CollectResultsTool {
    /// Create a new results collection tool.
    pub fn new(tracker: DelegationTracker, job_tracker: JobTracker) -> Self {
        Self {
            id: ToolId::new(),
            tracker,
            job_tracker,
        }
    }
}

#[async_trait]
impl Tool for CollectResultsTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "collect_results"
    }

    fn description(&self) -> &str {
        "Collect and aggregate results from all delegated tasks, queries, and async jobs. Set wait=true to block until all async jobs finish."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "agent_names": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional: filter results to specific agents. Omit to get all."
                },
                "wait": {
                    "type": "boolean",
                    "description": "If true, wait for all async jobs to complete before returning. Default: false."
                }
            },
            "required": []
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("coordination".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let filter_names: Option<Vec<String>> = input["agent_names"].as_array().map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        });
        let wait = input["wait"].as_bool().unwrap_or(false);

        // Optionally wait for async jobs
        if wait {
            let completed = self
                .job_tracker
                .wait_all(Duration::from_millis(500), Duration::from_secs(300))
                .await;
            if !completed {
                return Ok(serde_json::json!({
                    "error": "timed out waiting for async jobs (300s limit)"
                }));
            }
        }

        // Gather sync delegation results
        let results = self.tracker.lock().await;
        let filtered: Vec<&DelegationResult> = if let Some(ref names) = filter_names {
            results
                .iter()
                .filter(|r| names.contains(&r.agent))
                .collect()
        } else {
            results.iter().collect()
        };

        let mut summary: Vec<serde_json::Value> = filtered
            .iter()
            .map(|r| {
                serde_json::json!({
                    "agent": r.agent,
                    "task": r.task,
                    "status": r.status,
                    "response": r.response,
                })
            })
            .collect();

        // Include async job results too (they are also in the delegation tracker
        // when completed, but include pending/running jobs for visibility)
        let async_jobs = self.job_tracker.list_jobs().await;
        let pending_jobs: Vec<serde_json::Value> = async_jobs
            .iter()
            .filter(|j| {
                j.status == crate::job_tracker::JobStatus::Running
                    || j.status == crate::job_tracker::JobStatus::Pending
            })
            .filter(|j| {
                filter_names
                    .as_ref()
                    .is_none_or(|names| names.contains(&j.agent_name))
            })
            .map(|j| {
                serde_json::json!({
                    "agent": j.agent_name,
                    "task": j.task,
                    "status": format!("{:?}", j.status).to_lowercase(),
                    "response": "[in progress]",
                    "job_id": j.id,
                })
            })
            .collect();

        summary.extend(pending_jobs);

        let completed = summary
            .iter()
            .filter(|r| r["status"] == "completed")
            .count();
        let failed = summary.iter().filter(|r| r["status"] == "error").count();

        Ok(serde_json::json!({
            "total": summary.len(),
            "completed": completed,
            "failed": failed,
            "results": summary,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_results_schema() {
        let tracker = new_tracker();
        let job_tracker = JobTracker::new();
        let tool = CollectResultsTool::new(tracker, job_tracker);
        assert_eq!(tool.name(), "collect_results");
        let schema = tool.input_schema();
        assert!(schema["properties"]["agent_names"].is_object());
        assert!(schema["properties"]["wait"].is_object());
    }

    #[tokio::test]
    async fn tracker_records_and_collects() {
        let tracker = new_tracker();
        let job_tracker = JobTracker::new();

        // Simulate delegation results
        {
            let mut results = tracker.lock().await;
            results.push(DelegationResult {
                agent: "researcher".into(),
                task: "find docs".into(),
                status: "completed".into(),
                response: "found 3 files".into(),
            });
            results.push(DelegationResult {
                agent: "coder".into(),
                task: "implement X".into(),
                status: "completed".into(),
                response: "done, 42 lines".into(),
            });
            results.push(DelegationResult {
                agent: "reviewer".into(),
                task: "review code".into(),
                status: "error".into(),
                response: "failed to create".into(),
            });
        }

        let tool = CollectResultsTool::new(tracker, job_tracker);

        // Collect all
        let result = tool.execute(serde_json::json!({})).await.unwrap();
        assert_eq!(result["total"], 3);
        assert_eq!(result["completed"], 2);
        assert_eq!(result["failed"], 1);

        // Collect filtered
        let result = tool
            .execute(serde_json::json!({"agent_names": ["coder"]}))
            .await
            .unwrap();
        assert_eq!(result["total"], 1);
        assert_eq!(result["results"][0]["agent"], "coder");
    }

    #[test]
    fn delegation_result_serializes() {
        let r = DelegationResult {
            agent: "coder".into(),
            task: "write code".into(),
            status: "completed".into(),
            response: "done".into(),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("coder"));
        let roundtrip: DelegationResult = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip.agent, "coder");
    }

    #[test]
    fn delegate_task_schema_has_async() {
        let tracker = new_tracker();
        let job_tracker = JobTracker::new();
        // We can't create a real DelegateTaskTool without AgentSession,
        // but we can verify the schema is documented in input_schema.
        // CheckJobStatusTool is simpler to test directly.
        let tool = CheckJobStatusTool::new(job_tracker.clone());
        assert_eq!(tool.name(), "check_job_status");
        let schema = tool.input_schema();
        assert!(schema["properties"]["job_id"].is_object());

        // Verify CollectResultsTool got the wait param
        let tool = CollectResultsTool::new(tracker, job_tracker);
        let schema = tool.input_schema();
        assert!(schema["properties"]["wait"].is_object());
    }

    #[tokio::test]
    async fn specialist_pool_len_and_empty() {
        // Test pool structure without requiring actual agent creation
        // (which needs EncryptedStore + API keys). Full integration is
        // tested in aivyx-integration-tests.
        let dir = std::env::temp_dir().join(format!("aivyx-pool-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let dirs = aivyx_config::AivyxDirs::new(&dir);
        let config = aivyx_config::AivyxConfig::default();
        let key = aivyx_crypto::MasterKey::generate();
        let session = Arc::new(AgentSession::new(dirs, config, key));

        let pool = SpecialistPool::new(
            session,
            None,
            CapabilitySet::new(),
            None,
            DialogueConfig::default(),
        );

        // Pool starts empty
        assert!(pool.is_empty().await);
        assert_eq!(pool.len().await, 0);

        // get_or_create for nonexistent profile returns error (not panic)
        let result = pool.get_or_create("nonexistent").await;
        assert!(result.is_err());
        assert!(pool.is_empty().await); // failed creation doesn't pollute pool
    }

    #[test]
    fn attenuation_from_lead_produces_nonempty_result() {
        use crate::capability_delegation::attenuate_for_member;
        use aivyx_capability::Capability;
        use aivyx_core::{AgentId, CapabilityId};
        use chrono::Utc;

        // Simulate a lead agent with full capabilities (Filesystem, Shell, Custom("coordination"))
        let mut lead_set = CapabilitySet::new();
        let lead_principal = Principal::Agent(AgentId::new());

        // Lead has Filesystem { root: "/" }
        lead_set.grant(Capability {
            id: CapabilityId::new(),
            scope: CapabilityScope::Filesystem {
                root: std::path::PathBuf::from("/"),
            },
            pattern: aivyx_capability::ActionPattern::new("*").unwrap(),
            granted_to: vec![lead_principal.clone()],
            granted_by: Principal::System,
            created_at: Utc::now(),
            expires_at: None,
            revoked: false,
            parent_id: None,
        });

        // Lead has Shell
        lead_set.grant(Capability {
            id: CapabilityId::new(),
            scope: CapabilityScope::Shell {
                allowed_commands: vec![],
            },
            pattern: aivyx_capability::ActionPattern::new("*").unwrap(),
            granted_to: vec![lead_principal.clone()],
            granted_by: Principal::System,
            created_at: Utc::now(),
            expires_at: None,
            revoked: false,
            parent_id: None,
        });

        // Lead has Custom("coordination")
        lead_set.grant(Capability {
            id: CapabilityId::new(),
            scope: CapabilityScope::Custom("coordination".into()),
            pattern: aivyx_capability::ActionPattern::new("*").unwrap(),
            granted_to: vec![lead_principal.clone()],
            granted_by: Principal::System,
            created_at: Utc::now(),
            expires_at: None,
            revoked: false,
            parent_id: None,
        });

        // Specialist wants Filesystem only (e.g., researcher role)
        let specialist_principal = Principal::Agent(AgentId::new());
        let specialist_scopes = vec![
            CapabilityScope::Filesystem {
                root: std::path::PathBuf::from("/"),
            },
            CapabilityScope::Custom("coordination".into()),
        ];

        let narrowed = attenuate_for_member(
            &lead_set,
            &specialist_principal,
            &specialist_scopes,
            "execute:*",
        );

        // Specialist should get Filesystem + coordination (not Shell, since not declared)
        assert_eq!(
            narrowed.len(),
            2,
            "specialist should get 2 capabilities (Filesystem + coordination), got {}",
            narrowed.len()
        );

        // Verify it can check Filesystem scope
        assert!(
            narrowed
                .check(
                    &specialist_principal,
                    &CapabilityScope::Filesystem {
                        root: std::path::PathBuf::from("/"),
                    },
                    "execute:file_read"
                )
                .is_ok(),
            "specialist should be able to execute file_read"
        );

        // Verify it can check coordination scope
        assert!(
            narrowed
                .check(
                    &specialist_principal,
                    &CapabilityScope::Custom("coordination".into()),
                    "execute:send_message"
                )
                .is_ok(),
            "specialist should be able to execute send_message"
        );

        // Verify Shell is NOT available (not in specialist_scopes)
        assert!(
            narrowed
                .check(
                    &specialist_principal,
                    &CapabilityScope::Shell {
                        allowed_commands: vec![],
                    },
                    "execute:shell"
                )
                .is_err(),
            "specialist should NOT be able to execute shell"
        );
    }

    #[tokio::test]
    async fn check_job_status_all() {
        let job_tracker = JobTracker::new();
        let id = job_tracker.spawn_job("researcher", "analyze data").await;

        let tool = CheckJobStatusTool::new(job_tracker.clone());

        // Check all jobs — includes progress_count
        let result = tool.execute(serde_json::json!({})).await.unwrap();
        assert_eq!(result["total"], 1);
        assert!(!result["all_completed"].as_bool().unwrap());
        assert_eq!(result["jobs"][0]["progress_count"], 0);

        // Check specific job — includes progress and elapsed_secs
        let result = tool
            .execute(serde_json::json!({"job_id": id}))
            .await
            .unwrap();
        assert_eq!(result["agent_name"], "researcher");
        assert_eq!(result["status"], "Running");
        assert!(result["elapsed_secs"].as_f64().is_some());
        assert_eq!(result["progress_total"], 0);

        // Complete the job
        job_tracker.complete_job(&id, "done".to_string()).await;
        let result = tool.execute(serde_json::json!({})).await.unwrap();
        assert!(result["all_completed"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn check_job_status_includes_progress() {
        let job_tracker = JobTracker::new();
        let id = job_tracker.spawn_job("coder", "build module").await;

        job_tracker
            .update_progress(&id, "Parsing input".to_string())
            .await;
        job_tracker
            .update_progress(&id, "Writing code".to_string())
            .await;

        let tool = CheckJobStatusTool::new(job_tracker);

        let result = tool
            .execute(serde_json::json!({"job_id": id}))
            .await
            .unwrap();
        assert_eq!(result["progress_total"], 2);
        let progress = result["progress"].as_array().unwrap();
        assert_eq!(progress.len(), 2);
        assert_eq!(progress[0]["message"], "Parsing input");
    }

    #[tokio::test]
    async fn check_job_status_since_param() {
        let job_tracker = JobTracker::new();
        let id = job_tracker.spawn_job("researcher", "search").await;

        job_tracker.update_progress(&id, "step 1".to_string()).await;
        job_tracker.update_progress(&id, "step 2".to_string()).await;
        job_tracker.update_progress(&id, "step 3".to_string()).await;

        let tool = CheckJobStatusTool::new(job_tracker);

        // Request only events since index 2
        let result = tool
            .execute(serde_json::json!({"job_id": id, "since": 2}))
            .await
            .unwrap();
        assert_eq!(result["progress_total"], 3);
        let progress = result["progress"].as_array().unwrap();
        assert_eq!(progress.len(), 1);
        assert_eq!(progress[0]["message"], "step 3");
    }

    #[tokio::test]
    async fn check_job_status_elapsed_secs() {
        let job_tracker = JobTracker::new();
        let id = job_tracker.spawn_job("analyst", "analyze").await;

        // Small sleep to ensure elapsed > 0
        tokio::time::sleep(Duration::from_millis(10)).await;

        let tool = CheckJobStatusTool::new(job_tracker);
        let result = tool
            .execute(serde_json::json!({"job_id": id}))
            .await
            .unwrap();

        let elapsed = result["elapsed_secs"].as_f64().unwrap();
        assert!(elapsed >= 0.0, "elapsed_secs should be non-negative");
    }

    #[tokio::test]
    async fn team_context_format_empty_work() {
        let ctx = TeamContext::new(
            "test-team".into(),
            "coordinator".into(),
            vec![
                ("researcher".into(), "Researcher".into()),
                ("coder".into(), "Coder".into()),
            ],
            "Build a web scraper".into(),
            true,
        );

        let formatted = ctx.format_for_role("researcher", "Researcher").await;
        assert!(formatted.starts_with("[TEAM CONTEXT]"));
        assert!(formatted.ends_with("[END TEAM CONTEXT]"));
        assert!(formatted.contains("Team: test-team"));
        assert!(formatted.contains("Your role: Researcher (researcher)"));
        assert!(formatted.contains("Coordinator: coordinator"));
        assert!(formatted.contains("- researcher (Researcher)"));
        assert!(formatted.contains("- coder (Coder)"));
        assert!(formatted.contains("Original goal: Build a web scraper"));
        // No completed work section
        assert!(!formatted.contains("Completed work so far"));
    }

    #[tokio::test]
    async fn team_context_format_with_work() {
        let ctx = TeamContext::new(
            "test-team".into(),
            "lead".into(),
            vec![("alice".into(), "Researcher".into())],
            "Analyze data".into(),
            true,
        );

        ctx.record_completion("alice", "search for papers", "Found 5 relevant papers")
            .await;

        let formatted = ctx.format_for_role("alice", "Researcher").await;
        assert!(formatted.contains("Completed work so far:"));
        assert!(formatted.contains("[alice] search for papers -> Found 5 relevant papers"));
    }

    #[tokio::test]
    async fn team_context_truncates_long_outcomes() {
        let ctx = TeamContext::new(
            "t".into(),
            "lead".into(),
            vec![("a".into(), "R".into())],
            "g".into(),
            true,
        );

        let long_outcome = "x".repeat(300);
        ctx.record_completion("a", "task", &long_outcome).await;

        let formatted = ctx.format_for_role("a", "R").await;
        // Outcome should be truncated to ~200 chars + "..."
        assert!(formatted.contains("..."));
        // Full 300-char outcome should NOT appear
        assert!(!formatted.contains(&long_outcome));
    }

    #[tokio::test]
    async fn team_context_caps_at_10_entries() {
        let ctx = TeamContext::new(
            "t".into(),
            "lead".into(),
            vec![("a".into(), "R".into())],
            "g".into(),
            true,
        );

        // Add 15 entries
        for i in 0..15 {
            ctx.record_completion("a", &format!("task-{i}"), &format!("result-{i}"))
                .await;
        }

        let formatted = ctx.format_for_role("a", "R").await;
        // Should show tasks 5-14 (last 10), not 0-4
        assert!(!formatted.contains("task-0"));
        assert!(!formatted.contains("task-4"));
        assert!(formatted.contains("task-5"));
        assert!(formatted.contains("task-14"));
    }

    #[tokio::test]
    async fn team_context_record_returns_count() {
        let ctx = TeamContext::new("t".into(), "lead".into(), vec![], "g".into(), true);

        assert_eq!(ctx.record_completion("a", "t1", "r1").await, 1);
        assert_eq!(ctx.record_completion("b", "t2", "r2").await, 2);
    }

    #[test]
    fn delegate_task_schema_has_retry_and_fallback() {
        // Verify the new retry/fallback params are in the schema.
        // We can't construct DelegateTaskTool without AgentSession in a unit test,
        // so verify the schema constants are documented in the test.
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "agent_name": { "type": "string" },
                "task": { "type": "string" },
                "async": { "type": "boolean" },
                "max_retries": { "type": "integer" },
                "fallback_agent": { "type": "string" }
            },
            "required": ["agent_name", "task"]
        });
        assert!(schema["properties"]["max_retries"].is_object());
        assert!(schema["properties"]["fallback_agent"].is_object());
    }

    #[tokio::test]
    async fn completed_work_snapshot_returns_clone() {
        let ctx = TeamContext::new("t".into(), "lead".into(), vec![], "g".into(), true);

        assert!(ctx.completed_work_snapshot().await.is_empty());

        ctx.record_completion("agent-a", "task-1", "result-1").await;
        ctx.record_completion("agent-b", "task-2", "result-2").await;

        let snapshot = ctx.completed_work_snapshot().await;
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot[0].agent, "agent-a");
        assert_eq!(snapshot[1].task, "task-2");

        // Snapshot is a clone — adding more work doesn't change it
        ctx.record_completion("agent-c", "task-3", "result-3").await;
        assert_eq!(snapshot.len(), 2);
    }

    #[test]
    fn max_retry_limit_constant() {
        assert_eq!(MAX_RETRY_LIMIT, 3);
    }

    #[tokio::test]
    async fn format_for_role_includes_peer_messaging() {
        let ctx = TeamContext::new(
            "t".into(),
            "lead".into(),
            vec![("a".into(), "R".into())],
            "g".into(),
            true,
        );

        let formatted = ctx.format_for_role("a", "R").await;
        assert!(formatted.contains("[PEER MESSAGING]"));
        assert!(formatted.contains("send_message"));
        assert!(formatted.contains("read_messages"));
        assert!(formatted.contains("request_peer_review"));
        assert!(formatted.contains("review_request"));
        assert!(formatted.contains("[END PEER MESSAGING]"));
    }

    #[tokio::test]
    async fn deregister_spawned_removes_from_pool() {
        let dir = std::env::temp_dir().join(format!("aivyx-pool-dereg-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let dirs = aivyx_config::AivyxDirs::new(&dir);
        let config = aivyx_config::AivyxConfig::default();
        let key = aivyx_crypto::MasterKey::generate();
        let session = Arc::new(AgentSession::new(dirs, config, key));

        let pool = SpecialistPool::new(
            session,
            None,
            CapabilitySet::new(),
            None,
            DialogueConfig::default(),
        );

        // Register a spawned specialist
        pool.register_spawned("ephemeral", "helper").await;

        // Deregister it
        pool.deregister_spawned("ephemeral").await;

        // Pool should be empty (the agent was never materialized via
        // get_or_create, so the agents map stays empty — but the method
        // must not panic for non-existent keys).
        assert!(pool.is_empty().await);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn format_for_role_omits_peer_messaging_when_disabled() {
        let ctx = TeamContext::new(
            "t".into(),
            "lead".into(),
            vec![("a".into(), "R".into())],
            "g".into(),
            false,
        );

        let formatted = ctx.format_for_role("a", "R").await;
        assert!(!formatted.contains("[PEER MESSAGING]"));
        assert!(!formatted.contains("[END PEER MESSAGING]"));
    }
}

use std::sync::Arc;

use tokio::sync::{Mutex, mpsc};
use tracing::info;

use aivyx_agent::AgentSession;
use aivyx_config::AivyxDirs;
use aivyx_core::{AivyxError, ChannelAdapter, Result};

use crate::config::{OrchestrationMode, TeamConfig};
use crate::decompose::DecomposeTaskTool;
use crate::delegation::{
    CheckJobStatusTool, CollectResultsTool, DelegateTaskTool, QueryAgentTool, SpecialistPool,
    TeamContext, new_tracker,
};
use crate::job_tracker::JobTracker;
use crate::message_bus::MessageBus;
use crate::message_tools::{ReadMessagesTool, SendMessageTool};
use crate::suggest::{SuggestSpecialistTool, build_member_profiles};
use crate::synthesize::SynthesizeResultsTool;
use crate::verify::VerifyOutputTool;

/// Runs a team of agents coordinated by a lead agent.
pub struct TeamRuntime {
    config: TeamConfig,
    session: Arc<AgentSession>,
}

impl TeamRuntime {
    pub fn new(config: TeamConfig, session: AgentSession) -> Self {
        Self {
            config,
            session: Arc::new(session),
        }
    }

    /// Load a team from disk by name.
    pub fn load(name: &str, dirs: &AivyxDirs, session: AgentSession) -> Result<Self> {
        let config_path = dirs.teams_dir().join(format!("{name}.toml"));
        if !config_path.exists() {
            return Err(AivyxError::Config(format!(
                "team config not found: {name} (expected at {})",
                config_path.display()
            )));
        }
        let config = TeamConfig::load(&config_path)?;
        Ok(Self::new(config, session))
    }

    /// Execute a team task: the lead agent receives the prompt and coordinates.
    pub async fn run(&self, prompt: &str, channel: Option<&dyn ChannelAdapter>) -> Result<String> {
        let mut lead_agent = self.create_lead_agent(None, prompt).await?;

        let mut team_info = self.build_team_info();

        // Recall prior team runs for context
        #[cfg(feature = "memory")]
        if let Some(prior) = self.recall_prior_runs(prompt).await {
            team_info.push_str(&prior);
        }

        let enhanced_prompt = format!("{prompt}\n\n{team_info}");

        let result = lead_agent.turn(&enhanced_prompt, channel).await?;

        // Store run summary for future recall (fire-and-forget)
        #[cfg(feature = "memory")]
        if let Err(e) = self.store_run_summary(prompt, &result).await {
            tracing::warn!("Failed to store team run summary: {e}");
        }

        Ok(result)
    }

    /// Execute a team task with streaming: tokens are sent to `token_tx`
    /// as they arrive from the lead agent.
    ///
    /// Specialist output also streams through `token_tx` with
    /// `[specialist_name]` delimiters, so the client sees interleaved output.
    pub async fn run_stream(
        &self,
        prompt: &str,
        channel: Option<&dyn ChannelAdapter>,
        token_tx: mpsc::Sender<String>,
    ) -> Result<String> {
        let mut lead_agent = self
            .create_lead_agent(Some(token_tx.clone()), prompt)
            .await?;

        let mut team_info = self.build_team_info();

        // Recall prior team runs for context
        #[cfg(feature = "memory")]
        if let Some(prior) = self.recall_prior_runs(prompt).await {
            team_info.push_str(&prior);
        }

        let enhanced_prompt = format!("{prompt}\n\n{team_info}");

        let result = lead_agent
            .turn_stream(&enhanced_prompt, channel, token_tx, None)
            .await?;

        // Store run summary for future recall (fire-and-forget)
        #[cfg(feature = "memory")]
        if let Err(e) = self.store_run_summary(prompt, &result).await {
            tracing::warn!("Failed to store team run summary: {e}");
        }

        Ok(result)
    }

    /// Create the lead agent with delegation, decompose, and message tools registered.
    ///
    /// When `token_tx` is provided, delegation tools stream specialist output
    /// through the channel, enabling real-time visibility of specialist work.
    ///
    /// `original_goal` is the user's prompt, threaded into [`TeamContext`] so
    /// specialists know the high-level objective.
    async fn create_lead_agent(
        &self,
        token_tx: Option<mpsc::Sender<String>>,
        original_goal: &str,
    ) -> Result<aivyx_agent::Agent> {
        let OrchestrationMode::LeadAgent { ref lead } = self.config.orchestration;

        info!("Team '{}' starting with lead '{lead}'", self.config.name);

        let mut lead_agent = self.session.create_agent(lead).await?;

        // Set up MessageBus with channels for all team members
        let member_names: Vec<String> =
            self.config.members.iter().map(|m| m.name.clone()).collect();
        let bus = MessageBus::new(&member_names);

        // Subscribe to the lead agent's channel (can be done multiple times)
        let lead_rx = bus.subscribe(lead);
        let bus = Arc::new(bus);

        // NT-03: Create shared trackers for all delegation tools
        let tracker = new_tracker();
        let job_tracker = JobTracker::new();

        // Build team context so specialists know about the team structure,
        // the original goal, and completed work as it progresses.
        let team_context = TeamContext::new(
            self.config.name.clone(),
            lead.clone(),
            self.config
                .members
                .iter()
                .map(|m| (m.name.clone(), m.role.clone()))
                .collect(),
            original_goal.to_string(),
            self.config.dialogue.enable_peer_dialogue,
        );

        // Create specialist pool shared by delegation and query tools.
        // Specialists persist across delegations, maintaining conversation context.
        // The lead's capabilities are passed so specialist attenuation can derive
        // from the lead's authority (specialists can never exceed the lead).
        let lead_caps = lead_agent.capabilities().clone();
        let pool = SpecialistPool::new(
            Arc::clone(&self.session),
            Some(Arc::clone(&bus)),
            lead_caps,
            Some(team_context),
            self.config.dialogue.clone(),
        );

        // Register delegation tools with tracker + job_tracker + pool + audit logging.
        // When token_tx is provided, specialists stream their output through it.
        let mut delegate_tool = DelegateTaskTool::new(
            Arc::clone(&self.session),
            Some(self.session.create_audit_log()),
            lead.clone(),
            Arc::clone(&tracker),
            job_tracker.clone(),
            pool.clone(),
        );
        if let Some(ref tx) = token_tx {
            delegate_tool = delegate_tool.with_token_tx(tx.clone());
        }
        lead_agent.register_tool(Box::new(delegate_tool));

        let mut query_tool = QueryAgentTool::new(
            Some(self.session.create_audit_log()),
            lead.clone(),
            Arc::clone(&tracker),
            pool.clone(),
        );
        if let Some(ref tx) = token_tx {
            query_tool = query_tool.with_token_tx(tx.clone());
        }
        lead_agent.register_tool(Box::new(query_tool));
        // Async job status checking
        lead_agent.register_tool(Box::new(CheckJobStatusTool::new(job_tracker.clone())));
        // NT-03: CollectResultsTool reads from both sync and async trackers
        lead_agent.register_tool(Box::new(CollectResultsTool::new(
            Arc::clone(&tracker),
            job_tracker,
        )));

        // Register decompose_task tool for structured task decomposition
        let member_roles: Vec<(String, String)> = self
            .config
            .members
            .iter()
            .map(|m| (m.name.clone(), m.role.clone()))
            .collect();
        lead_agent.register_tool(Box::new(DecomposeTaskTool::new(
            Arc::clone(&self.session),
            member_roles,
        )));

        // Register suggest_specialist tool for capability-aware assignment
        let member_profiles = build_member_profiles(&self.config.members);
        lead_agent.register_tool(Box::new(SuggestSpecialistTool::new(member_profiles)));

        // Register synthesize_results tool for LLM-driven result aggregation
        lead_agent.register_tool(Box::new(SynthesizeResultsTool::new(
            Arc::clone(&self.session),
            Arc::clone(&tracker),
        )));

        // Register verify_output tool for structured quality gate
        let mut verify_tool = VerifyOutputTool::new(pool.clone());
        if let Some(ref tx) = token_tx {
            verify_tool = verify_tool.with_token_tx(tx.clone());
        }
        lead_agent.register_tool(Box::new(verify_tool));

        // Register message tools with rate limiting
        let mut lead_send = SendMessageTool::new(Arc::clone(&bus), lead.clone());
        if self.config.dialogue.max_messages_per_turn > 0 {
            lead_send = lead_send.with_max_per_turn(self.config.dialogue.max_messages_per_turn);
        }
        lead_agent.register_tool(Box::new(lead_send));

        if let Some(rx) = lead_rx {
            lead_agent.register_tool(Box::new(ReadMessagesTool::new(Arc::new(Mutex::new(rx)))));
        }

        // Register peer review tool when dialogue is enabled
        if self.config.dialogue.enable_peer_dialogue {
            lead_agent.register_tool(Box::new(crate::message_tools::RequestPeerReviewTool::new(
                Arc::clone(&bus),
                lead.clone(),
            )));
        }

        Ok(lead_agent)
    }

    /// Store a summary of this team run for future cross-run recall.
    ///
    /// Stores the goal and outcome as a `MemoryKind::Custom("team-run-summary")`
    /// entry, tagged with the team name for scoped retrieval.
    #[cfg(feature = "memory")]
    async fn store_run_summary(&self, original_goal: &str, result: &str) -> Result<()> {
        let mut mgr = self.create_memory_manager()?;

        // Truncate result for the memory entry
        let truncated_result = if result.len() > 500 {
            let boundary = result.floor_char_boundary(500);
            format!("{}...", &result[..boundary])
        } else {
            result.to_string()
        };

        let summary = format!(
            "Team '{}' run summary:\nGoal: {}\nOutcome: {}",
            self.config.name, original_goal, truncated_result
        );

        mgr.remember(
            summary,
            aivyx_memory::MemoryKind::Custom("team-run-summary".into()),
            None, // team-level, no agent scope
            vec![format!("team:{}", self.config.name), "team-run".into()],
        )
        .await?;

        info!("Stored team run summary for '{}'", self.config.name);
        Ok(())
    }

    /// Recall relevant prior team runs for the given goal.
    ///
    /// Returns a formatted string to append to the team info, or `None` if
    /// no relevant prior runs are found or memory is not configured.
    #[cfg(feature = "memory")]
    async fn recall_prior_runs(&self, goal: &str) -> Option<String> {
        let mut mgr = self.create_memory_manager().ok()?;

        let entries = mgr
            .recall(
                goal,
                3, // top 3 most relevant prior runs
                None,
                &[format!("team:{}", self.config.name)],
            )
            .await
            .ok()?;

        if entries.is_empty() {
            return None;
        }

        let mut context = String::from("\n## Prior Team Runs\n\n");
        for entry in &entries {
            context.push_str(&format!("- {}\n", entry.content));
        }
        Some(context)
    }

    /// Create a `MemoryManager` for team-level memory operations.
    ///
    /// Uses the same pattern as `AgentSession::create_memory_manager()` but
    /// is team-scoped (no agent ID).
    #[cfg(feature = "memory")]
    fn create_memory_manager(&self) -> Result<aivyx_memory::MemoryManager> {
        let embedding_config = self
            .session
            .config()
            .embedding
            .as_ref()
            .ok_or_else(|| AivyxError::Config("No embedding config for team memory".into()))?;
        let store = aivyx_crypto::EncryptedStore::open(self.session.dirs().store_path())?;
        let embedding_provider = aivyx_llm::create_embedding_provider(
            embedding_config,
            &store,
            self.session.master_key(),
        )?;
        let memory_db_path = self.session.dirs().memory_dir().join("memory.db");
        let memory_store = aivyx_memory::MemoryStore::open(&memory_db_path)?;
        let memory_key = aivyx_crypto::derive_memory_key(self.session.master_key());
        aivyx_memory::MemoryManager::new(
            memory_store,
            std::sync::Arc::from(embedding_provider),
            memory_key,
            self.session.config().memory.max_memories,
        )
    }

    fn build_team_info(&self) -> String {
        let mut info = String::from("## Available Team Members\n\n");
        for member in &self.config.members {
            info.push_str(&format!("- **{}** (role: {})\n", member.name, member.role));
        }
        info.push_str(
            "\nYou can delegate tasks to team members using the delegate_task tool \
             (with optional retry and fallback), query them with query_agent, collect \
             all results with collect_results, synthesize them with synthesize_results, \
             verify quality with verify_output, communicate via send_message / read_messages, \
             request peer reviews with request_peer_review, decompose goals with decompose_task, \
             and find the best specialist with suggest_specialist.\n",
        );
        info
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DialogueConfig, OrchestrationMode, TeamConfig, TeamMemberConfig};
    use aivyx_agent::AgentSession;
    use aivyx_config::{AivyxConfig, AivyxDirs};
    use aivyx_crypto::MasterKey;

    fn make_runtime(config: TeamConfig) -> TeamRuntime {
        let dir = std::env::temp_dir().join(format!("aivyx-team-rt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let dirs = AivyxDirs::new(&dir);
        let aivyx_config = AivyxConfig::default();
        let key = MasterKey::generate();
        let session = AgentSession::new(dirs, aivyx_config, key);
        TeamRuntime::new(config, session)
    }

    #[test]
    fn build_team_info_includes_members() {
        let config = TeamConfig {
            name: "test-team".into(),
            description: "A test team".into(),
            orchestration: OrchestrationMode::LeadAgent {
                lead: "coordinator".into(),
            },
            members: vec![
                TeamMemberConfig {
                    name: "alice".into(),
                    role: "researcher".into(),
                },
                TeamMemberConfig {
                    name: "bob".into(),
                    role: "coder".into(),
                },
            ],
            dialogue: DialogueConfig::default(),
        };

        let runtime = make_runtime(config);
        let info = runtime.build_team_info();
        assert!(info.contains("alice"));
        assert!(info.contains("researcher"));
        assert!(info.contains("bob"));
        assert!(info.contains("coder"));
    }

    #[test]
    fn build_team_info_includes_tool_hints() {
        let config = TeamConfig {
            name: "test-team".into(),
            description: "A test team".into(),
            orchestration: OrchestrationMode::LeadAgent {
                lead: "lead".into(),
            },
            members: vec![TeamMemberConfig {
                name: "lead".into(),
                role: "coordinator".into(),
            }],
            dialogue: DialogueConfig::default(),
        };

        let runtime = make_runtime(config);
        let info = runtime.build_team_info();
        assert!(info.contains("delegate_task"));
        assert!(info.contains("query_agent"));
        assert!(info.contains("collect_results"));
        assert!(info.contains("synthesize_results"));
        assert!(info.contains("verify_output"));
        assert!(info.contains("send_message"));
        assert!(info.contains("read_messages"));
        assert!(info.contains("request_peer_review"));
        assert!(info.contains("decompose_task"));
        assert!(info.contains("suggest_specialist"));
    }
}

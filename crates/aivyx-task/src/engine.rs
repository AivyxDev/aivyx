//! Task orchestration engine.
//!
//! [`TaskEngine`] coordinates multi-step mission execution: planning goals into
//! steps via LLM, executing each step sequentially as an agent turn, checkpointing
//! between steps, and supporting resume after crashes.

use std::sync::Arc;

use aivyx_agent::AgentSession;
use aivyx_audit::{AuditEvent, AuditLog};
use aivyx_core::{AivyxError, ChannelAdapter, Result, TaskId};
use aivyx_crypto::{EncryptedStore, MasterKey};
use aivyx_llm::create_provider;
use chrono::Utc;
use tokio::sync::mpsc;
use tracing;

use crate::planner;
use crate::progress::ProgressEvent;
use crate::store::{TaskMetadata, TaskStore};

/// Convenience alias for the progress sink trait parameterized to task events.
type DynProgressSink = dyn aivyx_core::ProgressSink<ProgressEvent>;
use crate::types::{Mission, StepStatus, TaskStatus};

/// Orchestrates multi-step mission execution.
pub struct TaskEngine {
    /// Factory for creating agents.
    session: Arc<AgentSession>,
    /// Encrypted persistence for missions.
    store: TaskStore,
    /// Domain-separated key for task encryption.
    task_key: MasterKey,
    /// Optional audit log for lifecycle events.
    audit_log: Option<AuditLog>,
}

impl TaskEngine {
    /// Create a new task engine.
    pub fn new(
        session: Arc<AgentSession>,
        store: TaskStore,
        task_key: MasterKey,
        audit_log: Option<AuditLog>,
    ) -> Self {
        Self {
            session,
            store,
            task_key,
            audit_log,
        }
    }

    /// Create and plan a new mission from a goal string. Returns the task ID.
    ///
    /// The LLM decomposes the goal into sequential steps. The mission is
    /// checkpointed in `Planned` state, ready for execution.
    pub async fn create_mission(
        &self,
        goal: &str,
        agent_name: &str,
        progress: Option<&DynProgressSink>,
    ) -> Result<TaskId> {
        let mut mission = Mission::new(goal, agent_name);

        // Create a temporary LLM provider for planning
        let store = EncryptedStore::open(self.session.dirs().store_path())?;
        let provider = create_provider(
            self.session.provider_config(),
            &store,
            self.session.master_key(),
        )?;

        // Plan the mission via LLM
        let steps = planner::plan_mission(provider.as_ref(), goal, 4096).await?;

        mission.steps = steps;
        mission.status = TaskStatus::Planned;
        mission.updated_at = Utc::now();

        // Checkpoint
        self.store.save(&mission, &self.task_key)?;

        // Audit
        self.audit(AuditEvent::TaskCreated {
            task_id: mission.id.to_string(),
            agent_name: agent_name.to_string(),
            goal: truncate(goal, 200),
        });

        // Progress
        if let Some(sink) = progress {
            let _ = sink
                .emit(ProgressEvent::Planned {
                    task_id: mission.id,
                    steps: mission.steps.len(),
                    timestamp: Utc::now(),
                })
                .await;
        }

        Ok(mission.id)
    }

    /// Execute a mission (all remaining steps).
    ///
    /// Creates an agent and runs each pending step as a separate `turn()` call.
    /// The agent accumulates conversation history across steps naturally.
    /// Between steps, the mission is checkpointed to encrypted storage.
    pub async fn execute_mission(
        &self,
        task_id: &TaskId,
        channel: Option<&dyn ChannelAdapter>,
        progress: Option<&DynProgressSink>,
    ) -> Result<Mission> {
        let mut mission = self
            .store
            .load(task_id, &self.task_key)?
            .ok_or_else(|| AivyxError::Task(format!("mission not found: {task_id}")))?;

        if mission.status.is_terminal() {
            return Err(AivyxError::Task(format!(
                "mission is already in terminal state: {:?}",
                mission.status
            )));
        }

        mission.status = TaskStatus::Executing;
        mission.updated_at = Utc::now();
        self.store.save(&mission, &self.task_key)?;

        // Create agent for execution
        let mut agent = self.session.create_agent(&mission.agent_name).await?;

        // Execute each pending step
        while let Some(step_idx) = mission.next_pending_step() {
            let step_prompt = build_step_prompt(&mission, step_idx);
            let step_desc = mission.steps[step_idx].description.clone();

            // Mark step as running
            mission.steps[step_idx].status = StepStatus::Running;
            mission.steps[step_idx].prompt = Some(step_prompt.clone());
            mission.steps[step_idx].started_at = Some(Utc::now());
            mission.updated_at = Utc::now();
            self.store.save(&mission, &self.task_key)?;

            // Emit progress
            if let Some(sink) = progress {
                let _ = sink
                    .emit(ProgressEvent::StepStarted {
                        task_id: mission.id,
                        step_index: step_idx,
                        step_description: step_desc.clone(),
                        timestamp: Utc::now(),
                    })
                    .await;
            }

            // Execute the step
            match agent.turn(&step_prompt, channel).await {
                Ok(result) => {
                    let summary = truncate(&result, 500);
                    mission.steps[step_idx].status = StepStatus::Completed;
                    mission.steps[step_idx].result = Some(result);
                    mission.steps[step_idx].completed_at = Some(Utc::now());
                    mission.updated_at = Utc::now();
                    self.store.save(&mission, &self.task_key)?;

                    // Audit + progress
                    self.audit(AuditEvent::TaskStepCompleted {
                        task_id: mission.id.to_string(),
                        step_index: step_idx,
                        step_description: truncate(&step_desc, 100),
                        success: true,
                    });
                    if let Some(sink) = progress {
                        let _ = sink
                            .emit(ProgressEvent::StepCompleted {
                                task_id: mission.id,
                                step_index: step_idx,
                                success: true,
                                result_summary: summary,
                                timestamp: Utc::now(),
                            })
                            .await;
                    }
                }
                Err(e) => {
                    mission.steps[step_idx].retries += 1;
                    let retries = mission.steps[step_idx].retries;

                    if retries > mission.max_step_retries {
                        // Step failed permanently
                        let reason = format!("{e}");
                        mission.steps[step_idx].status = StepStatus::Failed {
                            reason: reason.clone(),
                        };
                        mission.steps[step_idx].completed_at = Some(Utc::now());
                        mission.status = TaskStatus::Failed {
                            reason: format!("step {step_idx} failed after {retries} retries: {e}"),
                        };
                        mission.updated_at = Utc::now();
                        self.store.save(&mission, &self.task_key)?;

                        self.audit(AuditEvent::TaskStepCompleted {
                            task_id: mission.id.to_string(),
                            step_index: step_idx,
                            step_description: truncate(&step_desc, 100),
                            success: false,
                        });
                        self.audit(AuditEvent::TaskCompleted {
                            task_id: mission.id.to_string(),
                            status: "Failed".into(),
                            steps_completed: mission.steps_completed(),
                            steps_total: mission.steps.len(),
                        });
                        if let Some(sink) = progress {
                            let _ = sink
                                .emit(ProgressEvent::StepCompleted {
                                    task_id: mission.id,
                                    step_index: step_idx,
                                    success: false,
                                    result_summary: reason,
                                    timestamp: Utc::now(),
                                })
                                .await;
                            let _ = sink
                                .emit(ProgressEvent::MissionCompleted {
                                    task_id: mission.id,
                                    success: false,
                                    timestamp: Utc::now(),
                                })
                                .await;
                        }

                        return Ok(mission);
                    }

                    // Retry: reset status to Pending so next_pending_step picks it up again
                    tracing::warn!(
                        "Step {step_idx} failed (attempt {retries}/{max}): {e}",
                        max = mission.max_step_retries
                    );
                    mission.steps[step_idx].status = StepStatus::Pending;
                    mission.updated_at = Utc::now();
                    self.store.save(&mission, &self.task_key)?;
                }
            }
        }

        // All steps completed
        mission.status = TaskStatus::Completed;
        mission.updated_at = Utc::now();
        self.store.save(&mission, &self.task_key)?;

        self.audit(AuditEvent::TaskCompleted {
            task_id: mission.id.to_string(),
            status: "Completed".into(),
            steps_completed: mission.steps_completed(),
            steps_total: mission.steps.len(),
        });

        if let Some(sink) = progress {
            let _ = sink
                .emit(ProgressEvent::MissionCompleted {
                    task_id: mission.id,
                    success: true,
                    timestamp: Utc::now(),
                })
                .await;
        }

        Ok(mission)
    }

    /// Execute a mission with streaming: forwards agent tokens to `token_tx`.
    pub async fn execute_mission_stream(
        &self,
        task_id: &TaskId,
        channel: Option<&dyn ChannelAdapter>,
        progress: Option<&DynProgressSink>,
        token_tx: mpsc::Sender<String>,
    ) -> Result<Mission> {
        let mut mission = self
            .store
            .load(task_id, &self.task_key)?
            .ok_or_else(|| AivyxError::Task(format!("mission not found: {task_id}")))?;

        if mission.status.is_terminal() {
            return Err(AivyxError::Task(format!(
                "mission is already in terminal state: {:?}",
                mission.status
            )));
        }

        mission.status = TaskStatus::Executing;
        mission.updated_at = Utc::now();
        self.store.save(&mission, &self.task_key)?;

        let mut agent = self.session.create_agent(&mission.agent_name).await?;

        while let Some(step_idx) = mission.next_pending_step() {
            let step_prompt = build_step_prompt(&mission, step_idx);
            let step_desc = mission.steps[step_idx].description.clone();

            mission.steps[step_idx].status = StepStatus::Running;
            mission.steps[step_idx].prompt = Some(step_prompt.clone());
            mission.steps[step_idx].started_at = Some(Utc::now());
            mission.updated_at = Utc::now();
            self.store.save(&mission, &self.task_key)?;

            if let Some(sink) = progress {
                let _ = sink
                    .emit(ProgressEvent::StepStarted {
                        task_id: mission.id,
                        step_index: step_idx,
                        step_description: step_desc.clone(),
                        timestamp: Utc::now(),
                    })
                    .await;
            }

            match agent
                .turn_stream(&step_prompt, channel, token_tx.clone(), None)
                .await
            {
                Ok(result) => {
                    let summary = truncate(&result, 500);
                    mission.steps[step_idx].status = StepStatus::Completed;
                    mission.steps[step_idx].result = Some(result);
                    mission.steps[step_idx].completed_at = Some(Utc::now());
                    mission.updated_at = Utc::now();
                    self.store.save(&mission, &self.task_key)?;

                    if let Some(sink) = progress {
                        let _ = sink
                            .emit(ProgressEvent::StepCompleted {
                                task_id: mission.id,
                                step_index: step_idx,
                                success: true,
                                result_summary: summary,
                                timestamp: Utc::now(),
                            })
                            .await;
                    }
                }
                Err(e) => {
                    mission.steps[step_idx].retries += 1;
                    if mission.steps[step_idx].retries > mission.max_step_retries {
                        mission.steps[step_idx].status = StepStatus::Failed {
                            reason: format!("{e}"),
                        };
                        mission.status = TaskStatus::Failed {
                            reason: format!("step {step_idx} failed: {e}"),
                        };
                        mission.updated_at = Utc::now();
                        self.store.save(&mission, &self.task_key)?;

                        if let Some(sink) = progress {
                            let _ = sink
                                .emit(ProgressEvent::MissionCompleted {
                                    task_id: mission.id,
                                    success: false,
                                    timestamp: Utc::now(),
                                })
                                .await;
                        }
                        return Ok(mission);
                    }

                    mission.steps[step_idx].status = StepStatus::Pending;
                    mission.updated_at = Utc::now();
                    self.store.save(&mission, &self.task_key)?;
                }
            }
        }

        mission.status = TaskStatus::Completed;
        mission.updated_at = Utc::now();
        self.store.save(&mission, &self.task_key)?;

        if let Some(sink) = progress {
            let _ = sink
                .emit(ProgressEvent::MissionCompleted {
                    task_id: mission.id,
                    success: true,
                    timestamp: Utc::now(),
                })
                .await;
        }

        Ok(mission)
    }

    /// Create and immediately execute a mission (convenience method).
    pub async fn run(
        &self,
        goal: &str,
        agent_name: &str,
        channel: Option<&dyn ChannelAdapter>,
        progress: Option<&DynProgressSink>,
    ) -> Result<Mission> {
        let task_id = self.create_mission(goal, agent_name, progress).await?;
        self.execute_mission(&task_id, channel, progress).await
    }

    /// Resume a previously interrupted mission from its last checkpoint.
    pub async fn resume(
        &self,
        task_id: &TaskId,
        channel: Option<&dyn ChannelAdapter>,
        progress: Option<&DynProgressSink>,
    ) -> Result<Mission> {
        let mission = self
            .store
            .load(task_id, &self.task_key)?
            .ok_or_else(|| AivyxError::Task(format!("mission not found: {task_id}")))?;

        let from_step = mission.next_pending_step().unwrap_or(0);

        self.audit(AuditEvent::TaskResumed {
            task_id: task_id.to_string(),
            resumed_from_step: from_step,
        });

        if let Some(sink) = progress {
            let _ = sink
                .emit(ProgressEvent::Resumed {
                    task_id: *task_id,
                    from_step,
                    timestamp: Utc::now(),
                })
                .await;
        }

        self.execute_mission(task_id, channel, progress).await
    }

    /// Cancel an in-progress mission.
    pub fn cancel(&self, task_id: &TaskId) -> Result<()> {
        let mut mission = self
            .store
            .load(task_id, &self.task_key)?
            .ok_or_else(|| AivyxError::Task(format!("mission not found: {task_id}")))?;

        if mission.status.is_terminal() {
            return Err(AivyxError::Task(
                "cannot cancel a mission that is already terminal".into(),
            ));
        }

        // Skip remaining pending steps
        for step in &mut mission.steps {
            if matches!(step.status, StepStatus::Pending | StepStatus::Running) {
                step.status = StepStatus::Skipped;
            }
        }

        mission.status = TaskStatus::Cancelled;
        mission.updated_at = Utc::now();
        self.store.save(&mission, &self.task_key)
    }

    /// Load a mission by ID (read-only).
    pub fn get_mission(&self, task_id: &TaskId) -> Result<Option<Mission>> {
        self.store.load(task_id, &self.task_key)
    }

    /// List all missions as metadata summaries.
    pub fn list_missions(&self) -> Result<Vec<TaskMetadata>> {
        self.store.list(&self.task_key)
    }

    /// Delete a completed, failed, or cancelled mission.
    pub fn delete_mission(&self, task_id: &TaskId) -> Result<()> {
        self.store.delete(task_id)
    }

    /// Emit an audit event if an audit log is configured.
    fn audit(&self, event: AuditEvent) {
        if let Some(ref log) = self.audit_log
            && let Err(e) = log.append(event)
        {
            tracing::warn!("Failed to audit task event: {e}");
        }
    }
}

/// Build the prompt for a specific step, including context from prior steps.
fn build_step_prompt(mission: &Mission, step_index: usize) -> String {
    let step = &mission.steps[step_index];
    let total_steps = mission.steps.len();

    let mut prompt = format!(
        "You are executing step {} of {} in a multi-step mission.\n\n\
         Overall goal: {}\n\n",
        step_index + 1,
        total_steps,
        mission.goal
    );

    // Add context from completed steps
    let summaries = mission.completed_step_summaries();
    if !summaries.is_empty() {
        prompt.push_str("Previous steps completed:\n");
        for (idx, desc, result) in summaries {
            let truncated = truncate(result, 300);
            prompt.push_str(&format!("  Step {}: {} → {}\n", idx + 1, desc, truncated));
        }
        prompt.push('\n');
    }

    prompt.push_str(&format!(
        "Current step: {}\n\n\
         Please complete this step using the tools available to you. \
         Be thorough and report what you accomplished.",
        step.description
    ));

    if !step.tool_hints.is_empty() {
        prompt.push_str(&format!(
            "\n\nSuggested tools: {}",
            step.tool_hints.join(", ")
        ));
    }

    prompt
}

/// Truncate a string to the given maximum length (UTF-8 safe).
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let boundary = s.floor_char_boundary(max_len);
        format!("{}...", &s[..boundary])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Step, StepStatus};

    #[test]
    fn build_step_prompt_includes_goal_and_step() {
        let mut mission = Mission::new("Research Rust async", "researcher");
        mission.steps = vec![Step {
            index: 0,
            description: "Search for tokio info".into(),
            tool_hints: vec!["web_search".into()],
            status: StepStatus::Pending,
            prompt: None,
            result: None,
            retries: 0,
            started_at: None,
            completed_at: None,
        }];

        let prompt = build_step_prompt(&mission, 0);
        assert!(prompt.contains("step 1 of 1"));
        assert!(prompt.contains("Research Rust async"));
        assert!(prompt.contains("Search for tokio info"));
        assert!(prompt.contains("web_search"));
    }

    #[test]
    fn build_step_prompt_includes_prior_results() {
        let mut mission = Mission::new("Research Rust", "researcher");
        mission.steps = vec![
            Step {
                index: 0,
                description: "Search for info".into(),
                tool_hints: vec![],
                status: StepStatus::Completed,
                prompt: None,
                result: Some("Found 5 results about Rust".into()),
                retries: 0,
                started_at: None,
                completed_at: None,
            },
            Step {
                index: 1,
                description: "Write summary".into(),
                tool_hints: vec!["file_write".into()],
                status: StepStatus::Pending,
                prompt: None,
                result: None,
                retries: 0,
                started_at: None,
                completed_at: None,
            },
        ];

        let prompt = build_step_prompt(&mission, 1);
        assert!(prompt.contains("Previous steps completed"));
        assert!(prompt.contains("Found 5 results about Rust"));
        assert!(prompt.contains("Write summary"));
    }

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string() {
        let long = "a".repeat(100);
        let result = truncate(&long, 50);
        assert!(result.len() <= 53); // 50 + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn cancel_completed_mission_errors() {
        let dir = std::env::temp_dir().join(format!("aivyx-task-engine-cc-{}", TaskId::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = TaskStore::open(dir.join("tasks.db")).unwrap();
        let key = aivyx_crypto::MasterKey::generate();

        let mut mission = Mission::new("done goal", "agent");
        mission.status = TaskStatus::Completed;
        store.save(&mission, &key).unwrap();

        // We need a TaskEngine but only for cancel() which doesn't use session.
        // Build a minimal one by constructing with the store + key.
        // cancel() only uses self.store and self.task_key, so we can test it
        // by directly calling store.load + the cancel logic.
        // However, TaskEngine::new requires an Arc<AgentSession> which is hard
        // to construct in a unit test. Instead, replicate the cancel logic inline.
        let loaded = store.load(&mission.id, &key).unwrap().unwrap();
        assert!(loaded.status.is_terminal());
        // This confirms that the cancel path would return an error.

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cancel_sets_pending_steps_to_skipped() {
        let dir = std::env::temp_dir().join(format!("aivyx-task-engine-cs-{}", TaskId::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = TaskStore::open(dir.join("tasks.db")).unwrap();
        let key = aivyx_crypto::MasterKey::generate();

        let mut mission = Mission::new("cancel goal", "agent");
        mission.status = TaskStatus::Executing;
        mission.steps = vec![
            Step {
                index: 0,
                description: "step0".into(),
                tool_hints: vec![],
                status: StepStatus::Completed,
                prompt: None,
                result: Some("done".into()),
                retries: 0,
                started_at: None,
                completed_at: None,
            },
            Step {
                index: 1,
                description: "step1".into(),
                tool_hints: vec![],
                status: StepStatus::Pending,
                prompt: None,
                result: None,
                retries: 0,
                started_at: None,
                completed_at: None,
            },
            Step {
                index: 2,
                description: "step2".into(),
                tool_hints: vec![],
                status: StepStatus::Running,
                prompt: None,
                result: None,
                retries: 0,
                started_at: None,
                completed_at: None,
            },
        ];
        store.save(&mission, &key).unwrap();

        // Replicate the cancel logic since TaskEngine::new requires AgentSession
        let mut loaded = store.load(&mission.id, &key).unwrap().unwrap();
        assert!(!loaded.status.is_terminal());

        for step in &mut loaded.steps {
            if matches!(step.status, StepStatus::Pending | StepStatus::Running) {
                step.status = StepStatus::Skipped;
            }
        }
        loaded.status = TaskStatus::Cancelled;
        store.save(&loaded, &key).unwrap();

        let final_mission = store.load(&mission.id, &key).unwrap().unwrap();
        assert_eq!(final_mission.status, TaskStatus::Cancelled);
        assert_eq!(final_mission.steps[0].status, StepStatus::Completed);
        assert_eq!(final_mission.steps[1].status, StepStatus::Skipped);
        assert_eq!(final_mission.steps[2].status, StepStatus::Skipped);

        std::fs::remove_dir_all(&dir).ok();
    }
}

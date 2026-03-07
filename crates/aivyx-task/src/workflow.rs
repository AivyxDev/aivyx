//! Multi-stage workflow orchestration.
//!
//! A [`Workflow`] is a sequence of [`WorkflowStage`]s, each running a mission
//! with an agent. Stages advance based on [`StageCondition`] — allowing
//! conditional branching on success or failure of prior stages.
//!
//! Workflows are persisted in [`WorkflowStore`] (EncryptedStore-backed) and
//! can be paused between stages for durable execution.

use std::path::Path;

use aivyx_core::{AivyxError, Result, TaskId};
use aivyx_crypto::{EncryptedStore, MasterKey};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Unique identifier for a workflow instance.
pub type WorkflowId = TaskId;

/// Lifecycle status of a workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state")]
pub enum WorkflowStatus {
    /// Workflow is actively executing stages.
    Running,
    /// Paused between stages (can be resumed).
    Paused,
    /// All stages completed successfully.
    Completed,
    /// A stage failed and no fallback condition matched.
    Failed { reason: String },
    /// Workflow was cancelled by the user.
    Cancelled,
}

impl WorkflowStatus {
    /// Whether the workflow is in a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            WorkflowStatus::Completed | WorkflowStatus::Failed { .. } | WorkflowStatus::Cancelled
        )
    }
}

/// Condition that must be met for a stage to execute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum StageCondition {
    /// Always run this stage (default for the first stage).
    #[default]
    Always,
    /// Run only if the previous stage succeeded.
    OnSuccess,
    /// Run only if the previous stage failed.
    OnFailure,
}


/// A single stage within a workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStage {
    /// Human-readable stage name.
    pub name: String,
    /// Agent profile to use for this stage.
    pub agent: String,
    /// Prompt for the agent (may reference `{prev_result}` placeholder).
    pub prompt: String,
    /// Condition that must be met for this stage to execute.
    #[serde(default)]
    pub condition: StageCondition,
    /// Result of executing this stage (set after completion).
    pub result: Option<StageResult>,
}

/// The outcome of a completed workflow stage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageResult {
    /// Whether the stage succeeded.
    pub success: bool,
    /// The agent's output text.
    pub output: String,
    /// When the stage completed.
    pub completed_at: DateTime<Utc>,
}

/// A multi-stage workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    /// Unique workflow identifier.
    pub id: WorkflowId,
    /// Human-readable workflow name.
    pub name: String,
    /// Ordered list of stages.
    pub stages: Vec<WorkflowStage>,
    /// Index of the current stage (0-based).
    pub current_stage: usize,
    /// Lifecycle status.
    pub status: WorkflowStatus,
    /// When the workflow was created.
    pub created_at: DateTime<Utc>,
    /// When the workflow was last updated.
    pub updated_at: DateTime<Utc>,
}

impl Workflow {
    /// Create a new workflow with the given name and stages.
    pub fn new(name: &str, stages: Vec<WorkflowStage>) -> Self {
        let now = Utc::now();
        Self {
            id: WorkflowId::new(),
            name: name.to_string(),
            stages,
            current_stage: 0,
            status: WorkflowStatus::Running,
            created_at: now,
            updated_at: now,
        }
    }

    /// Check if the current stage's condition is met.
    pub fn should_run_current_stage(&self) -> bool {
        let stage = match self.stages.get(self.current_stage) {
            Some(s) => s,
            None => return false,
        };

        if self.current_stage == 0 {
            return true; // First stage always runs
        }

        let prev = &self.stages[self.current_stage - 1];
        match (&stage.condition, &prev.result) {
            (StageCondition::Always, _) => true,
            (StageCondition::OnSuccess, Some(r)) => r.success,
            (StageCondition::OnFailure, Some(r)) => !r.success,
            (_, None) => false, // Previous stage hasn't completed
        }
    }

    /// Get the result of the previous stage (for template interpolation).
    pub fn prev_result(&self) -> Option<&str> {
        if self.current_stage == 0 {
            return None;
        }
        self.stages
            .get(self.current_stage - 1)
            .and_then(|s| s.result.as_ref())
            .map(|r| r.output.as_str())
    }

    /// Return the current stage's prompt with `{prev_result}` substituted.
    ///
    /// If there is a previous stage result, its output replaces every
    /// occurrence of `{prev_result}`. Otherwise the placeholder is
    /// replaced with `"(no previous result)"`.
    ///
    /// Returns `None` when `current_stage` is out of bounds.
    pub fn interpolated_prompt(&self) -> Option<String> {
        let stage = self.stages.get(self.current_stage)?;
        let prompt = if let Some(prev) = self.prev_result() {
            stage.prompt.replace("{prev_result}", prev)
        } else {
            stage.prompt.replace("{prev_result}", "(no previous result)")
        };
        Some(prompt)
    }

    /// Number of stages.
    pub fn stage_count(&self) -> usize {
        self.stages.len()
    }
}

/// Summary metadata for listing workflows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowMetadata {
    pub id: WorkflowId,
    pub name: String,
    pub status: WorkflowStatus,
    pub current_stage: usize,
    pub total_stages: usize,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&Workflow> for WorkflowMetadata {
    fn from(w: &Workflow) -> Self {
        Self {
            id: w.id,
            name: w.name.clone(),
            status: w.status.clone(),
            current_stage: w.current_stage,
            total_stages: w.stages.len(),
            created_at: w.created_at,
            updated_at: w.updated_at,
        }
    }
}

/// Key prefix for workflow records in `EncryptedStore`.
const WORKFLOW_PREFIX: &str = "workflow:";

/// Encrypted persistence for workflows.
pub struct WorkflowStore {
    store: EncryptedStore,
}

impl WorkflowStore {
    /// Open or create a workflow store at the given path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            store: EncryptedStore::open(path)?,
        })
    }

    /// Save a workflow (create or update).
    pub fn save(&self, workflow: &Workflow, key: &MasterKey) -> Result<()> {
        let store_key = format!("{}{}", WORKFLOW_PREFIX, workflow.id);
        let json = serde_json::to_vec(workflow).map_err(AivyxError::Serialization)?;
        self.store.put(&store_key, &json, key)
    }

    /// Load a workflow by ID.
    pub fn load(&self, id: &WorkflowId, key: &MasterKey) -> Result<Option<Workflow>> {
        let store_key = format!("{}{}", WORKFLOW_PREFIX, id);
        match self.store.get(&store_key, key)? {
            Some(bytes) => {
                let wf: Workflow =
                    serde_json::from_slice(&bytes).map_err(AivyxError::Serialization)?;
                Ok(Some(wf))
            }
            None => Ok(None),
        }
    }

    /// List all workflows as metadata summaries.
    pub fn list(&self, key: &MasterKey) -> Result<Vec<WorkflowMetadata>> {
        let keys = self.store.list_keys()?;
        let mut result = Vec::new();
        for k in keys {
            if let Some(id_str) = k.strip_prefix(WORKFLOW_PREFIX) {
                let id: WorkflowId = id_str
                    .parse()
                    .map_err(|e| AivyxError::Storage(format!("invalid workflow ID: {e}")))?;
                if let Some(wf) = self.load(&id, key)? {
                    result.push(WorkflowMetadata::from(&wf));
                }
            }
        }
        Ok(result)
    }

    /// Delete a workflow by ID.
    pub fn delete(&self, id: &WorkflowId) -> Result<()> {
        let store_key = format!("{}{}", WORKFLOW_PREFIX, id);
        self.store.delete(&store_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_stages() -> Vec<WorkflowStage> {
        vec![
            WorkflowStage {
                name: "research".into(),
                agent: "researcher".into(),
                prompt: "Research the topic".into(),
                condition: StageCondition::Always,
                result: None,
            },
            WorkflowStage {
                name: "write".into(),
                agent: "writer".into(),
                prompt: "Write based on: {prev_result}".into(),
                condition: StageCondition::OnSuccess,
                result: None,
            },
            WorkflowStage {
                name: "handle-error".into(),
                agent: "fixer".into(),
                prompt: "Fix the error: {prev_result}".into(),
                condition: StageCondition::OnFailure,
                result: None,
            },
        ]
    }

    #[test]
    fn workflow_new() {
        let wf = Workflow::new("test-workflow", test_stages());
        assert_eq!(wf.name, "test-workflow");
        assert_eq!(wf.stage_count(), 3);
        assert_eq!(wf.current_stage, 0);
        assert_eq!(wf.status, WorkflowStatus::Running);
    }

    #[test]
    fn first_stage_always_runs() {
        let wf = Workflow::new("test", test_stages());
        assert!(wf.should_run_current_stage());
    }

    #[test]
    fn on_success_runs_after_success() {
        let mut wf = Workflow::new("test", test_stages());
        wf.stages[0].result = Some(StageResult {
            success: true,
            output: "found data".into(),
            completed_at: Utc::now(),
        });
        wf.current_stage = 1;
        assert!(wf.should_run_current_stage());
    }

    #[test]
    fn on_success_skips_after_failure() {
        let mut wf = Workflow::new("test", test_stages());
        wf.stages[0].result = Some(StageResult {
            success: false,
            output: "error occurred".into(),
            completed_at: Utc::now(),
        });
        wf.current_stage = 1;
        assert!(!wf.should_run_current_stage());
    }

    #[test]
    fn on_failure_runs_after_failure() {
        let mut wf = Workflow::new("test", test_stages());
        wf.stages[0].result = Some(StageResult {
            success: false,
            output: "error occurred".into(),
            completed_at: Utc::now(),
        });
        // Stage 2 has OnFailure condition
        wf.current_stage = 2;
        // Need to set stage 1 result too (OnFailure checks previous stage = stage 1)
        wf.stages[1].result = Some(StageResult {
            success: false,
            output: "also failed".into(),
            completed_at: Utc::now(),
        });
        assert!(wf.should_run_current_stage());
    }

    #[test]
    fn prev_result_returns_output() {
        let mut wf = Workflow::new("test", test_stages());
        wf.stages[0].result = Some(StageResult {
            success: true,
            output: "research findings".into(),
            completed_at: Utc::now(),
        });
        wf.current_stage = 1;
        assert_eq!(wf.prev_result(), Some("research findings"));
    }

    #[test]
    fn prev_result_none_for_first_stage() {
        let wf = Workflow::new("test", test_stages());
        assert!(wf.prev_result().is_none());
    }

    #[test]
    fn workflow_status_terminal() {
        assert!(!WorkflowStatus::Running.is_terminal());
        assert!(!WorkflowStatus::Paused.is_terminal());
        assert!(WorkflowStatus::Completed.is_terminal());
        assert!(WorkflowStatus::Failed { reason: "x".into() }.is_terminal());
        assert!(WorkflowStatus::Cancelled.is_terminal());
    }

    #[test]
    fn workflow_serde_roundtrip() {
        let wf = Workflow::new("serde-test", test_stages());
        let json = serde_json::to_string(&wf).unwrap();
        let restored: Workflow = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.name, "serde-test");
        assert_eq!(restored.stages.len(), 3);
        assert_eq!(restored.status, WorkflowStatus::Running);
    }

    #[test]
    fn workflow_store_save_load() {
        let dir = std::env::temp_dir().join(format!("aivyx-wf-{}", TaskId::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = WorkflowStore::open(dir.join("workflows.db")).unwrap();
        let key = MasterKey::generate();

        let wf = Workflow::new("test-wf", test_stages());
        let wf_id = wf.id;
        store.save(&wf, &key).unwrap();

        let loaded = store.load(&wf_id, &key).unwrap().unwrap();
        assert_eq!(loaded.name, "test-wf");
        assert_eq!(loaded.stages.len(), 3);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn workflow_store_list_and_delete() {
        let dir = std::env::temp_dir().join(format!("aivyx-wf-ld-{}", TaskId::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = WorkflowStore::open(dir.join("workflows.db")).unwrap();
        let key = MasterKey::generate();

        let wf1 = Workflow::new("wf-1", test_stages());
        let wf2 = Workflow::new("wf-2", test_stages());
        store.save(&wf1, &key).unwrap();
        store.save(&wf2, &key).unwrap();

        let list = store.list(&key).unwrap();
        assert_eq!(list.len(), 2);

        store.delete(&wf1.id).unwrap();
        let list = store.list(&key).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "wf-2");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn workflow_metadata_from_workflow() {
        let wf = Workflow::new("meta-test", test_stages());
        let meta = WorkflowMetadata::from(&wf);
        assert_eq!(meta.name, "meta-test");
        assert_eq!(meta.total_stages, 3);
        assert_eq!(meta.current_stage, 0);
    }

    #[test]
    fn stage_condition_serde() {
        for cond in [
            StageCondition::Always,
            StageCondition::OnSuccess,
            StageCondition::OnFailure,
        ] {
            let json = serde_json::to_string(&cond).unwrap();
            let restored: StageCondition = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, cond);
        }
    }

    #[test]
    fn interpolated_prompt_substitutes_prev_result() {
        let mut wf = Workflow::new("test", test_stages());
        // Set stage 0 result
        wf.stages[0].result = Some(StageResult {
            success: true,
            output: "research findings".into(),
            completed_at: Utc::now(),
        });
        // Advance to stage 1 whose prompt contains {prev_result}
        wf.current_stage = 1;
        let prompt = wf.interpolated_prompt().unwrap();
        assert_eq!(prompt, "Write based on: research findings");
    }

    #[test]
    fn interpolated_prompt_no_prev_result_at_first_stage() {
        let wf = Workflow::new("test", test_stages());
        // Stage 0 prompt has no {prev_result} but the method still works
        let prompt = wf.interpolated_prompt().unwrap();
        assert_eq!(prompt, "Research the topic");

        // Build a workflow where stage 0 explicitly uses {prev_result}
        let stages = vec![WorkflowStage {
            name: "first".into(),
            agent: "a".into(),
            prompt: "Do something with {prev_result}".into(),
            condition: StageCondition::Always,
            result: None,
        }];
        let wf2 = Workflow::new("test2", stages);
        let prompt2 = wf2.interpolated_prompt().unwrap();
        assert_eq!(prompt2, "Do something with (no previous result)");
    }

    #[test]
    fn interpolated_prompt_out_of_bounds_returns_none() {
        let mut wf = Workflow::new("test", test_stages());
        wf.current_stage = 99;
        assert!(wf.interpolated_prompt().is_none());
    }

    #[test]
    fn workflow_status_serde() {
        let statuses = vec![
            WorkflowStatus::Running,
            WorkflowStatus::Paused,
            WorkflowStatus::Completed,
            WorkflowStatus::Failed {
                reason: "timeout".into(),
            },
            WorkflowStatus::Cancelled,
        ];
        for s in &statuses {
            let json = serde_json::to_string(s).unwrap();
            let restored: WorkflowStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(&restored, s);
        }
    }
}

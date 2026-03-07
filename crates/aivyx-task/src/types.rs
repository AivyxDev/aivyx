//! Core data types for the task orchestration engine.
//!
//! Defines the [`Mission`] (a multi-step goal), [`Step`] (an individual action),
//! and their lifecycle status enums.

use aivyx_core::TaskId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The lifecycle state of a mission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state")]
pub enum TaskStatus {
    /// LLM is decomposing the goal into steps.
    Planning,
    /// Steps have been created, awaiting execution.
    Planned,
    /// Steps are being executed sequentially.
    Executing,
    /// All steps done, running a verification step.
    Verifying,
    /// Mission completed successfully.
    Completed,
    /// Mission failed (with reason).
    Failed { reason: String },
    /// Mission was cancelled by user.
    Cancelled,
}

impl TaskStatus {
    /// Whether the mission is in a terminal state (Completed, Failed, or Cancelled).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TaskStatus::Completed | TaskStatus::Failed { .. } | TaskStatus::Cancelled
        )
    }
}

/// The state of a single step within a mission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state")]
pub enum StepStatus {
    /// Waiting to be executed.
    Pending,
    /// Currently running.
    Running,
    /// Finished successfully.
    Completed,
    /// Failed (will be retried or mission fails).
    Failed { reason: String },
    /// Skipped (e.g., due to cancellation).
    Skipped,
}

/// A single step in a mission plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    /// Zero-based index within the mission.
    pub index: usize,
    /// Human-readable description of what this step does.
    pub description: String,
    /// Hint about which tools the agent should use (informational only).
    pub tool_hints: Vec<String>,
    /// Current status.
    pub status: StepStatus,
    /// The prompt sent to the agent for this step.
    pub prompt: Option<String>,
    /// The agent's response after execution.
    pub result: Option<String>,
    /// Number of retry attempts so far.
    pub retries: u32,
    /// When this step started executing.
    pub started_at: Option<DateTime<Utc>>,
    /// When this step completed.
    pub completed_at: Option<DateTime<Utc>>,
}

/// Default maximum retries per step.
pub const DEFAULT_MAX_STEP_RETRIES: u32 = 2;

/// A multi-step mission representing a high-level goal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mission {
    /// Unique task identifier.
    pub id: TaskId,
    /// The original goal from the user.
    pub goal: String,
    /// Agent profile used for execution.
    pub agent_name: String,
    /// Current lifecycle status.
    pub status: TaskStatus,
    /// Ordered list of steps (flat, sequential).
    pub steps: Vec<Step>,
    /// Maximum retries per step before the mission fails.
    pub max_step_retries: u32,
    /// When the mission was created.
    pub created_at: DateTime<Utc>,
    /// When the mission was last updated (checkpoint time).
    pub updated_at: DateTime<Utc>,
}

impl Mission {
    /// Create a new mission in Planning state.
    pub fn new(goal: &str, agent_name: &str) -> Self {
        let now = Utc::now();
        Self {
            id: TaskId::new(),
            goal: goal.to_string(),
            agent_name: agent_name.to_string(),
            status: TaskStatus::Planning,
            steps: Vec::new(),
            max_step_retries: DEFAULT_MAX_STEP_RETRIES,
            created_at: now,
            updated_at: now,
        }
    }

    /// Number of steps that have completed successfully.
    pub fn steps_completed(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| matches!(s.status, StepStatus::Completed))
            .count()
    }

    /// Index of the first step that is not yet completed.
    /// Returns `None` if all steps are completed.
    pub fn next_pending_step(&self) -> Option<usize> {
        self.steps.iter().position(|s| {
            matches!(
                s.status,
                StepStatus::Pending | StepStatus::Running | StepStatus::Failed { .. }
            )
        })
    }

    /// Collect summaries of completed steps for context injection.
    pub fn completed_step_summaries(&self) -> Vec<(usize, &str, &str)> {
        self.steps
            .iter()
            .filter(|s| matches!(s.status, StepStatus::Completed))
            .filter_map(|s| {
                s.result
                    .as_deref()
                    .map(|r| (s.index, s.description.as_str(), r))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mission_new_has_planning_status() {
        let m = Mission::new("test goal", "agent1");
        assert_eq!(m.status, TaskStatus::Planning);
        assert!(m.steps.is_empty());
        assert_eq!(m.goal, "test goal");
        assert_eq!(m.agent_name, "agent1");
    }

    #[test]
    fn task_status_is_terminal() {
        assert!(!TaskStatus::Planning.is_terminal());
        assert!(!TaskStatus::Executing.is_terminal());
        assert!(TaskStatus::Completed.is_terminal());
        assert!(TaskStatus::Failed { reason: "x".into() }.is_terminal());
        assert!(TaskStatus::Cancelled.is_terminal());
    }

    #[test]
    fn next_pending_step_finds_first_non_completed() {
        let mut m = Mission::new("g", "a");
        m.steps = vec![
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
        ];
        assert_eq!(m.next_pending_step(), Some(1));
        assert_eq!(m.steps_completed(), 1);
    }

    #[test]
    fn mission_serde_roundtrip() {
        let m = Mission::new("research rust", "researcher");
        let json = serde_json::to_string(&m).unwrap();
        let restored: Mission = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.goal, "research rust");
        assert_eq!(restored.status, TaskStatus::Planning);
    }

    #[test]
    fn step_status_serde_roundtrip() {
        let statuses = vec![
            StepStatus::Pending,
            StepStatus::Running,
            StepStatus::Completed,
            StepStatus::Failed {
                reason: "timeout".into(),
            },
            StepStatus::Skipped,
        ];
        for s in &statuses {
            let json = serde_json::to_string(s).unwrap();
            let restored: StepStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(&restored, s);
        }
    }

    #[test]
    fn task_status_all_variants_serde() {
        let variants = vec![
            TaskStatus::Planning,
            TaskStatus::Planned,
            TaskStatus::Executing,
            TaskStatus::Verifying,
            TaskStatus::Completed,
            TaskStatus::Failed {
                reason: "out of memory".into(),
            },
            TaskStatus::Cancelled,
        ];
        for v in &variants {
            let json = serde_json::to_string(v).unwrap();
            let restored: TaskStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(&restored, v);
        }
    }

    #[test]
    fn step_status_all_variants_serde() {
        let variants = vec![
            StepStatus::Pending,
            StepStatus::Running,
            StepStatus::Completed,
            StepStatus::Failed {
                reason: "network error".into(),
            },
            StepStatus::Skipped,
        ];
        for v in &variants {
            let json = serde_json::to_string(v).unwrap();
            let restored: StepStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(&restored, v);
        }
    }

    #[test]
    fn mission_next_pending_step_all_completed() {
        let mut m = Mission::new("goal", "agent");
        m.steps = vec![
            Step {
                index: 0,
                description: "s0".into(),
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
                description: "s1".into(),
                tool_hints: vec![],
                status: StepStatus::Completed,
                prompt: None,
                result: Some("done".into()),
                retries: 0,
                started_at: None,
                completed_at: None,
            },
        ];
        assert!(m.next_pending_step().is_none());
    }

    #[test]
    fn mission_next_pending_step_finds_first() {
        let mut m = Mission::new("goal", "agent");
        m.steps = vec![
            Step {
                index: 0,
                description: "s0".into(),
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
                description: "s1".into(),
                tool_hints: vec![],
                status: StepStatus::Completed,
                prompt: None,
                result: Some("done".into()),
                retries: 0,
                started_at: None,
                completed_at: None,
            },
            Step {
                index: 2,
                description: "s2".into(),
                tool_hints: vec![],
                status: StepStatus::Pending,
                prompt: None,
                result: None,
                retries: 0,
                started_at: None,
                completed_at: None,
            },
            Step {
                index: 3,
                description: "s3".into(),
                tool_hints: vec![],
                status: StepStatus::Pending,
                prompt: None,
                result: None,
                retries: 0,
                started_at: None,
                completed_at: None,
            },
        ];
        assert_eq!(m.next_pending_step(), Some(2));
    }

    #[test]
    fn mission_completed_step_summaries() {
        let mut m = Mission::new("goal", "agent");
        m.steps = vec![
            Step {
                index: 0,
                description: "search".into(),
                tool_hints: vec![],
                status: StepStatus::Completed,
                prompt: None,
                result: Some("found 5 results".into()),
                retries: 0,
                started_at: None,
                completed_at: None,
            },
            Step {
                index: 1,
                description: "analyze".into(),
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
                description: "write".into(),
                tool_hints: vec![],
                status: StepStatus::Completed,
                prompt: None,
                result: Some("wrote report".into()),
                retries: 0,
                started_at: None,
                completed_at: None,
            },
        ];
        let summaries = m.completed_step_summaries();
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0], (0, "search", "found 5 results"));
        assert_eq!(summaries[1], (2, "write", "wrote report"));
    }
}

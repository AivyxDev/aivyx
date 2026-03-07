//! Task orchestration engine for the aivyx framework.
//!
//! Provides multi-step mission planning, sequential execution with checkpoints,
//! encrypted persistence, and progress event streaming.
//!
//! # Architecture
//!
//! The engine decomposes a high-level goal into sequential steps via LLM
//! ([`planner::plan_mission`]), then executes each step as an agent turn.
//! Between steps, the entire mission state is checkpointed to encrypted
//! storage ([`TaskStore`]). If the process crashes, the mission can be
//! resumed from the last checkpoint.
//!
//! ```text
//! Goal → [Planning] → [Step 1] → [Step 2] → ... → [Completed]
//!                        ↓           ↓
//!                    checkpoint   checkpoint
//! ```
//!
//! # Example
//!
//! ```rust,no_run
//! use aivyx_task::TaskEngine;
//!
//! # async fn example(engine: &TaskEngine) -> aivyx_core::Result<()> {
//! let mission = engine.run("Research Rust async and write a summary", "researcher", None, None).await?;
//! println!("Mission completed with {} steps", mission.steps.len());
//! # Ok(())
//! # }
//! ```

pub mod dag;
pub mod engine;
pub mod feedback;
pub mod planner;
pub mod progress;
pub mod skill_scoring;
pub mod store;
pub mod types;
pub mod workflow;

pub use engine::TaskEngine;
pub use progress::{ChannelProgressSink, ProgressEvent};
pub use store::{TaskMetadata, TaskStore};
pub use types::{ExecutionMode, Mission, Step, StepKind, StepStatus, TaskStatus};
pub use workflow::{
    StageCondition, StageResult, Workflow, WorkflowId, WorkflowMetadata, WorkflowStage,
    WorkflowStatus, WorkflowStore,
};

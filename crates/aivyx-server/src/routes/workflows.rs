//! Workflow management endpoints.
//!
//! - `POST   /workflows`             — create and start a workflow
//! - `GET    /workflows`             — list workflows
//! - `GET    /workflows/{id}`        — get workflow status
//! - `POST   /workflows/{id}/pause`  — pause a running workflow
//! - `POST   /workflows/{id}/resume` — resume a paused workflow

use std::sync::Arc;

use aivyx_core::AivyxError;
use aivyx_task::{StageCondition, WorkflowStage};
use aivyx_tenant::AivyxRole;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::Deserialize;

use crate::app_state::AppState;
use crate::error::ServerError;
use crate::extractors::AuthContextExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Placeholder: the workflow engine is not yet wired into AppState.
///
/// All handlers return 503 Service Unavailable until `WorkflowStore` is added
/// to `AppState` and the workflow execution loop is integrated.
fn workflow_not_configured() -> ServerError {
    ServerError(AivyxError::NotInitialized(
        "workflow engine not configured".into(),
    ))
}

// ---------------------------------------------------------------------------
// POST /workflows — create a workflow
// ---------------------------------------------------------------------------

/// Request body for creating a new workflow.
#[derive(Debug, Deserialize)]
pub struct CreateWorkflowRequest {
    /// Human-readable workflow name.
    pub name: String,
    /// Ordered list of stages to execute.
    pub stages: Vec<CreateStageRequest>,
}

/// A single stage in the create-workflow request.
#[derive(Debug, Deserialize)]
pub struct CreateStageRequest {
    /// Human-readable stage name.
    pub name: String,
    /// Agent profile to use for this stage.
    pub agent: String,
    /// Prompt for the agent (may use `{prev_result}` placeholder).
    pub prompt: String,
    /// Condition for executing this stage (defaults to `Always`).
    #[serde(default = "default_condition")]
    pub condition: StageCondition,
}

fn default_condition() -> StageCondition {
    StageCondition::Always
}

impl From<CreateStageRequest> for WorkflowStage {
    fn from(req: CreateStageRequest) -> Self {
        WorkflowStage {
            name: req.name,
            agent: req.agent,
            prompt: req.prompt,
            condition: req.condition,
            result: None,
        }
    }
}

/// `POST /workflows` — create and start a workflow.
///
/// Requires `Operator` or higher role.
pub async fn create_workflow(
    State(_state): State<Arc<AppState>>,
    auth: AuthContextExt,
    axum::Json(_body): axum::Json<CreateWorkflowRequest>,
) -> Result<Response, ServerError> {
    auth.require_role(AivyxRole::Operator)?;
    Err(workflow_not_configured())
}

// ---------------------------------------------------------------------------
// GET /workflows — list workflows
// ---------------------------------------------------------------------------

/// `GET /workflows` — list all workflows.
///
/// Requires `Viewer` or higher role.
pub async fn list_workflows(
    State(_state): State<Arc<AppState>>,
    auth: AuthContextExt,
) -> Result<Response, ServerError> {
    auth.require_role(AivyxRole::Viewer)?;
    Err(workflow_not_configured())
}

// ---------------------------------------------------------------------------
// GET /workflows/{id} — get workflow details
// ---------------------------------------------------------------------------

/// `GET /workflows/{id}` — get a single workflow by ID.
///
/// Requires `Viewer` or higher role.
pub async fn get_workflow(
    State(_state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Path(_id): Path<String>,
) -> Result<Response, ServerError> {
    auth.require_role(AivyxRole::Viewer)?;
    Err(workflow_not_configured())
}

// ---------------------------------------------------------------------------
// POST /workflows/{id}/pause — pause a running workflow
// ---------------------------------------------------------------------------

/// `POST /workflows/{id}/pause` — pause a running workflow.
///
/// Requires `Operator` or higher role.
pub async fn pause_workflow(
    State(_state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Path(_id): Path<String>,
) -> Result<Response, ServerError> {
    auth.require_role(AivyxRole::Operator)?;
    Err(workflow_not_configured())
}

// ---------------------------------------------------------------------------
// POST /workflows/{id}/resume — resume a paused workflow
// ---------------------------------------------------------------------------

/// `POST /workflows/{id}/resume` — resume a paused workflow.
///
/// Requires `Operator` or higher role.
pub async fn resume_workflow(
    State(_state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Path(_id): Path<String>,
) -> Result<Response, ServerError> {
    auth.require_role(AivyxRole::Operator)?;
    Err(workflow_not_configured())
}

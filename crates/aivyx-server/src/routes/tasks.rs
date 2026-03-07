//! Task (mission) management endpoints.
//!
//! `GET /tasks` — list all missions with summary metadata.
//! `POST /tasks` — create and start a multi-step mission.
//! `GET /tasks/{id}` — get mission status and step details.
//! `DELETE /tasks/{id}` — delete a completed/failed/cancelled mission.
//! `POST /tasks/{id}/resume` — resume a paused or failed mission.
//! `POST /tasks/{id}/cancel` — cancel a running or planned mission.

use std::sync::Arc;

use aivyx_core::{AivyxError, TaskId};
use aivyx_crypto::derive_task_key;
use aivyx_task::{TaskEngine, TaskStore};
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use serde::Deserialize;

use crate::app_state::AppState;
use crate::error::ServerError;

/// Request body for `POST /tasks`.
#[derive(Deserialize)]
pub struct CreateTaskRequest {
    /// The high-level goal to accomplish.
    pub goal: String,
    /// Agent profile name to use for execution.
    pub agent: String,
}

/// `POST /tasks` — create a mission, plan it, and spawn background execution.
///
/// Returns immediately with the task ID and planned steps. The client
/// can poll `GET /tasks/{id}` for progress.
pub async fn create_task(
    State(state): State<Arc<AppState>>,
    axum::Json(req): axum::Json<CreateTaskRequest>,
) -> Result<impl IntoResponse, ServerError> {
    let task_key = derive_task_key(&state.master_key);
    let store = TaskStore::open(state.dirs.tasks_dir().join("tasks.db"))?;

    let engine = TaskEngine::new(
        state.agent_session.clone(),
        store,
        task_key,
        None, // audit log handled separately
    );

    // Plan the mission synchronously (quick LLM call)
    let task_id = engine.create_mission(&req.goal, &req.agent, None).await?;

    // Load the planned mission to return its steps
    let mission = engine
        .get_mission(&task_id)?
        .ok_or_else(|| AivyxError::Task("mission disappeared after creation".into()))?;

    // Spawn background execution
    let bg_session = state.agent_session.clone();
    let bg_master_key_bytes = {
        // We need to derive the task key again for the background task
        // since MasterKey is not Clone
        let tk = derive_task_key(&state.master_key);
        // Export the bytes so we can reconstruct
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(tk.expose_secret());
        bytes
    };
    let bg_dirs = aivyx_config::AivyxDirs::new(state.dirs.root());
    let bg_task_id = task_id;

    tokio::spawn(async move {
        let task_key = aivyx_crypto::MasterKey::from_bytes(bg_master_key_bytes);
        let store = match TaskStore::open(bg_dirs.tasks_dir().join("tasks.db")) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Failed to open task store for background execution: {e}");
                return;
            }
        };
        let engine = TaskEngine::new(bg_session, store, task_key, None);

        if let Err(e) = engine.execute_mission(&bg_task_id, None, None).await {
            tracing::error!("Background mission execution failed: {e}");
        }
    });

    Ok((
        axum::http::StatusCode::CREATED,
        axum::Json(serde_json::json!({
            "task_id": task_id.to_string(),
            "status": "planned",
            "goal": mission.goal,
            "agent": mission.agent_name,
            "steps": mission.steps.iter().map(|s| {
                serde_json::json!({
                    "index": s.index,
                    "description": s.description,
                    "tool_hints": s.tool_hints,
                })
            }).collect::<Vec<_>>(),
        })),
    ))
}

/// `GET /tasks/{id}` — get mission status and step details.
pub async fn get_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
    let task_id: TaskId = id
        .parse()
        .map_err(|_| ServerError(AivyxError::Config(format!("invalid task ID: {id}"))))?;

    let task_key = derive_task_key(&state.master_key);
    let store = TaskStore::open(state.dirs.tasks_dir().join("tasks.db"))?;
    let engine = TaskEngine::new(state.agent_session.clone(), store, task_key, None);

    let mission = engine
        .get_mission(&task_id)?
        .ok_or_else(|| ServerError(AivyxError::Config(format!("task not found: {id}"))))?;

    Ok(axum::Json(serde_json::json!({
        "task_id": mission.id.to_string(),
        "goal": mission.goal,
        "agent": mission.agent_name,
        "status": format!("{:?}", mission.status),
        "steps_completed": mission.steps_completed(),
        "steps_total": mission.steps.len(),
        "created_at": mission.created_at.to_rfc3339(),
        "updated_at": mission.updated_at.to_rfc3339(),
        "steps": mission.steps.iter().map(|s| {
            let mut step = serde_json::json!({
                "index": s.index,
                "description": s.description,
                "status": format!("{:?}", s.status),
            });
            if let Some(ref result) = s.result {
                let preview = if result.len() > 500 {
                    format!("{}...", &result[..result.floor_char_boundary(500)])
                } else {
                    result.clone()
                };
                step["result"] = serde_json::Value::String(preview);
            }
            step
        }).collect::<Vec<_>>(),
    })))
}

/// `DELETE /tasks/{id}` — delete a completed/failed/cancelled mission.
pub async fn delete_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
    let task_id: TaskId = id
        .parse()
        .map_err(|_| ServerError(AivyxError::Config(format!("invalid task ID: {id}"))))?;

    let task_key = derive_task_key(&state.master_key);
    let store = TaskStore::open(state.dirs.tasks_dir().join("tasks.db"))?;
    let engine = TaskEngine::new(state.agent_session.clone(), store, task_key, None);

    engine.delete_mission(&task_id)?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// `GET /tasks` — list all missions with summary metadata.
pub async fn list_tasks(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ServerError> {
    let task_key = derive_task_key(&state.master_key);
    let store = TaskStore::open(state.dirs.tasks_dir().join("tasks.db"))?;
    let engine = TaskEngine::new(state.agent_session.clone(), store, task_key, None);

    let missions = engine.list_missions()?;
    let results: Vec<serde_json::Value> = missions
        .into_iter()
        .map(|m| {
            serde_json::json!({
                "task_id": m.id.to_string(),
                "goal": m.goal,
                "agent": m.agent_name,
                "status": format!("{:?}", m.status),
                "steps_completed": m.steps_completed,
                "steps_total": m.steps_total,
                "created_at": m.created_at.to_rfc3339(),
                "updated_at": m.updated_at.to_rfc3339(),
            })
        })
        .collect();

    Ok(axum::Json(results))
}

/// `POST /tasks/{id}/resume` — resume a paused or failed mission.
///
/// Spawns background execution and returns immediately.
pub async fn resume_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
    let task_id: TaskId = id
        .parse()
        .map_err(|_| ServerError(AivyxError::Config(format!("invalid task ID: {id}"))))?;

    // Verify the mission exists before spawning background work
    let task_key = derive_task_key(&state.master_key);
    let store = TaskStore::open(state.dirs.tasks_dir().join("tasks.db"))?;
    let engine = TaskEngine::new(state.agent_session.clone(), store, task_key, None);
    engine
        .get_mission(&task_id)?
        .ok_or_else(|| ServerError(AivyxError::Config(format!("task not found: {id}"))))?;

    // Spawn background resume (same MasterKey cloning pattern as create_task)
    let bg_session = state.agent_session.clone();
    let bg_master_key_bytes = {
        let tk = derive_task_key(&state.master_key);
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(tk.expose_secret());
        bytes
    };
    let bg_dirs = aivyx_config::AivyxDirs::new(state.dirs.root());
    let bg_task_id = task_id;

    tokio::spawn(async move {
        let task_key = aivyx_crypto::MasterKey::from_bytes(bg_master_key_bytes);
        let store = match TaskStore::open(bg_dirs.tasks_dir().join("tasks.db")) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Failed to open task store for resume: {e}");
                return;
            }
        };
        let engine = TaskEngine::new(bg_session, store, task_key, None);

        if let Err(e) = engine.resume(&bg_task_id, None, None).await {
            tracing::error!("Background mission resume failed: {e}");
        }
    });

    Ok(axum::Json(serde_json::json!({
        "task_id": task_id.to_string(),
        "status": "resuming",
    })))
}

/// `POST /tasks/{id}/cancel` — cancel a running or planned mission.
pub async fn cancel_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
    let task_id: TaskId = id
        .parse()
        .map_err(|_| ServerError(AivyxError::Config(format!("invalid task ID: {id}"))))?;

    let task_key = derive_task_key(&state.master_key);
    let store = TaskStore::open(state.dirs.tasks_dir().join("tasks.db"))?;
    let engine = TaskEngine::new(state.agent_session.clone(), store, task_key, None);

    engine.cancel(&task_id)?;

    Ok(axum::Json(serde_json::json!({
        "task_id": task_id.to_string(),
        "status": "cancelled",
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_task_request_deserializes_all_fields() {
        let json = r#"{"goal":"Refactor the auth module","agent":"coder"}"#;
        let req: CreateTaskRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.goal, "Refactor the auth module");
        assert_eq!(req.agent, "coder");
    }

    #[test]
    fn create_task_request_minimal() {
        let json = r#"{"goal":"do stuff","agent":"assistant"}"#;
        let req: CreateTaskRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.goal, "do stuff");
        assert_eq!(req.agent, "assistant");
    }

    #[test]
    fn create_task_request_missing_goal_fails() {
        let json = r#"{"agent":"coder"}"#;
        let result = serde_json::from_str::<CreateTaskRequest>(json);
        assert!(result.is_err());
    }

    #[test]
    fn create_task_request_missing_agent_fails() {
        let json = r#"{"goal":"do stuff"}"#;
        let result = serde_json::from_str::<CreateTaskRequest>(json);
        assert!(result.is_err());
    }

    #[test]
    fn create_task_request_empty_strings() {
        let json = r#"{"goal":"","agent":""}"#;
        let req: CreateTaskRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.goal, "");
        assert_eq!(req.agent, "");
    }

    #[test]
    fn task_id_parse_valid() {
        let uuid_str = "550e8400-e29b-41d4-a716-446655440000";
        let result: Result<TaskId, _> = uuid_str.parse();
        assert!(result.is_ok());
    }

    #[test]
    fn task_id_parse_invalid() {
        let result: Result<TaskId, _> = "not-a-uuid".parse();
        assert!(result.is_err());
    }

    #[test]
    fn task_id_roundtrip() {
        let id = TaskId::new();
        let s = id.to_string();
        let parsed: TaskId = s.parse().unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn invalid_task_id_error_message() {
        let bad_id = "xyz";
        let err = ServerError(AivyxError::Config(format!("invalid task ID: {bad_id}")));
        let resp = err.into_response();
        // "invalid task ID" does not contain "not found", so it should be 500
        assert_eq!(resp.status(), axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn task_not_found_maps_to_404() {
        let err = ServerError(AivyxError::Config("task not found: abc".into()));
        let resp = err.into_response();
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
    }
}

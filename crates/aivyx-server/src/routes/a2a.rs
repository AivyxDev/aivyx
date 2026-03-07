//! Google A2A (Agent-to-Agent) protocol endpoints.
//!
//! `GET /.well-known/agent.json` — public Agent Card for discovery.
//! `POST /a2a` — JSON-RPC 2.0 dispatcher for task operations.
//!
//! The Agent Card endpoint is **unauthenticated** per the A2A spec, allowing
//! external agents to discover this instance's capabilities. All task
//! operations require Bearer token authentication.
//!
//! Supported JSON-RPC methods:
//! - `tasks/send` — create a new task from an A2A message
//! - `tasks/get` — retrieve task status and artifacts
//! - `tasks/cancel` — cancel a running task

use std::convert::Infallible;
use std::sync::Arc;

use aivyx_agent::AgentProfile;
use aivyx_core::a2a::{
    A2aArtifact, A2aMessage, A2aPart, A2aRole, A2aTask, A2aTaskState, A2aTaskStatus,
    AgentAuthentication, AgentCapabilities, AgentCard, AgentSkill, JsonRpcRequest, JsonRpcResponse,
    PushNotificationConfig, TaskStatusUpdateEvent,
};
use aivyx_core::{AivyxError, TaskId};
use aivyx_crypto::derive_task_key;
use aivyx_task::{Mission, StepKind, StepStatus, TaskEngine, TaskStatus, TaskStore};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::response::sse::{Event, Sse};
use futures_util::stream::Stream;

use crate::app_state::AppState;

// ---------------------------------------------------------------------------
// Agent Card (public, no auth)
// ---------------------------------------------------------------------------

/// `GET /.well-known/agent.json` — A2A Agent Card.
///
/// Returns a discovery document describing this instance's capabilities,
/// available skills (derived from agent profiles), and authentication
/// requirements. This endpoint is unauthenticated per the A2A spec.
pub async fn agent_card(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let agents_dir = state.dirs.agents_dir();
    let mut skills = Vec::new();

    if agents_dir.exists()
        && let Ok(entries) = std::fs::read_dir(&agents_dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "toml")
                && let Ok(profile) = AgentProfile::load(&path)
            {
                skills.push(AgentSkill {
                    id: profile.name.clone(),
                    name: profile.name.clone(),
                    description: profile.role.clone(),
                });
            }
        }
    }

    let config = state.config.read().await;
    let base_url = config
        .server
        .as_ref()
        .and_then(|s| s.public_url.as_deref())
        .unwrap_or(
            // No public_url configured — construct from bind address + port
            // (this is a local fallback; in production, public_url should be set)
            "http://localhost:3000",
        );

    let card = AgentCard {
        name: "Aivyx Engine".to_string(),
        description: "AI agent orchestration engine with multi-step task execution".to_string(),
        url: base_url.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        capabilities: AgentCapabilities {
            streaming: true,
            push_notifications: true,
        },
        skills,
        default_input_modes: vec!["text/plain".to_string()],
        default_output_modes: vec!["text/plain".to_string()],
        authentication: Some(AgentAuthentication {
            schemes: vec!["bearer".to_string()],
        }),
    };

    axum::Json(card)
}

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 dispatcher
// ---------------------------------------------------------------------------

/// `POST /a2a` — JSON-RPC 2.0 dispatcher for A2A task operations.
///
/// Dispatches to the appropriate handler based on the `method` field:
/// - `tasks/send` — create and execute a task
/// - `tasks/get` — retrieve task status
/// - `tasks/cancel` — cancel a running task
pub async fn a2a_handler(
    State(state): State<Arc<AppState>>,
    axum::Json(req): axum::Json<JsonRpcRequest>,
) -> impl IntoResponse {
    let result = match req.method.as_str() {
        "tasks/send" => handle_task_send(&state, &req).await,
        "tasks/get" => handle_task_get(&state, &req).await,
        "tasks/cancel" => handle_task_cancel(&state, &req).await,
        "tasks/pushNotification/set" => handle_push_notification_set(&state, &req).await,
        "tasks/pushNotification/get" => handle_push_notification_get(&state, &req).await,
        "tasks/pushNotification/delete" => handle_push_notification_delete(&state, &req).await,
        _ => Ok(JsonRpcResponse::error(
            req.id.clone(),
            -32601,
            format!("method not found: {}", req.method),
        )),
    };

    match result {
        Ok(resp) => axum::Json(resp),
        Err(e) => axum::Json(JsonRpcResponse::error(
            req.id.clone(),
            -32000,
            e.to_string(),
        )),
    }
}

// ---------------------------------------------------------------------------
// tasks/send — create a new task from an A2A message
// ---------------------------------------------------------------------------

/// Handle `tasks/send` — extract text from the A2A message, create and
/// launch a background mission, and return an A2A task with `submitted` state.
async fn handle_task_send(
    state: &AppState,
    req: &JsonRpcRequest,
) -> Result<JsonRpcResponse, AivyxError> {
    // Extract the message text from params
    let message: A2aMessage = serde_json::from_value(
        req.params
            .get("message")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    )
    .map_err(|e| AivyxError::Other(format!("invalid message in params: {e}")))?;

    let goal = extract_text_from_parts(&message.parts);
    if goal.is_empty() {
        return Ok(JsonRpcResponse::error(
            req.id.clone(),
            -32602,
            "message must contain at least one text part",
        ));
    }

    // Use the agent specified in params, or fall back to a default
    let agent_name = req
        .params
        .get("agent")
        .and_then(|v| v.as_str())
        .unwrap_or("assistant")
        .to_string();

    // Create the mission via TaskEngine
    let task_key = derive_task_key(&state.master_key);
    let store = TaskStore::open(state.dirs.tasks_dir().join("tasks.db"))?;
    let engine = TaskEngine::new(state.agent_session.clone(), store, task_key, None);

    let task_id = engine.create_mission(&goal, &agent_name, None).await?;

    // Spawn background execution (same pattern as routes/tasks.rs)
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
                tracing::error!("A2A background execution: failed to open task store: {e}");
                return;
            }
        };
        let engine = TaskEngine::new(bg_session, store, task_key, None);
        if let Err(e) = engine.execute_mission(&bg_task_id, None, None).await {
            tracing::error!(task_id = %bg_task_id, "A2A background execution failed: {e}");
        }
    });

    // Build the A2A response
    let a2a_task = A2aTask {
        id: task_id.to_string(),
        status: A2aTaskStatus {
            state: A2aTaskState::Submitted,
            message: Some(A2aMessage {
                role: A2aRole::Agent,
                parts: vec![A2aPart::Text {
                    text: format!(
                        "Task created and queued for execution with agent '{agent_name}'"
                    ),
                }],
            }),
            timestamp: chrono::Utc::now().to_rfc3339(),
        },
        history: Some(vec![message]),
        artifacts: None,
        metadata: None,
    };

    let result = serde_json::to_value(&a2a_task)
        .map_err(|e| AivyxError::Other(format!("serialize A2A task: {e}")))?;
    Ok(JsonRpcResponse::success(req.id.clone(), result))
}

// ---------------------------------------------------------------------------
// tasks/get — retrieve task status and artifacts
// ---------------------------------------------------------------------------

/// Handle `tasks/get` — load the mission and convert to an A2A task
/// representation with status, history, and artifacts.
async fn handle_task_get(
    state: &AppState,
    req: &JsonRpcRequest,
) -> Result<JsonRpcResponse, AivyxError> {
    let task_id_str = req
        .params
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AivyxError::Other("missing 'id' in params".into()))?;

    let task_id: TaskId = task_id_str
        .parse()
        .map_err(|_| AivyxError::Other(format!("invalid task ID: {task_id_str}")))?;

    let task_key = derive_task_key(&state.master_key);
    let store = TaskStore::open(state.dirs.tasks_dir().join("tasks.db"))?;
    let engine = TaskEngine::new(state.agent_session.clone(), store, task_key, None);

    let mission = engine
        .get_mission(&task_id)?
        .ok_or_else(|| AivyxError::Config(format!("task not found: {task_id_str}")))?;

    let a2a_task = mission_to_a2a_task(&mission);

    let result = serde_json::to_value(&a2a_task)
        .map_err(|e| AivyxError::Other(format!("serialize A2A task: {e}")))?;
    Ok(JsonRpcResponse::success(req.id.clone(), result))
}

// ---------------------------------------------------------------------------
// tasks/cancel — cancel a running task
// ---------------------------------------------------------------------------

/// Handle `tasks/cancel` — cancel the mission and return updated status.
async fn handle_task_cancel(
    state: &AppState,
    req: &JsonRpcRequest,
) -> Result<JsonRpcResponse, AivyxError> {
    let task_id_str = req
        .params
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AivyxError::Other("missing 'id' in params".into()))?;

    let task_id: TaskId = task_id_str
        .parse()
        .map_err(|_| AivyxError::Other(format!("invalid task ID: {task_id_str}")))?;

    let task_key = derive_task_key(&state.master_key);
    let store = TaskStore::open(state.dirs.tasks_dir().join("tasks.db"))?;
    let engine = TaskEngine::new(state.agent_session.clone(), store, task_key, None);

    engine.cancel(&task_id)?;

    let a2a_task = A2aTask {
        id: task_id.to_string(),
        status: A2aTaskStatus {
            state: A2aTaskState::Canceled,
            message: Some(A2aMessage {
                role: A2aRole::Agent,
                parts: vec![A2aPart::Text {
                    text: "Task cancelled".to_string(),
                }],
            }),
            timestamp: chrono::Utc::now().to_rfc3339(),
        },
        history: None,
        artifacts: None,
        metadata: None,
    };

    let result = serde_json::to_value(&a2a_task)
        .map_err(|e| AivyxError::Other(format!("serialize A2A task: {e}")))?;
    Ok(JsonRpcResponse::success(req.id.clone(), result))
}

// ---------------------------------------------------------------------------
// tasks/sendSubscribe — SSE streaming variant of tasks/send
// ---------------------------------------------------------------------------

/// `POST /a2a/stream` — SSE streaming variant of `tasks/send`.
///
/// Accepts the same JSON-RPC body as `tasks/send` but returns an SSE stream
/// of `TaskStatusUpdateEvent` messages. Currently a stub that emits a
/// "submitted" event followed by a "completed" event (the full mission
/// execution loop is not yet wired into the stream).
pub async fn a2a_stream_handler(
    State(state): State<Arc<AppState>>,
    axum::Json(req): axum::Json<JsonRpcRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, axum::Json<JsonRpcResponse>> {
    if req.method.as_str() != "tasks/sendSubscribe" {
        return Err(axum::Json(JsonRpcResponse::error(
            req.id.clone(),
            -32601,
            format!(
                "streaming endpoint only supports tasks/sendSubscribe, got: {}",
                req.method
            ),
        )));
    }

    // Extract the message text from params (same as tasks/send)
    let message: A2aMessage = serde_json::from_value(
        req.params
            .get("message")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    )
    .map_err(|e| {
        axum::Json(JsonRpcResponse::error(
            req.id.clone(),
            -32602,
            format!("invalid message in params: {e}"),
        ))
    })?;

    let goal = extract_text_from_parts(&message.parts);
    if goal.is_empty() {
        return Err(axum::Json(JsonRpcResponse::error(
            req.id.clone(),
            -32602,
            "message must contain at least one text part",
        )));
    }

    let agent_name = req
        .params
        .get("agent")
        .and_then(|v| v.as_str())
        .unwrap_or("assistant")
        .to_string();

    // Create the mission via TaskEngine
    let task_key = derive_task_key(&state.master_key);
    let store = TaskStore::open(state.dirs.tasks_dir().join("tasks.db")).map_err(|e| {
        axum::Json(JsonRpcResponse::error(
            req.id.clone(),
            -32000,
            e.to_string(),
        ))
    })?;
    let engine = TaskEngine::new(state.agent_session.clone(), store, task_key, None);

    let task_id = engine
        .create_mission(&goal, &agent_name, None)
        .await
        .map_err(|e| {
            axum::Json(JsonRpcResponse::error(
                req.id.clone(),
                -32000,
                e.to_string(),
            ))
        })?;

    let task_id_str = task_id.to_string();

    // Build the SSE stream with stub events
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(8);

    // Spawn background task to emit events
    let bg_state = state.clone();
    let bg_task_id = task_id;
    let bg_task_id_str = task_id_str.clone();
    tokio::spawn(async move {
        // Emit "submitted" event
        let submitted = TaskStatusUpdateEvent {
            id: bg_task_id_str.clone(),
            status: A2aTaskStatus {
                state: A2aTaskState::Submitted,
                message: Some(A2aMessage {
                    role: A2aRole::Agent,
                    parts: vec![A2aPart::Text {
                        text: format!(
                            "Task created and queued for execution with agent '{agent_name}'"
                        ),
                    }],
                }),
                timestamp: chrono::Utc::now().to_rfc3339(),
            },
            is_final: false,
        };
        if let Ok(data) = serde_json::to_string(&submitted) {
            let _ = tx.send(Ok(Event::default().data(data))).await;
        }

        // Spawn background execution (same pattern as tasks/send)
        let bg_master_key_bytes = {
            let tk = derive_task_key(&bg_state.master_key);
            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(tk.expose_secret());
            bytes
        };
        let bg_dirs = aivyx_config::AivyxDirs::new(bg_state.dirs.root());
        let task_key = aivyx_crypto::MasterKey::from_bytes(bg_master_key_bytes);
        let store = match TaskStore::open(bg_dirs.tasks_dir().join("tasks.db")) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("A2A stream: failed to open task store: {e}");
                return;
            }
        };
        let engine = TaskEngine::new(bg_state.agent_session.clone(), store, task_key, None);
        let exec_result = engine.execute_mission(&bg_task_id, None, None).await;

        // Emit "completed" or "failed" event
        let final_event = match exec_result {
            Ok(_) => TaskStatusUpdateEvent {
                id: bg_task_id_str.clone(),
                status: A2aTaskStatus {
                    state: A2aTaskState::Completed,
                    message: Some(A2aMessage {
                        role: A2aRole::Agent,
                        parts: vec![A2aPart::Text {
                            text: "Task completed successfully".to_string(),
                        }],
                    }),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                },
                is_final: true,
            },
            Err(e) => TaskStatusUpdateEvent {
                id: bg_task_id_str.clone(),
                status: A2aTaskStatus {
                    state: A2aTaskState::Failed,
                    message: Some(A2aMessage {
                        role: A2aRole::Agent,
                        parts: vec![A2aPart::Text {
                            text: format!("Task failed: {e}"),
                        }],
                    }),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                },
                is_final: true,
            },
        };

        if let Ok(data) = serde_json::to_string(&final_event) {
            let _ = tx.send(Ok(Event::default().data(data))).await;
        }
    });

    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    Ok(Sse::new(stream))
}

// ---------------------------------------------------------------------------
// tasks/pushNotification/set — store push notification config for a task
// ---------------------------------------------------------------------------

/// Handle `tasks/pushNotification/set` — store a push notification config
/// for the given task ID.
async fn handle_push_notification_set(
    state: &AppState,
    req: &JsonRpcRequest,
) -> Result<JsonRpcResponse, AivyxError> {
    let task_id = req
        .params
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AivyxError::Other("missing 'id' in params".into()))?
        .to_string();

    let config: PushNotificationConfig = serde_json::from_value(
        req.params
            .get("pushNotificationConfig")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    )
    .map_err(|e| AivyxError::Other(format!("invalid pushNotificationConfig: {e}")))?;

    state
        .push_notification_configs
        .write()
        .await
        .insert(task_id.clone(), config.clone());

    let result = serde_json::to_value(&config)
        .map_err(|e| AivyxError::Other(format!("serialize config: {e}")))?;
    Ok(JsonRpcResponse::success(req.id.clone(), result))
}

// ---------------------------------------------------------------------------
// tasks/pushNotification/get — retrieve push notification config for a task
// ---------------------------------------------------------------------------

/// Handle `tasks/pushNotification/get` — retrieve the push notification
/// config for the given task ID.
async fn handle_push_notification_get(
    state: &AppState,
    req: &JsonRpcRequest,
) -> Result<JsonRpcResponse, AivyxError> {
    let task_id = req
        .params
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AivyxError::Other("missing 'id' in params".into()))?;

    let configs = state.push_notification_configs.read().await;
    match configs.get(task_id) {
        Some(config) => {
            let result = serde_json::to_value(config)
                .map_err(|e| AivyxError::Other(format!("serialize config: {e}")))?;
            Ok(JsonRpcResponse::success(req.id.clone(), result))
        }
        None => Ok(JsonRpcResponse::error(
            req.id.clone(),
            -32602,
            format!("no push notification config for task: {task_id}"),
        )),
    }
}

// ---------------------------------------------------------------------------
// tasks/pushNotification/delete — remove push notification config for a task
// ---------------------------------------------------------------------------

/// Handle `tasks/pushNotification/delete` — remove the push notification
/// config for the given task ID.
async fn handle_push_notification_delete(
    state: &AppState,
    req: &JsonRpcRequest,
) -> Result<JsonRpcResponse, AivyxError> {
    let task_id = req
        .params
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AivyxError::Other("missing 'id' in params".into()))?;

    let mut configs = state.push_notification_configs.write().await;
    if configs.remove(task_id).is_some() {
        Ok(JsonRpcResponse::success(
            req.id.clone(),
            serde_json::json!({"deleted": true}),
        ))
    } else {
        Ok(JsonRpcResponse::error(
            req.id.clone(),
            -32602,
            format!("no push notification config for task: {task_id}"),
        ))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract concatenated text from A2A message parts.
fn extract_text_from_parts(parts: &[A2aPart]) -> String {
    parts
        .iter()
        .filter_map(|p| match p {
            A2aPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Map Aivyx `TaskStatus` to A2A `A2aTaskState`, with special handling for
/// the `InputRequired` state when an `Approval` step is pending.
fn map_task_state(mission: &Mission) -> A2aTaskState {
    match &mission.status {
        TaskStatus::Planning | TaskStatus::Planned => A2aTaskState::Submitted,
        TaskStatus::Executing | TaskStatus::Verifying => {
            // Check if there's a pending Approval step → InputRequired
            let has_pending_approval = mission.steps.iter().any(|s| {
                matches!(s.status, StepStatus::Pending | StepStatus::Running)
                    && matches!(s.kind, StepKind::Approval { .. })
            });
            if has_pending_approval {
                A2aTaskState::InputRequired
            } else {
                A2aTaskState::Working
            }
        }
        TaskStatus::Completed => A2aTaskState::Completed,
        TaskStatus::Failed { .. } => A2aTaskState::Failed,
        TaskStatus::Cancelled => A2aTaskState::Canceled,
    }
}

/// Convert an internal `Mission` to an A2A `A2aTask`.
fn mission_to_a2a_task(mission: &Mission) -> A2aTask {
    let state = map_task_state(mission);

    // Build a status message from the current state
    let status_text = match &mission.status {
        TaskStatus::Planning => "Planning task steps".to_string(),
        TaskStatus::Planned => format!("Planned {} steps", mission.steps.len()),
        TaskStatus::Executing => format!(
            "Executing step {}/{}",
            mission.steps_completed() + 1,
            mission.steps.len()
        ),
        TaskStatus::Verifying => "Verifying results".to_string(),
        TaskStatus::Completed => "Task completed successfully".to_string(),
        TaskStatus::Failed { reason } => format!("Task failed: {reason}"),
        TaskStatus::Cancelled => "Task cancelled".to_string(),
    };

    // Build artifacts from completed step results
    let artifacts: Vec<A2aArtifact> = mission
        .steps
        .iter()
        .filter(|s| matches!(s.status, StepStatus::Completed))
        .filter_map(|s| {
            s.result.as_ref().map(|result| A2aArtifact {
                name: Some(format!("step-{}: {}", s.index, s.description)),
                parts: vec![A2aPart::Text {
                    text: result.clone(),
                }],
            })
        })
        .collect();

    // Build history: the original goal as user message, plus agent responses
    let mut history = vec![A2aMessage {
        role: A2aRole::User,
        parts: vec![A2aPart::Text {
            text: mission.goal.clone(),
        }],
    }];

    // Add a summary agent message if there are completed steps
    let completed_summaries: Vec<String> = mission
        .steps
        .iter()
        .filter(|s| matches!(s.status, StepStatus::Completed))
        .filter_map(|s| {
            s.result
                .as_ref()
                .map(|r| format!("Step {}: {}", s.index + 1, truncate(r, 200)))
        })
        .collect();

    if !completed_summaries.is_empty() {
        history.push(A2aMessage {
            role: A2aRole::Agent,
            parts: vec![A2aPart::Text {
                text: completed_summaries.join("\n\n"),
            }],
        });
    }

    A2aTask {
        id: mission.id.to_string(),
        status: A2aTaskStatus {
            state,
            message: Some(A2aMessage {
                role: A2aRole::Agent,
                parts: vec![A2aPart::Text { text: status_text }],
            }),
            timestamp: mission.updated_at.to_rfc3339(),
        },
        history: Some(history),
        artifacts: if artifacts.is_empty() {
            None
        } else {
            Some(artifacts)
        },
        metadata: None,
    }
}

/// Truncate a string to the given byte length, appending "..." if truncated.
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let boundary = s.floor_char_boundary(max_len);
        format!("{}...", &s[..boundary])
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_text_joins_parts() {
        let parts = vec![
            A2aPart::Text {
                text: "Hello".into(),
            },
            A2aPart::Data {
                data: serde_json::json!({"key": "value"}),
            },
            A2aPart::Text {
                text: "world".into(),
            },
        ];
        assert_eq!(extract_text_from_parts(&parts), "Hello\nworld");
    }

    #[test]
    fn extract_text_empty_parts() {
        let parts: Vec<A2aPart> = vec![];
        assert_eq!(extract_text_from_parts(&parts), "");
    }

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string() {
        let long = "a".repeat(300);
        let result = truncate(&long, 200);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 204); // 200 + "..."
    }

    #[test]
    fn json_rpc_error_for_unknown_method() {
        let resp = JsonRpcResponse::error(serde_json::json!(1), -32601, "method not found: foo");
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["error"]["code"], -32601);
        assert_eq!(json["jsonrpc"], "2.0");
    }

    #[test]
    fn json_rpc_request_deserializes() {
        let json = r#"{
            "jsonrpc": "2.0",
            "method": "tasks/send",
            "params": {
                "message": {
                    "role": "user",
                    "parts": [{"type": "text", "text": "Research AI trends"}]
                },
                "agent": "researcher"
            },
            "id": 1
        }"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "tasks/send");
        assert_eq!(req.params["agent"], "researcher");
    }

    #[test]
    fn a2a_task_serializes_correctly() {
        let task = A2aTask {
            id: "test-id".into(),
            status: A2aTaskStatus {
                state: A2aTaskState::Submitted,
                message: None,
                timestamp: "2026-03-07T00:00:00Z".into(),
            },
            history: None,
            artifacts: None,
            metadata: None,
        };
        let json = serde_json::to_value(&task).unwrap();
        assert_eq!(json["id"], "test-id");
        assert_eq!(json["status"]["state"], "submitted");
        // history/artifacts should be absent (skip_serializing_if)
        assert!(json.get("history").is_none() || json["history"].is_null());
    }
}

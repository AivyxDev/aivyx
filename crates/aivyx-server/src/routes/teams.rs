//! Team management and execution endpoints.
//!
//! `GET /teams` — list all configured teams.
//! `GET /teams/:name` — get a single team configuration.
//! `POST /teams/:name/run` — run a team task (non-streaming).
//! `POST /teams/:name/run/stream` — run a team task (SSE streaming).

use std::convert::Infallible;
use std::sync::Arc;

use aivyx_agent::AgentSession;
use aivyx_core::AivyxError;
use aivyx_team::TeamRuntime;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum::response::sse::{Event, Sse};
use futures_util::stream::Stream;
use serde::{Deserialize, Serialize};
use tokio_stream::wrappers::ReceiverStream;

use crate::app_state::AppState;
use crate::error::ServerError;
use crate::validation::validate_name;

/// Summary of a team for listing.
#[derive(Debug, Serialize)]
pub struct TeamSummary {
    /// Team name.
    pub name: String,
    /// Team description.
    pub description: String,
}

/// Request body for `POST /teams/:name/run`.
#[derive(Debug, Deserialize)]
pub struct TeamRunRequest {
    /// Prompt to send to the team.
    pub prompt: String,
    /// Optional session ID to resume from.
    #[serde(default)]
    pub session_id: Option<String>,
}

/// Response body for `POST /teams/:name/run`.
#[derive(Debug, Serialize)]
pub struct TeamRunResponse {
    /// The team's response.
    pub response: String,
    /// Session ID for resuming this team run.
    pub session_id: String,
}

/// Summary of a saved team session.
#[derive(Debug, Serialize)]
pub struct TeamSessionSummary {
    /// Session identifier.
    pub session_id: String,
    /// Number of lead conversation messages.
    pub lead_message_count: usize,
    /// Number of specialists with saved conversations.
    pub specialist_count: usize,
    /// Number of completed work entries.
    pub completed_work_count: usize,
    /// When the session was last saved (RFC 3339).
    pub updated_at: String,
}

/// `GET /teams` — list all team configurations.
pub async fn list_teams(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ServerError> {
    let teams_dir = state.dirs.teams_dir();
    let mut teams = Vec::new();

    if teams_dir.exists() {
        for entry in std::fs::read_dir(&teams_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "toml")
                && let Ok(config) = aivyx_team::TeamConfig::load(&path)
            {
                teams.push(TeamSummary {
                    name: config.name.clone(),
                    description: config.description.clone(),
                });
            }
        }
    }

    Ok(axum::Json(teams))
}

/// `GET /teams/:name` — get a single team configuration.
pub async fn get_team(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
    validate_name(&name)?;
    let path = state.dirs.teams_dir().join(format!("{name}.toml"));
    if !path.exists() {
        return Err(ServerError(AivyxError::Config(format!(
            "team not found: {name}"
        ))));
    }
    let config = aivyx_team::TeamConfig::load(&path)?;
    Ok(axum::Json(config))
}

/// `POST /teams/:name/run` — run a team task (non-streaming).
pub async fn run_team(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    axum::Json(req): axum::Json<TeamRunRequest>,
) -> Result<impl IntoResponse, ServerError> {
    validate_name(&name)?;
    let runtime = load_runtime(&state, &name)?;
    let response = runtime.run(&req.prompt, None).await?;

    // Generate or reuse session ID, save session
    let session_id = req
        .session_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let ts_key = aivyx_crypto::derive_team_session_key(&state.master_key);
    let ts_dir = state.dirs.team_sessions_dir();
    std::fs::create_dir_all(&ts_dir)?;
    let store = aivyx_team::TeamSessionStore::open(ts_dir.join("team-sessions.db"))?;

    let persisted = aivyx_team::PersistedTeamSession {
        session_id: session_id.clone(),
        team_name: name.clone(),
        lead_conversation: Vec::new(),
        specialist_conversations: std::collections::HashMap::new(),
        completed_work: vec![format!(
            "Goal: {}\nResult: {} chars output",
            req.prompt,
            response.len()
        )],
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
    };
    store.save(&persisted, &ts_key)?;

    Ok(axum::Json(TeamRunResponse {
        response,
        session_id,
    }))
}

/// `POST /teams/:name/run/stream` — run a team task (SSE streaming).
pub async fn stream_run_team(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    axum::Json(req): axum::Json<TeamRunRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ServerError> {
    validate_name(&name)?;
    let runtime = load_runtime(&state, &name)?;
    let (token_tx, token_rx) = tokio::sync::mpsc::channel::<String>(64);
    let (sse_tx, sse_rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);

    let prompt = req.prompt.clone();

    tokio::spawn(async move {
        // Forward tokens to SSE events
        let sse_tx_tokens = sse_tx.clone();
        let forwarder = tokio::spawn(async move {
            let mut rx = token_rx;
            while let Some(token) = rx.recv().await {
                let data = serde_json::json!({"type": "token", "content": token});
                let event = Event::default().data(data.to_string());
                if sse_tx_tokens.send(Ok(event)).await.is_err() {
                    break;
                }
            }
        });

        match runtime.run_stream(&prompt, None, token_tx).await {
            Ok(response) => {
                let _ = forwarder.await;
                let done = serde_json::json!({"type": "done", "response": response});
                // Client may have disconnected; safe to discard
                let _ = sse_tx
                    .send(Ok(Event::default().data(done.to_string())))
                    .await;
                // Client may have disconnected; safe to discard
                let _ = sse_tx.send(Ok(Event::default().data("[DONE]"))).await;
            }
            Err(e) => {
                let _ = forwarder.await;
                let err = serde_json::json!({"type": "error", "message": e.to_string()});
                // Client may have disconnected; safe to discard
                let _ = sse_tx
                    .send(Ok(Event::default().data(err.to_string())))
                    .await;
                // Client may have disconnected; safe to discard
                let _ = sse_tx.send(Ok(Event::default().data("[DONE]"))).await;
            }
        }
    });

    let stream = ReceiverStream::new(sse_rx);
    Ok(Sse::new(stream))
}

/// `GET /teams/:name/sessions` — list saved sessions for a team.
pub async fn list_team_sessions(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
    validate_name(&name)?;
    let ts_key = aivyx_crypto::derive_team_session_key(&state.master_key);
    let ts_dir = state.dirs.team_sessions_dir();

    if !ts_dir.exists() {
        return Ok(axum::Json(Vec::<TeamSessionSummary>::new()));
    }

    let store = aivyx_team::TeamSessionStore::open(ts_dir.join("team-sessions.db"))?;
    let sessions = store.list(&name, &ts_key)?;

    let summaries: Vec<TeamSessionSummary> = sessions
        .into_iter()
        .map(|meta| TeamSessionSummary {
            session_id: meta.session_id,
            lead_message_count: meta.lead_message_count,
            specialist_count: meta.specialist_count,
            completed_work_count: meta.completed_work_count,
            updated_at: meta.updated_at,
        })
        .collect();

    Ok(axum::Json(summaries))
}

/// Load a `TeamRuntime` for the given team name.
fn load_runtime(state: &AppState, name: &str) -> Result<TeamRuntime, ServerError> {
    // TeamRuntime::load consumes an AgentSession, so we create a fresh one
    let dirs = aivyx_config::AivyxDirs::new(state.dirs.root());
    // We need a fresh MasterKey for AgentSession — re-derive from the same bytes
    // Since MasterKey is not Clone, we derive a session key from the store key.
    // In practice, the AgentSession already exists in state — but TeamRuntime::load
    // consumes AgentSession by value. We can't clone AgentSession either.
    // Workaround: use TeamRuntime::new() with a loaded TeamConfig and a fresh session.
    let runtime = TeamRuntime::load(name, &dirs, create_agent_session(state)?)?;
    Ok(runtime)
}

/// Create a fresh `AgentSession` for team runtime.
///
/// This is needed because `TeamRuntime::load` consumes the session by value.
fn create_agent_session(state: &AppState) -> Result<AgentSession, ServerError> {
    // We need a MasterKey, but it's not Clone.
    // Workaround: derive a deterministic key from master key bytes.
    // This is safe because we're creating an equivalent key.
    let key_bytes: [u8; 32] = state.master_key.expose_secret()[..32]
        .try_into()
        .map_err(|_| ServerError(AivyxError::Crypto("invalid master key length".into())))?;
    let mk = aivyx_crypto::MasterKey::from_bytes(key_bytes);
    let dirs = aivyx_config::AivyxDirs::new(state.dirs.root());
    Ok(AgentSession::new(dirs, state.config.clone(), mk))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_summary_serializes() {
        let s = TeamSummary {
            name: "ops".into(),
            description: "operations team".into(),
        };
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["name"], "ops");
    }

    #[test]
    fn team_run_request_deserializes() {
        let json = r#"{"prompt":"analyze this"}"#;
        let req: TeamRunRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.prompt, "analyze this");
        assert!(req.session_id.is_none());
    }

    #[test]
    fn team_run_request_with_session_id() {
        let json = r#"{"prompt":"continue","session_id":"abc-123"}"#;
        let req: TeamRunRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.prompt, "continue");
        assert_eq!(req.session_id.as_deref(), Some("abc-123"));
    }

    #[test]
    fn team_run_response_serializes() {
        let resp = TeamRunResponse {
            response: "analysis complete".into(),
            session_id: "sess-001".into(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["response"], "analysis complete");
        assert_eq!(json["session_id"], "sess-001");
    }

    #[test]
    fn team_session_summary_serializes() {
        let s = TeamSessionSummary {
            session_id: "s-1".into(),
            lead_message_count: 10,
            specialist_count: 2,
            completed_work_count: 3,
            updated_at: "2026-03-05T12:00:00Z".into(),
        };
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["session_id"], "s-1");
        assert_eq!(json["lead_message_count"], 10);
        assert_eq!(json["specialist_count"], 2);
    }
}

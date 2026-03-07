//! Chat endpoints for agent conversation.
//!
//! `POST /chat` — synchronous single-turn chat with an agent.
//! `POST /chat/stream` — SSE streaming variant with OpenAI-compatible format.
//! `POST /chat/audio` — multipart audio upload, transcription, and chat.

use std::convert::Infallible;
use std::sync::Arc;

use aivyx_audit::AuditEvent;
use aivyx_core::{AivyxError, SessionId};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::response::sse::{Event, Sse};
use futures_util::stream::Stream;
use serde::{Deserialize, Serialize};
use tokio_stream::wrappers::ReceiverStream;

use crate::app_state::AppState;
use crate::error::ServerError;

/// Request body for `POST /chat`.
#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    /// Agent profile name (must exist in `~/.aivyx/agents/`).
    pub agent: String,
    /// The user's message.
    pub message: String,
    /// Optional session ID to resume a previous conversation.
    pub session_id: Option<String>,
    /// Optional project name to scope the agent's context.
    pub project: Option<String>,
}

/// Response body for `POST /chat`.
#[derive(Debug, Serialize)]
pub struct ChatResponse {
    /// The agent's response text.
    pub response: String,
    /// Session ID (new or resumed) for continuing the conversation.
    pub session_id: String,
    /// Estimated cost of this turn in USD.
    pub cost_usd: f64,
}

/// `POST /chat` — run a single agent turn and return the response.
pub async fn chat(
    State(state): State<Arc<AppState>>,
    axum::Json(req): axum::Json<ChatRequest>,
) -> Result<impl IntoResponse, ServerError> {
    let mut agent = state
        .agent_session
        .create_agent(&req.agent)
        .await
        .map_err(|e| match e {
            AivyxError::Io(_) | AivyxError::Config(_) => ServerError(AivyxError::Config(format!(
                "agent not found: {}",
                req.agent
            ))),
            other => ServerError(other),
        })?;

    // Reject Leash/Locked tiers — server has no interactive channel
    reject_non_autonomous(&agent)?;

    // Set active project if specified
    if let Some(ref project_name) = req.project
        && let Some(project) = state.config.find_project(project_name)
    {
        agent.set_active_project(project.clone());
    }

    // Restore conversation if session_id provided
    if let Some(ref id_str) = req.session_id {
        let session_id: SessionId = id_str.parse().map_err(|_| {
            ServerError(AivyxError::Config(format!("invalid session ID: {id_str}")))
        })?;
        if let Some(persisted) = state.session_store.load(
            &session_id,
            &state.master_key,
            state.config.memory.session_max_age_hours,
        )? {
            agent.restore_conversation(persisted.messages);
        }
    }

    // Wire memory manager if available
    if let Some(ref mm) = state.memory_manager {
        agent.set_memory_manager(mm.clone());
    }

    // Run the turn (no channel adapter — Trust/Free tier only)
    let response = agent.turn(&req.message, None).await?;

    // Save session
    let persisted = agent.to_persisted_session();
    if let Err(e) = state.session_store.save(&persisted, &state.master_key) {
        tracing::warn!("failed to save session: {e}");
    }

    Ok(axum::Json(ChatResponse {
        response,
        session_id: agent.session_id().to_string(),
        cost_usd: agent.current_cost_usd(),
    }))
}

/// `POST /chat/stream` — SSE streaming agent turn.
///
/// Returns an SSE stream with OpenAI-compatible event format:
/// - `data: {"type":"token","content":"..."}`  — incremental tokens
/// - `data: {"type":"done","response":"...","session_id":"...","cost_usd":0.001}` — final
/// - `data: [DONE]` — stream end sentinel
/// - `data: {"type":"error","message":"..."}` — on error
pub async fn stream_chat(
    State(state): State<Arc<AppState>>,
    axum::Json(req): axum::Json<ChatRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ServerError> {
    let mut agent = state
        .agent_session
        .create_agent(&req.agent)
        .await
        .map_err(|e| match e {
            AivyxError::Io(_) | AivyxError::Config(_) => ServerError(AivyxError::Config(format!(
                "agent not found: {}",
                req.agent
            ))),
            other => ServerError(other),
        })?;

    reject_non_autonomous(&agent)?;

    // Set active project if specified
    if let Some(ref project_name) = req.project
        && let Some(project) = state.config.find_project(project_name)
    {
        agent.set_active_project(project.clone());
    }

    // Restore conversation if session_id provided
    if let Some(ref id_str) = req.session_id {
        let session_id: SessionId = id_str.parse().map_err(|_| {
            ServerError(AivyxError::Config(format!("invalid session ID: {id_str}")))
        })?;
        if let Some(persisted) = state.session_store.load(
            &session_id,
            &state.master_key,
            state.config.memory.session_max_age_hours,
        )? {
            agent.restore_conversation(persisted.messages);
        }
    }

    // Wire memory manager if available
    if let Some(ref mm) = state.memory_manager {
        agent.set_memory_manager(mm.clone());
    }

    // Create the mpsc channel for token streaming
    let (token_tx, token_rx) = tokio::sync::mpsc::channel::<String>(64);

    // Channel for the SSE events (tokens + done/error)
    let (sse_tx, sse_rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);

    let message = req.message.clone();
    let state_clone = state.clone();

    // Spawn the agent turn in a background task
    tokio::spawn(async move {
        let token_rx_stream = token_rx;

        // Spawn a forwarder that sends tokens as SSE events
        let sse_tx_tokens = sse_tx.clone();
        let forwarder = tokio::spawn(async move {
            let mut rx = token_rx_stream;
            let mut accumulated = String::new();
            while let Some(token) = rx.recv().await {
                accumulated.push_str(&token);
                let event_data = serde_json::json!({"type": "token", "content": token});
                let event = Event::default().data(event_data.to_string());
                if sse_tx_tokens.send(Ok(event)).await.is_err() {
                    break;
                }
            }
            accumulated
        });

        // Run the agent turn
        match agent.turn_stream(&message, None, token_tx, None).await {
            Ok(response) => {
                // Wait for forwarder to finish
                let _ = forwarder.await;

                // Save session
                let persisted = agent.to_persisted_session();
                if let Err(e) = state_clone
                    .session_store
                    .save(&persisted, &state_clone.master_key)
                {
                    tracing::warn!("failed to save session: {e}");
                }

                // Send done event
                let done_data = serde_json::json!({
                    "type": "done",
                    "response": response,
                    "session_id": agent.session_id().to_string(),
                    "cost_usd": agent.current_cost_usd(),
                });
                let done_event = Event::default().data(done_data.to_string());
                // Client may have disconnected; safe to discard
                let _ = sse_tx.send(Ok(done_event)).await;

                // Send [DONE] sentinel
                let sentinel = Event::default().data("[DONE]");
                // Client may have disconnected; safe to discard
                let _ = sse_tx.send(Ok(sentinel)).await;
            }
            Err(e) => {
                // Wait for forwarder
                let _ = forwarder.await;

                // Send error event
                let error_data = serde_json::json!({"type": "error", "message": e.to_string()});
                let error_event = Event::default().data(error_data.to_string());
                // Client may have disconnected; safe to discard
                let _ = sse_tx.send(Ok(error_event)).await;

                // Send [DONE] sentinel even on error
                let sentinel = Event::default().data("[DONE]");
                // Client may have disconnected; safe to discard
                let _ = sse_tx.send(Ok(sentinel)).await;
            }
        }
    });

    let stream = ReceiverStream::new(sse_rx);
    Ok(Sse::new(stream))
}

/// Response body for `POST /chat/audio`.
#[derive(Debug, Serialize)]
pub struct AudioChatResponse {
    /// The transcribed text from the audio input.
    pub transcription: String,
    /// The agent's response text.
    pub response: String,
    /// Session ID (new or resumed) for continuing the conversation.
    pub session_id: String,
    /// Estimated cost of this turn in USD.
    pub cost_usd: f64,
}

/// `POST /chat/audio` — upload audio, transcribe it, and run an agent turn.
///
/// Accepts a multipart form with the following fields:
/// - `audio` (required): the audio file (wav, mp3, flac, ogg, webm, m4a)
/// - `agent` (optional): agent profile name (defaults to `"aivyx"`)
/// - `session_id` (optional): session to resume
/// - `project` (optional): project context
pub async fn audio_chat(
    State(state): State<Arc<AppState>>,
    mut multipart: axum::extract::Multipart,
) -> Result<impl IntoResponse, ServerError> {
    // Check that speech is configured
    let speech_config = state.config.speech.as_ref().ok_or_else(|| {
        ServerError(AivyxError::Config(
            "speech not configured — add [speech] section to config.toml".into(),
        ))
    })?;

    // Parse multipart fields
    let mut audio_bytes: Option<Vec<u8>> = None;
    let mut audio_filename = String::from("audio.wav");
    let mut agent_name = String::from("aivyx");
    let mut session_id: Option<String> = None;
    let mut project: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ServerError(AivyxError::Http(format!("multipart error: {e}"))))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "audio" => {
                if let Some(fname) = field.file_name() {
                    audio_filename = fname.to_string();
                }
                let bytes = field.bytes().await.map_err(|e| {
                    ServerError(AivyxError::Http(format!("failed to read audio field: {e}")))
                })?;
                audio_bytes = Some(bytes.to_vec());
            }
            "agent" => {
                let text = field.text().await.map_err(|e| {
                    ServerError(AivyxError::Http(format!("failed to read agent field: {e}")))
                })?;
                if !text.is_empty() {
                    agent_name = text;
                }
            }
            "session_id" => {
                let text = field.text().await.map_err(|e| {
                    ServerError(AivyxError::Http(format!(
                        "failed to read session_id field: {e}"
                    )))
                })?;
                if !text.is_empty() {
                    session_id = Some(text);
                }
            }
            "project" => {
                let text = field.text().await.map_err(|e| {
                    ServerError(AivyxError::Http(format!(
                        "failed to read project field: {e}"
                    )))
                })?;
                if !text.is_empty() {
                    project = Some(text);
                }
            }
            _ => {
                // Ignore unknown fields
            }
        }
    }

    let audio_bytes = audio_bytes.ok_or_else(|| {
        ServerError(AivyxError::Config(
            "missing required 'audio' field in multipart form".into(),
        ))
    })?;

    // Transcribe
    let result = crate::transcription::transcribe(
        speech_config,
        audio_bytes,
        &audio_filename,
        &state.master_key,
        &state.dirs.store_path(),
    )
    .await?;

    // Audit-log the transcription
    let duration = result.duration_secs.unwrap_or(0.0);
    if let Err(e) = state.audit_log.append(AuditEvent::AudioTranscribed {
        model: speech_config.model.clone(),
        duration_secs: duration,
    }) {
        tracing::warn!("failed to audit transcription: {e}");
    }

    let transcribed_text = result.text.clone();

    // Now run the agent turn with the transcribed text
    let mut agent = state
        .agent_session
        .create_agent(&agent_name)
        .await
        .map_err(|e| match e {
            AivyxError::Io(_) | AivyxError::Config(_) => ServerError(AivyxError::Config(format!(
                "agent not found: {}",
                agent_name
            ))),
            other => ServerError(other),
        })?;

    reject_non_autonomous(&agent)?;

    // Set active project if specified
    if let Some(ref project_name) = project
        && let Some(proj) = state.config.find_project(project_name)
    {
        agent.set_active_project(proj.clone());
    }

    // Restore conversation if session_id provided
    if let Some(ref id_str) = session_id {
        let sid: SessionId = id_str.parse().map_err(|_| {
            ServerError(AivyxError::Config(format!("invalid session ID: {id_str}")))
        })?;
        if let Some(persisted) = state.session_store.load(
            &sid,
            &state.master_key,
            state.config.memory.session_max_age_hours,
        )? {
            agent.restore_conversation(persisted.messages);
        }
    }

    // Wire memory manager if available
    if let Some(ref mm) = state.memory_manager {
        agent.set_memory_manager(mm.clone());
    }

    // Run the turn
    let response = agent.turn(&result.text, None).await?;

    // Save session
    let persisted = agent.to_persisted_session();
    if let Err(e) = state.session_store.save(&persisted, &state.master_key) {
        tracing::warn!("failed to save session: {e}");
    }

    Ok(axum::Json(AudioChatResponse {
        transcription: transcribed_text,
        response,
        session_id: agent.session_id().to_string(),
        cost_usd: agent.current_cost_usd(),
    }))
}

/// Rejects agents with Leash or Locked autonomy tiers.
///
/// The HTTP server has no interactive channel, so Leash-tier agents (which need
/// user approval for tool calls) and Locked-tier agents (all tool calls denied)
/// cannot function correctly. Only Trust and Free tiers are allowed.
fn reject_non_autonomous(agent: &aivyx_agent::Agent) -> Result<(), ServerError> {
    match agent.autonomy_tier() {
        aivyx_core::AutonomyTier::Locked => Err(ServerError(AivyxError::Config(
            "Locked-tier agents cannot be used via the HTTP API".into(),
        ))),
        aivyx_core::AutonomyTier::Leash => Err(ServerError(AivyxError::Config(
            "Leash-tier agents require an interactive channel (not supported via HTTP)".into(),
        ))),
        aivyx_core::AutonomyTier::Trust | aivyx_core::AutonomyTier::Free => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_request_deserializes() {
        let json = r#"{"agent":"test","message":"hello"}"#;
        let req: ChatRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.agent, "test");
        assert_eq!(req.message, "hello");
        assert!(req.session_id.is_none());
    }

    #[test]
    fn chat_request_with_session() {
        let json = r#"{"agent":"test","message":"hello","session_id":"abc-123"}"#;
        let req: ChatRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.session_id.as_deref(), Some("abc-123"));
        assert!(req.project.is_none());
    }

    #[test]
    fn chat_request_with_project() {
        let json = r#"{"agent":"test","message":"hello","project":"my-app"}"#;
        let req: ChatRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.project.as_deref(), Some("my-app"));
    }

    #[test]
    fn sse_token_event_format() {
        let data = serde_json::json!({"type": "token", "content": "Hello"});
        assert_eq!(data["type"], "token");
        assert_eq!(data["content"], "Hello");
    }

    #[test]
    fn sse_done_event_format() {
        let data = serde_json::json!({
            "type": "done",
            "response": "Hello world",
            "session_id": "sess-1",
            "cost_usd": 0.001,
        });
        assert_eq!(data["type"], "done");
        assert!(data["session_id"].is_string());
        assert!(data["cost_usd"].is_f64());
    }

    #[test]
    fn sse_error_event_format() {
        let data = serde_json::json!({"type": "error", "message": "provider failed"});
        assert_eq!(data["type"], "error");
        assert_eq!(data["message"], "provider failed");
    }

    #[test]
    fn chat_response_serializes() {
        let resp = ChatResponse {
            response: "Hi there".into(),
            session_id: "sess-1".into(),
            cost_usd: 0.001,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["response"], "Hi there");
        assert_eq!(json["session_id"], "sess-1");
    }

    #[test]
    fn audio_chat_response_serializes() {
        let resp = AudioChatResponse {
            transcription: "Hello world".into(),
            response: "Hi there".into(),
            session_id: "sess-1".into(),
            cost_usd: 0.002,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["transcription"], "Hello world");
        assert_eq!(json["response"], "Hi there");
        assert_eq!(json["session_id"], "sess-1");
        assert!(json["cost_usd"].is_f64());
    }
}

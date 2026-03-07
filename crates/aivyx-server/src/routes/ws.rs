//! WebSocket endpoint for bidirectional agent communication.
//!
//! `GET /ws` — upgrades to WebSocket for real-time streaming, cancellation,
//! and Leash-tier approval flows. Complements the SSE streaming endpoint
//! with full bidirectional support.
//!
//! ## Protocol
//!
//! Client sends JSON text frames:
//! - `{"type":"auth","token":"..."}` — first message, required
//! - `{"type":"message","text":"...","agent":"...","session_id":"...","project":"..."}`
//! - `{"type":"cancel"}` — cancel in-flight agent turn
//! - `{"type":"approval_response","request_id":"...","approved":true}`
//! - `{"type":"ping"}`
//!
//! Server sends JSON text frames:
//! - `{"type":"auth_ok"}`
//! - `{"type":"auth_error","message":"..."}`
//! - `{"type":"token","content":"..."}`
//! - `{"type":"done","response":"...","session_id":"...","cost_usd":N}`
//! - `{"type":"error","message":"..."}`
//! - `{"type":"approval_request","tool":"...","input":"...","request_id":"..."}`
//! - `{"type":"pong"}`

use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use aivyx_core::{AivyxError, Result, SessionId};

use crate::app_state::AppState;

/// Auth timeout: client must send auth message within 5 seconds.
const AUTH_TIMEOUT_SECS: u64 = 5;

// ── Client → Server messages ───────────────────────────────────────────

/// Incoming WebSocket message from client.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    /// First message: authenticate with bearer token.
    Auth { token: String },
    /// Send a chat message to an agent (optionally with images).
    Message {
        text: String,
        #[serde(default)]
        images: Vec<crate::routes::chat::ImageInput>,
        #[serde(default = "default_agent")]
        agent: String,
        session_id: Option<String>,
        project: Option<String>,
    },
    /// Cancel the current in-flight agent turn.
    Cancel,
    /// Respond to a Leash-tier approval request.
    ApprovalResponse { request_id: String, approved: bool },
    /// Respond to a task-level approval gate.
    TaskApprovalResponse {
        request_id: String,
        approved: bool,
        #[allow(dead_code)]
        reason: Option<String>,
    },
    /// Keepalive ping.
    Ping,
}

fn default_agent() -> String {
    "aivyx".to_string()
}

// ── Server → Client messages ───────────────────────────────────────────

/// Outgoing WebSocket message to client.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMessage {
    /// Auth succeeded.
    AuthOk,
    /// Auth failed.
    AuthError { message: String },
    /// Streaming text delta.
    Token { content: String },
    /// Agent turn completed.
    Done {
        response: String,
        session_id: String,
        cost_usd: f64,
    },
    /// Error during agent turn.
    Error { message: String },
    /// Leash-tier tool approval request.
    ApprovalRequest {
        tool: String,
        input: String,
        request_id: String,
    },
    /// Task-level approval gate request.
    #[allow(dead_code)]
    TaskApprovalRequest {
        task_id: String,
        step_index: usize,
        context: String,
        request_id: String,
        timeout_secs: Option<u64>,
    },
    /// Keepalive pong.
    Pong,
}

// ── WebSocket ChannelAdapter for Leash-tier agents ────────────────────

/// Bridges agent Leash-tier approval to the WebSocket client.
///
/// When the agent calls `channel.send(prompt)`, the prompt is forwarded
/// as an `approval_request` frame. When the client responds with
/// `approval_response`, the answer flows back through `channel.receive()`.
struct WsChannelAdapter {
    /// Send approval requests to the WebSocket writer task.
    outgoing_tx: mpsc::Sender<ServerMessage>,
    /// Receive approval responses from the WebSocket reader task.
    /// Shared with the connection handler (one channel per connection).
    approval_rx: Arc<tokio::sync::Mutex<mpsc::Receiver<ApprovalAnswer>>>,
    /// Counter for generating unique request IDs.
    request_counter: std::sync::atomic::AtomicU64,
}

/// An approval response from the client.
struct ApprovalAnswer {
    #[allow(dead_code)]
    request_id: String,
    approved: bool,
}

#[async_trait]
impl aivyx_core::ChannelAdapter for WsChannelAdapter {
    async fn send(&self, message: &str) -> Result<()> {
        let request_id = self
            .request_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .to_string();

        // Parse tool name and input from the approval prompt.
        // The agent formats it as: "Tool `{name}` wants to execute with input: {input}\nApprove? [y/n]"
        let (tool, input) = parse_approval_prompt(message);

        self.outgoing_tx
            .send(ServerMessage::ApprovalRequest {
                tool,
                input,
                request_id,
            })
            .await
            .map_err(|_| AivyxError::Channel("WebSocket connection closed".into()))?;
        Ok(())
    }

    async fn receive(&self) -> Result<String> {
        let answer = self
            .approval_rx
            .lock()
            .await
            .recv()
            .await
            .ok_or_else(|| AivyxError::Channel("WebSocket connection closed".into()))?;
        Ok(if answer.approved { "y" } else { "n" }.to_string())
    }
}

/// Parse approval prompt into (tool_name, input) for structured WS messages.
fn parse_approval_prompt(prompt: &str) -> (String, String) {
    // Try to extract tool name from backtick-quoted pattern
    if let Some(start) = prompt.find('`')
        && let Some(end) = prompt[start + 1..].find('`')
    {
        let tool = prompt[start + 1..start + 1 + end].to_string();
        let input = prompt
            .find("input:")
            .map(|i| prompt[i + 6..].trim().to_string())
            .unwrap_or_default();
        return (tool, input);
    }
    ("unknown".to_string(), prompt.to_string())
}

// ── Handler ────────────────────────────────────────────────────────────

/// `GET /ws` — WebSocket upgrade handler.
///
/// Upgrades the HTTP connection to WebSocket. Authentication happens
/// as the first message (not via HTTP headers), keeping the token out
/// of URLs and server logs.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let max_size = state
        .config
        .read()
        .await
        .server
        .as_ref()
        .map(|s| s.ws_max_message_size)
        .unwrap_or(1_048_576);

    ws.max_message_size(max_size)
        .max_frame_size(max_size)
        .max_write_buffer_size(max_size)
        .on_upgrade(|socket| handle_connection(socket, state))
}

/// Main WebSocket connection handler.
async fn handle_connection(socket: WebSocket, state: Arc<AppState>) {
    let (ws_sink, ws_stream) = socket.split();

    // Wrap sink in Arc<Mutex> for sharing between tasks
    let ws_sink = Arc::new(tokio::sync::Mutex::new(ws_sink));

    // Channel for outgoing server messages (multiple producers → single WS writer)
    let (outgoing_tx, mut outgoing_rx) = mpsc::channel::<ServerMessage>(64);

    // Spawn the writer task: reads from outgoing channel, writes to WebSocket
    let writer_sink = ws_sink.clone();
    let writer_handle = tokio::spawn(async move {
        while let Some(msg) = outgoing_rx.recv().await {
            let json = match serde_json::to_string(&msg) {
                Ok(j) => j,
                Err(e) => {
                    tracing::error!("Failed to serialize WS message: {e}");
                    continue;
                }
            };
            let mut sink = writer_sink.lock().await;
            if sink.send(Message::Text(json.into())).await.is_err() {
                break; // Client disconnected
            }
        }
    });

    // Wrap the incoming stream for reading
    let mut reader = ws_stream;

    // Step 1: Authenticate
    if !authenticate(&mut reader, &outgoing_tx, &state).await {
        let _ = writer_handle.await;
        return;
    }

    // Step 2: Message loop
    // Track the current in-flight agent turn for cancellation
    let cancel_token: Arc<tokio::sync::Mutex<Option<CancellationToken>>> =
        Arc::new(tokio::sync::Mutex::new(None));

    // Approval channel: reader sends responses, adapter receives them.
    // One channel per connection; only one turn is active at a time.
    let (approval_tx, approval_rx) = mpsc::channel::<ApprovalAnswer>(4);
    let approval_rx = Arc::new(tokio::sync::Mutex::new(approval_rx));

    loop {
        let msg = match reader.next().await {
            Some(Ok(Message::Text(text))) => text,
            Some(Ok(Message::Close(_))) | None => break,
            Some(Ok(Message::Ping(data))) => {
                let mut sink = ws_sink.lock().await;
                let _ = sink.send(Message::Pong(data)).await;
                continue;
            }
            Some(Ok(_)) => continue, // Binary, Pong — ignore
            Some(Err(e)) => {
                tracing::debug!("WebSocket read error: {e}");
                break;
            }
        };

        // Defense-in-depth size check (tungstenite enforces max_message_size
        // at the frame level, but we audit any message that slips through).
        let ws_max = state
            .config
            .read()
            .await
            .server
            .as_ref()
            .map(|s| s.ws_max_message_size)
            .unwrap_or(1_048_576);
        if msg.len() > ws_max {
            let _ = state
                .audit_log
                .append(aivyx_audit::AuditEvent::WebSocketFrameTooLarge {
                    size_bytes: msg.len(),
                    max_bytes: ws_max,
                });
            let _ = outgoing_tx
                .send(ServerMessage::Error {
                    message: format!("message too large: {} bytes (max {})", msg.len(), ws_max),
                })
                .await;
            continue;
        }

        let client_msg: ClientMessage = match serde_json::from_str(&msg) {
            Ok(m) => m,
            Err(e) => {
                let _ = outgoing_tx
                    .send(ServerMessage::Error {
                        message: format!("invalid message: {e}"),
                    })
                    .await;
                continue;
            }
        };

        match client_msg {
            ClientMessage::Auth { .. } => {
                // Already authenticated — ignore duplicate auth
                let _ = outgoing_tx.send(ServerMessage::AuthOk).await;
            }
            ClientMessage::Message {
                text,
                images,
                agent,
                session_id,
                project,
            } => {
                // Cancel any previous in-flight turn
                {
                    let ct = cancel_token.lock().await;
                    if let Some(ref token) = *ct {
                        token.cancel();
                    }
                }

                // Create a new cancellation token for this turn
                let token = CancellationToken::new();
                {
                    let mut ct = cancel_token.lock().await;
                    *ct = Some(token.clone());
                }

                // Create the WsChannelAdapter for Leash-tier support
                let adapter = WsChannelAdapter {
                    outgoing_tx: outgoing_tx.clone(),
                    approval_rx: approval_rx.clone(),
                    request_counter: std::sync::atomic::AtomicU64::new(0),
                };

                // Spawn the agent turn
                let state_clone = state.clone();
                let turn_tx = outgoing_tx.clone();
                tokio::spawn(async move {
                    run_agent_turn(
                        state_clone,
                        turn_tx,
                        TurnParams {
                            text,
                            images,
                            agent_name: agent,
                            session_id,
                            project,
                            adapter,
                            cancel_token: token,
                        },
                    )
                    .await;
                });
            }
            ClientMessage::Cancel => {
                let ct = cancel_token.lock().await;
                if let Some(ref token) = *ct {
                    token.cancel();
                    tracing::info!("WebSocket: client requested turn cancellation");
                }
            }
            ClientMessage::ApprovalResponse {
                request_id,
                approved,
            } => {
                let _ = approval_tx
                    .send(ApprovalAnswer {
                        request_id,
                        approved,
                    })
                    .await;
            }
            ClientMessage::TaskApprovalResponse {
                request_id,
                approved,
                reason: _,
            } => {
                // Task approval responses use the same channel as leash approvals
                let _ = approval_tx
                    .send(ApprovalAnswer {
                        request_id,
                        approved,
                    })
                    .await;
            }
            ClientMessage::Ping => {
                let _ = outgoing_tx.send(ServerMessage::Pong).await;
            }
        }
    }

    // Clean up
    drop(outgoing_tx);
    let _ = writer_handle.await;
}

/// Authenticate the WebSocket connection.
///
/// Waits for the first message to be an `auth` message with a valid bearer
/// token. Returns `true` if authentication succeeds, `false` otherwise.
async fn authenticate(
    reader: &mut (impl StreamExt<Item = std::result::Result<Message, axum::Error>> + Unpin),
    outgoing_tx: &mpsc::Sender<ServerMessage>,
    state: &AppState,
) -> bool {
    let auth_result = tokio::time::timeout(
        std::time::Duration::from_secs(AUTH_TIMEOUT_SECS),
        reader.next(),
    )
    .await;

    let msg = match auth_result {
        Ok(Some(Ok(Message::Text(text)))) => text,
        Ok(Some(Ok(_))) => {
            let _ = outgoing_tx
                .send(ServerMessage::AuthError {
                    message: "expected text message with auth token".into(),
                })
                .await;
            return false;
        }
        _ => {
            let _ = outgoing_tx
                .send(ServerMessage::AuthError {
                    message: "auth timeout or connection error".into(),
                })
                .await;
            return false;
        }
    };

    let client_msg: ClientMessage = match serde_json::from_str(&msg) {
        Ok(m) => m,
        Err(_) => {
            let _ = outgoing_tx
                .send(ServerMessage::AuthError {
                    message: "invalid auth message format".into(),
                })
                .await;
            return false;
        }
    };

    let token = match client_msg {
        ClientMessage::Auth { token } => token,
        _ => {
            let _ = outgoing_tx
                .send(ServerMessage::AuthError {
                    message: "first message must be auth".into(),
                })
                .await;
            return false;
        }
    };

    // SHA-256 hash and constant-time compare (same as HTTP auth middleware)
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let provided_hash: [u8; 32] = hasher.finalize().into();
    let current_hash = *state.bearer_token_hash.read().await;

    if provided_hash.ct_eq(&current_hash).unwrap_u8() == 0 {
        if let Err(e) = state
            .audit_log
            .append(aivyx_audit::AuditEvent::HttpAuthFailed {
                remote_addr: "websocket".to_string(),
                reason: "invalid bearer token".to_string(),
            })
        {
            tracing::error!("failed to audit WS auth failure: {e}");
        }
        let _ = outgoing_tx
            .send(ServerMessage::AuthError {
                message: "invalid bearer token".into(),
            })
            .await;
        return false;
    }

    let _ = outgoing_tx.send(ServerMessage::AuthOk).await;
    true
}

/// Parameters for an agent turn over WebSocket.
struct TurnParams {
    text: String,
    images: Vec<crate::routes::chat::ImageInput>,
    agent_name: String,
    session_id: Option<String>,
    project: Option<String>,
    adapter: WsChannelAdapter,
    cancel_token: CancellationToken,
}

/// Run an agent turn with streaming token output over WebSocket.
async fn run_agent_turn(
    state: Arc<AppState>,
    outgoing_tx: mpsc::Sender<ServerMessage>,
    params: TurnParams,
) {
    // Create agent
    let mut agent = match state.agent_session.create_agent(&params.agent_name).await {
        Ok(a) => a,
        Err(e) => {
            let _ = outgoing_tx
                .send(ServerMessage::Error {
                    message: format!("agent creation failed: {e}"),
                })
                .await;
            return;
        }
    };

    // Set active project if specified
    let config = state.config.read().await;
    if let Some(ref project_name) = params.project
        && let Some(proj) = config.find_project(project_name)
    {
        agent.set_active_project(proj.clone());
    }

    // Restore conversation if session_id provided
    if let Some(ref id_str) = params.session_id
        && let Ok(sid) = id_str.parse::<SessionId>()
        && let Ok(Some(persisted)) = state.session_store.load(
            &sid,
            &state.master_key,
            config.memory.session_max_age_hours,
        )
    {
        agent.restore_conversation(persisted.messages);
    }
    drop(config);

    // Wire memory manager if available
    if let Some(ref mm) = state.memory_manager {
        agent.set_memory_manager(mm.clone());
    }

    // Determine if we can use the channel adapter (Leash-tier support)
    let use_channel = matches!(agent.autonomy_tier(), aivyx_core::AutonomyTier::Leash);

    // Set up token streaming channel
    let (token_tx, mut token_rx) = mpsc::channel::<String>(64);

    // Forward tokens to WebSocket
    let token_outgoing = outgoing_tx.clone();
    let token_forwarder = tokio::spawn(async move {
        while let Some(token) = token_rx.recv().await {
            if token_outgoing
                .send(ServerMessage::Token { content: token })
                .await
                .is_err()
            {
                break;
            }
        }
    });

    // Run the agent turn
    let channel_ref: Option<&dyn aivyx_core::ChannelAdapter> = if use_channel {
        Some(&params.adapter)
    } else {
        None
    };

    // Reject Locked-tier agents
    if matches!(agent.autonomy_tier(), aivyx_core::AutonomyTier::Locked) {
        let _ = outgoing_tx
            .send(ServerMessage::Error {
                message: "Locked-tier agents cannot be used".into(),
            })
            .await;
        return;
    }

    let result = if params.images.is_empty() {
        agent
            .turn_stream(
                &params.text,
                channel_ref,
                token_tx,
                Some(params.cancel_token),
            )
            .await
    } else {
        agent
            .turn_stream_with_content(
                crate::routes::chat::build_multimodal_content(&params.text, &params.images),
                channel_ref,
                token_tx,
                Some(params.cancel_token),
            )
            .await
    };

    // Wait for token forwarder to finish
    let _ = token_forwarder.await;

    match result {
        Ok(response) => {
            // Save session
            let persisted = agent.to_persisted_session();
            if let Err(e) = state.session_store.save(&persisted, &state.master_key) {
                tracing::warn!("failed to save WS session: {e}");
            }

            let _ = outgoing_tx
                .send(ServerMessage::Done {
                    response,
                    session_id: agent.session_id().to_string(),
                    cost_usd: agent.current_cost_usd(),
                })
                .await;
        }
        Err(e) => {
            let _ = outgoing_tx
                .send(ServerMessage::Error {
                    message: e.to_string(),
                })
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_message_auth_deserializes() {
        let json = r#"{"type":"auth","token":"test-token"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, ClientMessage::Auth { token } if token == "test-token"));
    }

    #[test]
    fn client_message_chat_deserializes() {
        let json = r#"{"type":"message","text":"hello","agent":"assistant"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(
            matches!(msg, ClientMessage::Message { text, agent, .. } if text == "hello" && agent == "assistant")
        );
    }

    #[test]
    fn client_message_chat_default_agent() {
        let json = r#"{"type":"message","text":"hello"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, ClientMessage::Message { agent, .. } if agent == "aivyx"));
    }

    #[test]
    fn client_message_cancel_deserializes() {
        let json = r#"{"type":"cancel"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, ClientMessage::Cancel));
    }

    #[test]
    fn client_message_approval_response_deserializes() {
        let json = r#"{"type":"approval_response","request_id":"42","approved":true}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(
            matches!(msg, ClientMessage::ApprovalResponse { request_id, approved } if request_id == "42" && approved)
        );
    }

    #[test]
    fn client_message_ping_deserializes() {
        let json = r#"{"type":"ping"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, ClientMessage::Ping));
    }

    #[test]
    fn server_message_auth_ok_serializes() {
        let msg = ServerMessage::AuthOk;
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "auth_ok");
    }

    #[test]
    fn server_message_token_serializes() {
        let msg = ServerMessage::Token {
            content: "Hello".into(),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "token");
        assert_eq!(json["content"], "Hello");
    }

    #[test]
    fn server_message_done_serializes() {
        let msg = ServerMessage::Done {
            response: "Hi there".into(),
            session_id: "sess-1".into(),
            cost_usd: 0.001,
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "done");
        assert_eq!(json["response"], "Hi there");
        assert_eq!(json["session_id"], "sess-1");
        assert!(json["cost_usd"].is_f64());
    }

    #[test]
    fn server_message_error_serializes() {
        let msg = ServerMessage::Error {
            message: "something broke".into(),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "error");
        assert_eq!(json["message"], "something broke");
    }

    #[test]
    fn server_message_approval_request_serializes() {
        let msg = ServerMessage::ApprovalRequest {
            tool: "shell".into(),
            input: "rm -rf /".into(),
            request_id: "1".into(),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "approval_request");
        assert_eq!(json["tool"], "shell");
        assert_eq!(json["request_id"], "1");
    }

    #[test]
    fn parse_approval_prompt_extracts_tool() {
        let prompt = "Tool `shell` wants to execute with input: ls -la\nApprove? [y/n]";
        let (tool, input) = parse_approval_prompt(prompt);
        assert_eq!(tool, "shell");
        assert!(input.contains("ls -la"));
    }

    #[test]
    fn parse_approval_prompt_fallback() {
        let prompt = "Do you approve this action?";
        let (tool, input) = parse_approval_prompt(prompt);
        assert_eq!(tool, "unknown");
        assert_eq!(input, prompt);
    }

    #[test]
    fn server_message_pong_serializes() {
        let msg = ServerMessage::Pong;
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "pong");
    }

    #[test]
    fn server_message_auth_error_serializes() {
        let msg = ServerMessage::AuthError {
            message: "bad token".into(),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "auth_error");
        assert_eq!(json["message"], "bad token");
    }

    #[test]
    fn client_message_with_images_deserializes() {
        let json = r#"{
            "type": "message",
            "text": "what is this?",
            "images": [{"media_type": "image/png", "data": "iVBOR"}],
            "agent": "vision-agent"
        }"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::Message { text, images, agent, .. } => {
                assert_eq!(text, "what is this?");
                assert_eq!(images.len(), 1);
                assert_eq!(images[0].media_type, "image/png");
                assert_eq!(agent, "vision-agent");
            }
            _ => panic!("expected Message variant"),
        }
    }

    #[test]
    fn client_message_without_images_backward_compat() {
        let json = r#"{"type":"message","text":"hello","agent":"test"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::Message { images, .. } => assert!(images.is_empty()),
            _ => panic!("expected Message variant"),
        }
    }

    mod fuzz {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn fuzz_client_message_parse_never_panics(s in "\\PC*") {
                let _ = serde_json::from_str::<ClientMessage>(&s);
            }

            #[test]
            fn fuzz_server_message_serialize_never_panics(content in "\\PC{0,200}") {
                let msg = ServerMessage::Token { content };
                let _ = serde_json::to_string(&msg).unwrap();
            }

            #[test]
            fn message_variant_is_never_auth(
                text in "[a-zA-Z0-9 ]{1,50}",
                agent in "[a-z]{1,20}"
            ) {
                let json = serde_json::json!({
                    "type": "message",
                    "text": text,
                    "agent": agent
                });
                let msg: ClientMessage = serde_json::from_value(json).unwrap();
                let is_auth = matches!(msg, ClientMessage::Auth { .. });
                prop_assert!(!is_auth);
            }

            #[test]
            fn approval_response_deserialize(
                request_id in "[a-zA-Z0-9-]{1,36}",
                approved in proptest::bool::ANY
            ) {
                let json = serde_json::json!({
                    "type": "approval_response",
                    "request_id": request_id,
                    "approved": approved
                });
                let msg: ClientMessage = serde_json::from_value(json).unwrap();
                match msg {
                    ClientMessage::ApprovalResponse { request_id: rid, approved: app } => {
                        prop_assert_eq!(rid, request_id);
                        prop_assert_eq!(app, approved);
                    }
                    _ => prop_assert!(false, "expected ApprovalResponse variant"),
                }
            }
        }
    }
}

//! Real-time voice WebSocket endpoint.
//!
//! `GET /ws/voice` — upgrades to a WebSocket with bidirectional audio
//! streaming for voice conversations. Uses binary frames for PCM/MP3
//! audio and text frames for control messages.
//!
//! ## Protocol
//!
//! **Client → Server (text frames, JSON):**
//! - `{"type":"auth","token":"..."}` — first message, required
//! - `{"type":"config","agent":"...","session_id":"...","voice":"alloy"}`
//! - `{"type":"interrupt"}` — barge-in: cancel TTS playback
//! - `{"type":"end_utterance"}` — signal end of speech (push-to-talk mode)
//! - `{"type":"ping"}`
//!
//! **Client → Server (binary frames):**
//! - Raw audio chunks (expected format depends on client, typically PCM s16le 16kHz mono)
//!
//! **Server → Client (text frames, JSON):**
//! - `{"type":"auth_ok"}`
//! - `{"type":"auth_error","message":"..."}`
//! - `{"type":"transcript","text":"...","is_final":true}`
//! - `{"type":"agent_text","content":"..."}`
//! - `{"type":"speaking"}` — TTS audio about to start
//! - `{"type":"done","session_id":"...","cost_usd":0.001}`
//! - `{"type":"error","message":"..."}`
//! - `{"type":"pong"}`
//!
//! **Server → Client (binary frames):**
//! - TTS audio chunks (MP3 by default)

use std::sync::Arc;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use aivyx_core::SessionId;
use aivyx_llm::{
    AudioFormat, TtsAudioFormat, TtsOptions, create_stt_provider, create_tts_provider,
};

use crate::app_state::AppState;

/// Auth timeout: client must send auth message within 5 seconds.
const AUTH_TIMEOUT_SECS: u64 = 5;

// ── Client → Server messages ───────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum VoiceClientMessage {
    /// First message: authenticate with bearer token.
    Auth { token: String },
    /// Configure the voice session.
    Config {
        #[serde(default = "default_agent")]
        agent: String,
        session_id: Option<String>,
        project: Option<String>,
        #[serde(default = "default_voice")]
        voice: String,
    },
    /// Barge-in: cancel in-flight TTS and agent processing.
    Interrupt,
    /// Signal end of user speech (push-to-talk mode).
    EndUtterance,
    /// Keepalive.
    Ping,
}

fn default_agent() -> String {
    "aivyx".into()
}

fn default_voice() -> String {
    "alloy".into()
}

// ── Server → Client messages ───────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum VoiceServerMessage {
    AuthOk,
    AuthError {
        message: String,
    },
    /// Transcription result from STT.
    Transcript {
        text: String,
        is_final: bool,
    },
    /// Streamed text from the agent.
    AgentText {
        content: String,
    },
    /// About to start sending TTS audio.
    Speaking,
    /// Turn complete.
    Done {
        session_id: String,
        cost_usd: f64,
    },
    /// Error occurred.
    Error {
        message: String,
    },
    Pong,
}

/// Voice session configuration (set via `config` message).
struct VoiceSessionConfig {
    agent: String,
    session_id: Option<String>,
    project: Option<String>,
    voice: String,
}

impl Default for VoiceSessionConfig {
    fn default() -> Self {
        Self {
            agent: "aivyx".into(),
            session_id: None,
            project: None,
            voice: "alloy".into(),
        }
    }
}

// ── Handler ────────────────────────────────────────────────────────────

/// `GET /ws/voice` — WebSocket upgrade handler for voice.
pub async fn voice_handler(
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

    ws.max_message_size(max_size * 4) // Larger for audio
        .max_frame_size(max_size * 4)
        .max_write_buffer_size(max_size * 4)
        .on_upgrade(|socket| handle_voice_connection(socket, state))
}

/// Main voice WebSocket connection handler.
async fn handle_voice_connection(socket: WebSocket, state: Arc<AppState>) {
    let (ws_sink, ws_stream) = socket.split();
    let ws_sink = Arc::new(tokio::sync::Mutex::new(ws_sink));

    // Channel for outgoing server messages
    let (outgoing_tx, mut outgoing_rx) = mpsc::channel::<OutgoingMessage>(128);

    // Writer task: serializes messages and sends to WebSocket
    let writer_sink = ws_sink.clone();
    let writer_handle = tokio::spawn(async move {
        while let Some(msg) = outgoing_rx.recv().await {
            let ws_msg = match msg {
                OutgoingMessage::Text(server_msg) => match serde_json::to_string(&server_msg) {
                    Ok(json) => Message::Text(json.into()),
                    Err(e) => {
                        tracing::error!("Failed to serialize voice WS message: {e}");
                        continue;
                    }
                },
                OutgoingMessage::Binary(data) => Message::Binary(data.into()),
            };
            let mut sink = writer_sink.lock().await;
            if sink.send(ws_msg).await.is_err() {
                break;
            }
        }
    });

    let mut reader = ws_stream;

    // Step 1: Authenticate
    if !authenticate_voice(&mut reader, &outgoing_tx, &state).await {
        let _ = writer_handle.await;
        return;
    }

    // Session config and state
    let mut session_config = VoiceSessionConfig::default();
    let cancel_token: Arc<tokio::sync::Mutex<Option<CancellationToken>>> =
        Arc::new(tokio::sync::Mutex::new(None));

    // Audio buffer for accumulating voice input
    let (audio_tx, mut audio_rx) = mpsc::channel::<Vec<u8>>(256);

    loop {
        let msg = match reader.next().await {
            Some(Ok(Message::Text(text))) => IncomingFrame::Text(text.to_string()),
            Some(Ok(Message::Binary(data))) => IncomingFrame::Binary(data.to_vec()),
            Some(Ok(Message::Close(_))) | None => break,
            Some(Ok(Message::Ping(data))) => {
                let mut sink = ws_sink.lock().await;
                let _ = sink.send(Message::Pong(data)).await;
                continue;
            }
            Some(Ok(_)) => continue,
            Some(Err(e)) => {
                tracing::debug!("Voice WebSocket read error: {e}");
                break;
            }
        };

        match msg {
            IncomingFrame::Text(text) => {
                let client_msg: VoiceClientMessage = match serde_json::from_str(&text) {
                    Ok(m) => m,
                    Err(e) => {
                        let _ = outgoing_tx
                            .send(OutgoingMessage::Text(VoiceServerMessage::Error {
                                message: format!("invalid message: {e}"),
                            }))
                            .await;
                        continue;
                    }
                };

                match client_msg {
                    VoiceClientMessage::Auth { .. } => {
                        let _ = outgoing_tx
                            .send(OutgoingMessage::Text(VoiceServerMessage::AuthOk))
                            .await;
                    }
                    VoiceClientMessage::Config {
                        agent,
                        session_id,
                        project,
                        voice,
                    } => {
                        session_config = VoiceSessionConfig {
                            agent,
                            session_id,
                            project,
                            voice,
                        };
                    }
                    VoiceClientMessage::Interrupt => {
                        let ct = cancel_token.lock().await;
                        if let Some(ref token) = *ct {
                            token.cancel();
                            tracing::info!("Voice WS: client interrupted (barge-in)");
                        }
                    }
                    VoiceClientMessage::EndUtterance => {
                        // Process accumulated audio
                        let mut audio_chunks = Vec::new();
                        while let Ok(chunk) = audio_rx.try_recv() {
                            audio_chunks.push(chunk);
                        }

                        if audio_chunks.is_empty() {
                            continue;
                        }

                        let audio_data: Vec<u8> = audio_chunks.into_iter().flatten().collect();

                        // Cancel previous turn
                        {
                            let ct = cancel_token.lock().await;
                            if let Some(ref token) = *ct {
                                token.cancel();
                            }
                        }

                        let token = CancellationToken::new();
                        {
                            let mut ct = cancel_token.lock().await;
                            *ct = Some(token.clone());
                        }

                        // Spawn the voice turn pipeline
                        let state_clone = state.clone();
                        let tx = outgoing_tx.clone();
                        let config = VoiceTurnConfig {
                            agent: session_config.agent.clone(),
                            session_id: session_config.session_id.clone(),
                            project: session_config.project.clone(),
                            voice: session_config.voice.clone(),
                        };

                        tokio::spawn(async move {
                            run_voice_turn(state_clone, tx, audio_data, config, token).await;
                        });
                    }
                    VoiceClientMessage::Ping => {
                        let _ = outgoing_tx
                            .send(OutgoingMessage::Text(VoiceServerMessage::Pong))
                            .await;
                    }
                }
            }
            IncomingFrame::Binary(data) => {
                // Buffer audio chunks
                let _ = audio_tx.send(data).await;
            }
        }
    }

    drop(outgoing_tx);
    let _ = writer_handle.await;
}

/// Outgoing message types (text JSON or binary audio).
enum OutgoingMessage {
    Text(VoiceServerMessage),
    Binary(Vec<u8>),
}

/// Incoming frame types.
enum IncomingFrame {
    Text(String),
    Binary(Vec<u8>),
}

/// Config for a voice turn.
struct VoiceTurnConfig {
    agent: String,
    session_id: Option<String>,
    project: Option<String>,
    voice: String,
}

/// Run the full voice pipeline: STT → agent → TTS.
async fn run_voice_turn(
    state: Arc<AppState>,
    outgoing_tx: mpsc::Sender<OutgoingMessage>,
    audio_data: Vec<u8>,
    config: VoiceTurnConfig,
    cancel: CancellationToken,
) {
    // Step 1: STT — transcribe the audio
    let speech_config = {
        let app_config = state.config.read().await;
        match app_config.speech.clone() {
            Some(c) => c,
            None => {
                let _ = outgoing_tx
                    .send(OutgoingMessage::Text(VoiceServerMessage::Error {
                        message: "speech not configured".into(),
                    }))
                    .await;
                return;
            }
        }
    };

    let store = match aivyx_crypto::EncryptedStore::open(state.dirs.store_path()) {
        Ok(s) => s,
        Err(e) => {
            let _ = outgoing_tx
                .send(OutgoingMessage::Text(VoiceServerMessage::Error {
                    message: format!("store error: {e}"),
                }))
                .await;
            return;
        }
    };

    let stt_provider = match create_stt_provider(&speech_config, &store, &state.master_key) {
        Ok(p) => p,
        Err(e) => {
            let _ = outgoing_tx
                .send(OutgoingMessage::Text(VoiceServerMessage::Error {
                    message: format!("STT provider error: {e}"),
                }))
                .await;
            return;
        }
    };

    if cancel.is_cancelled() {
        return;
    }

    let stt_result = match stt_provider.transcribe(&audio_data, AudioFormat::Wav).await {
        Ok(r) => r,
        Err(e) => {
            let _ = outgoing_tx
                .send(OutgoingMessage::Text(VoiceServerMessage::Error {
                    message: format!("transcription failed: {e}"),
                }))
                .await;
            return;
        }
    };

    if stt_result.text.trim().is_empty() {
        return; // No speech detected
    }

    // Send transcript to client
    let _ = outgoing_tx
        .send(OutgoingMessage::Text(VoiceServerMessage::Transcript {
            text: stt_result.text.clone(),
            is_final: true,
        }))
        .await;

    if cancel.is_cancelled() {
        return;
    }

    // Step 2: Agent turn — get response text
    let mut agent = match state.agent_session.create_agent(&config.agent).await {
        Ok(a) => a,
        Err(e) => {
            let _ = outgoing_tx
                .send(OutgoingMessage::Text(VoiceServerMessage::Error {
                    message: format!("agent creation failed: {e}"),
                }))
                .await;
            return;
        }
    };

    let app_config = state.config.read().await;
    if let Some(ref project_name) = config.project
        && let Some(proj) = app_config.find_project(project_name)
    {
        agent.set_active_project(proj.clone());
    }

    if let Some(ref id_str) = config.session_id
        && let Ok(sid) = id_str.parse::<SessionId>()
        && let Ok(Some(persisted)) = state.session_store.load(
            &sid,
            &state.master_key,
            app_config.memory.session_max_age_hours,
        )
    {
        agent.restore_conversation(persisted.messages);
    }
    drop(app_config);

    if let Some(ref mm) = state.memory_manager {
        agent.set_memory_manager(mm.clone());
    }

    // Set up streaming token channel
    let (token_tx, mut token_rx) = mpsc::channel::<String>(64);

    // Sentence buffer for TTS chunking
    let tx_text = outgoing_tx.clone();
    let cancel_tts = cancel.clone();
    let voice = config.voice.clone();
    let speech_config_clone = speech_config.clone();
    let state_tts = state.clone();

    // Forward tokens to client and accumulate sentences for TTS
    let tts_handle = tokio::spawn(async move {
        let mut sentence_buffer = String::new();
        let mut sentences: Vec<String> = Vec::new();

        while let Some(token) = token_rx.recv().await {
            if cancel_tts.is_cancelled() {
                break;
            }

            // Send text token to client
            let _ = tx_text
                .send(OutgoingMessage::Text(VoiceServerMessage::AgentText {
                    content: token.clone(),
                }))
                .await;

            sentence_buffer.push_str(&token);

            // Check for sentence boundaries
            if let Some(boundary) = find_sentence_boundary(&sentence_buffer) {
                let sentence = sentence_buffer[..boundary].trim().to_string();
                sentence_buffer = sentence_buffer[boundary..].to_string();
                if !sentence.is_empty() {
                    sentences.push(sentence);
                }
            }
        }

        // Flush remaining text
        let remaining = sentence_buffer.trim().to_string();
        if !remaining.is_empty() {
            sentences.push(remaining);
        }

        // Now synthesize all sentences as TTS
        if cancel_tts.is_cancelled() || sentences.is_empty() {
            return;
        }

        // Try to create TTS provider
        let tts_config = match speech_config_clone.tts.as_ref() {
            Some(c) => c,
            None => return, // TTS not configured, skip audio
        };

        let store = match aivyx_crypto::EncryptedStore::open(state_tts.dirs.store_path()) {
            Ok(s) => s,
            Err(_) => return,
        };

        let tts_provider = match create_tts_provider(tts_config, &store, &state_tts.master_key) {
            Ok(p) => p,
            Err(_) => return,
        };

        let _ = tx_text
            .send(OutgoingMessage::Text(VoiceServerMessage::Speaking))
            .await;

        let tts_options = TtsOptions {
            voice,
            speed: tts_config.speed,
            format: TtsAudioFormat::Mp3,
        };

        for sentence in &sentences {
            if cancel_tts.is_cancelled() {
                break;
            }

            match tts_provider.synthesize(sentence, &tts_options).await {
                Ok(output) => {
                    let _ = tx_text.send(OutgoingMessage::Binary(output.audio)).await;
                }
                Err(e) => {
                    tracing::warn!("TTS synthesis failed for sentence: {e}");
                }
            }
        }
    });

    // Run the agent turn
    let result = agent
        .turn_stream(&stt_result.text, None, token_tx, Some(cancel.clone()))
        .await;

    // Wait for TTS to finish
    let _ = tts_handle.await;

    match result {
        Ok(_response) => {
            // Save session
            let persisted = agent.to_persisted_session();
            if let Err(e) = state.session_store.save(&persisted, &state.master_key) {
                tracing::warn!("failed to save voice session: {e}");
            }

            let _ = outgoing_tx
                .send(OutgoingMessage::Text(VoiceServerMessage::Done {
                    session_id: agent.session_id().to_string(),
                    cost_usd: agent.current_cost_usd(),
                }))
                .await;
        }
        Err(e) => {
            if !cancel.is_cancelled() {
                let _ = outgoing_tx
                    .send(OutgoingMessage::Text(VoiceServerMessage::Error {
                        message: e.to_string(),
                    }))
                    .await;
            }
        }
    }
}

/// Find the byte index of a sentence boundary in the buffer.
///
/// A sentence boundary is a period, question mark, or exclamation mark
/// followed by a space or end of string. Returns the index *after* the
/// boundary character (start of next sentence).
fn find_sentence_boundary(text: &str) -> Option<usize> {
    for (i, c) in text.char_indices() {
        if matches!(c, '.' | '?' | '!') {
            let next_idx = i + c.len_utf8();
            // End of string or followed by whitespace
            if next_idx >= text.len() || text[next_idx..].starts_with(char::is_whitespace) {
                return Some(next_idx);
            }
        }
    }
    None
}

/// Authenticate the voice WebSocket connection.
async fn authenticate_voice(
    reader: &mut (impl StreamExt<Item = std::result::Result<Message, axum::Error>> + Unpin),
    outgoing_tx: &mpsc::Sender<OutgoingMessage>,
    state: &AppState,
) -> bool {
    let auth_result = tokio::time::timeout(
        std::time::Duration::from_secs(AUTH_TIMEOUT_SECS),
        reader.next(),
    )
    .await;

    let msg = match auth_result {
        Ok(Some(Ok(Message::Text(text)))) => text,
        _ => {
            let _ = outgoing_tx
                .send(OutgoingMessage::Text(VoiceServerMessage::AuthError {
                    message: "auth timeout or connection error".into(),
                }))
                .await;
            return false;
        }
    };

    let client_msg: VoiceClientMessage = match serde_json::from_str(&msg) {
        Ok(m) => m,
        Err(_) => {
            let _ = outgoing_tx
                .send(OutgoingMessage::Text(VoiceServerMessage::AuthError {
                    message: "invalid auth message format".into(),
                }))
                .await;
            return false;
        }
    };

    let token = match client_msg {
        VoiceClientMessage::Auth { token } => token,
        _ => {
            let _ = outgoing_tx
                .send(OutgoingMessage::Text(VoiceServerMessage::AuthError {
                    message: "first message must be auth".into(),
                }))
                .await;
            return false;
        }
    };

    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let provided_hash: [u8; 32] = hasher.finalize().into();
    let current_hash = *state.bearer_token_hash.read().await;

    if provided_hash.ct_eq(&current_hash).unwrap_u8() == 0 {
        let _ = outgoing_tx
            .send(OutgoingMessage::Text(VoiceServerMessage::AuthError {
                message: "invalid bearer token".into(),
            }))
            .await;
        return false;
    }

    let _ = outgoing_tx
        .send(OutgoingMessage::Text(VoiceServerMessage::AuthOk))
        .await;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_client_auth_deserializes() {
        let json = r#"{"type":"auth","token":"test-token"}"#;
        let msg: VoiceClientMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, VoiceClientMessage::Auth { token } if token == "test-token"));
    }

    #[test]
    fn voice_client_config_deserializes() {
        let json = r#"{"type":"config","agent":"vision","voice":"nova"}"#;
        let msg: VoiceClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            VoiceClientMessage::Config { agent, voice, .. } => {
                assert_eq!(agent, "vision");
                assert_eq!(voice, "nova");
            }
            _ => panic!("expected Config"),
        }
    }

    #[test]
    fn voice_client_config_defaults() {
        let json = r#"{"type":"config"}"#;
        let msg: VoiceClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            VoiceClientMessage::Config { agent, voice, .. } => {
                assert_eq!(agent, "aivyx");
                assert_eq!(voice, "alloy");
            }
            _ => panic!("expected Config"),
        }
    }

    #[test]
    fn voice_client_interrupt_deserializes() {
        let json = r#"{"type":"interrupt"}"#;
        let msg: VoiceClientMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, VoiceClientMessage::Interrupt));
    }

    #[test]
    fn voice_client_end_utterance_deserializes() {
        let json = r#"{"type":"end_utterance"}"#;
        let msg: VoiceClientMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, VoiceClientMessage::EndUtterance));
    }

    #[test]
    fn voice_server_auth_ok_serializes() {
        let msg = VoiceServerMessage::AuthOk;
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "auth_ok");
    }

    #[test]
    fn voice_server_transcript_serializes() {
        let msg = VoiceServerMessage::Transcript {
            text: "Hello world".into(),
            is_final: true,
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "transcript");
        assert_eq!(json["text"], "Hello world");
        assert_eq!(json["is_final"], true);
    }

    #[test]
    fn voice_server_agent_text_serializes() {
        let msg = VoiceServerMessage::AgentText {
            content: "Hi".into(),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "agent_text");
        assert_eq!(json["content"], "Hi");
    }

    #[test]
    fn voice_server_done_serializes() {
        let msg = VoiceServerMessage::Done {
            session_id: "sess-1".into(),
            cost_usd: 0.003,
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "done");
        assert_eq!(json["session_id"], "sess-1");
        assert!(json["cost_usd"].is_f64());
    }

    #[test]
    fn find_sentence_boundary_period() {
        assert_eq!(find_sentence_boundary("Hello world. This"), Some(12));
    }

    #[test]
    fn find_sentence_boundary_question() {
        assert_eq!(find_sentence_boundary("How are you? Fine"), Some(12));
    }

    #[test]
    fn find_sentence_boundary_exclamation() {
        assert_eq!(find_sentence_boundary("Wow! That"), Some(4));
    }

    #[test]
    fn find_sentence_boundary_none() {
        assert_eq!(find_sentence_boundary("Hello world"), None);
    }

    #[test]
    fn find_sentence_boundary_end_of_string() {
        assert_eq!(find_sentence_boundary("Done."), Some(5));
    }

    #[test]
    fn find_sentence_boundary_abbreviation() {
        // "e.g." — first '.' at index 1 is followed by 'g' (not whitespace), skip.
        // Second '.' at index 3 is at end of string → boundary at 4.
        assert_eq!(find_sentence_boundary("e.g."), Some(4));
        // "e.g. something" — the '.' at index 3 is followed by space → boundary
        assert_eq!(find_sentence_boundary("e.g. something"), Some(4));
    }

    mod fuzz {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn fuzz_voice_client_message_parse_never_panics(s in "\\PC*") {
                let _ = serde_json::from_str::<VoiceClientMessage>(&s);
            }

            #[test]
            fn interrupt_message_deserializes(__ in 0u8..10u8) {
                let json = r#"{"type":"interrupt"}"#;
                let msg: VoiceClientMessage = serde_json::from_str(json).unwrap();
                let is_interrupt = matches!(msg, VoiceClientMessage::Interrupt);
                prop_assert!(is_interrupt);
            }

            #[test]
            fn config_has_default_agent_and_voice(
                session_id in proptest::option::of("[a-z0-9-]{8,36}")
            ) {
                let mut json_obj = serde_json::json!({"type": "config"});
                if let Some(sid) = &session_id {
                    json_obj["session_id"] = serde_json::json!(sid);
                }
                let msg: VoiceClientMessage = serde_json::from_value(json_obj).unwrap();
                match msg {
                    VoiceClientMessage::Config { agent, voice, .. } => {
                        prop_assert_eq!(agent, "aivyx");
                        prop_assert_eq!(voice, "alloy");
                    }
                    _ => panic!("expected Config variant"),
                }
            }
        }
    }
}

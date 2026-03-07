//! Telegram Bot API adapter.
//!
//! Uses raw HTTP requests via `reqwest` against the
//! [Telegram Bot API](https://core.telegram.org/bots/api). Only three
//! endpoints are needed: `getMe`, `getUpdates`, and `sendMessage`.
//!
//! No external Telegram SDK dependency is required.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use aivyx_config::AivyxDirs;
use aivyx_config::channel::ChannelConfig;
use aivyx_core::AivyxError;
use aivyx_crypto::{EncryptedStore, MasterKey};
use async_trait::async_trait;
use serde::Deserialize;

use super::manager::{InboundChannel, MessageHandler};

/// Default long-poll timeout for `getUpdates` (seconds).
const DEFAULT_POLL_TIMEOUT: u64 = 30;

/// Default interval between polls when an error occurs (seconds).
const ERROR_RETRY_INTERVAL: u64 = 5;

/// Maximum Telegram message length (characters).
const MAX_TG_MESSAGE_LENGTH: usize = 4000;

// ────────────────────────────────────────────────────────────────────
// Telegram Bot API types (minimal)
// ────────────────────────────────────────────────────────────────────

/// Generic Telegram API response wrapper.
#[derive(Debug, Deserialize)]
struct TgResponse<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
}

/// A single update from `getUpdates`.
#[derive(Debug, Deserialize)]
struct TgUpdate {
    update_id: i64,
    message: Option<TgMessage>,
}

/// A Telegram message.
#[derive(Debug, Deserialize)]
struct TgMessage {
    #[allow(dead_code)]
    message_id: i64,
    from: Option<TgUser>,
    chat: TgChat,
    text: Option<String>,
}

/// A Telegram user.
#[derive(Debug, Deserialize)]
struct TgUser {
    id: i64,
}

/// A Telegram chat.
#[derive(Debug, Deserialize)]
struct TgChat {
    id: i64,
}

/// Telegram bot identity from `getMe`.
#[derive(Debug, Deserialize)]
struct TgBotInfo {
    #[allow(dead_code)]
    id: i64,
    #[allow(dead_code)]
    first_name: String,
    username: Option<String>,
}

// ────────────────────────────────────────────────────────────────────
// TelegramChannel
// ────────────────────────────────────────────────────────────────────

/// Inbound channel adapter for the Telegram Bot API.
///
/// Uses long-polling (`getUpdates`) to receive messages and `sendMessage`
/// to reply. The bot token is read from the encrypted store using the
/// `bot_token_ref` setting.
pub struct TelegramChannel {
    name: String,
    bot_token: String,
    allowed_users: HashSet<String>,
    poll_timeout: u64,
    client: reqwest::Client,
    shutdown: tokio::sync::watch::Sender<bool>,
}

impl TelegramChannel {
    /// Create a new Telegram channel from config.
    ///
    /// Reads the bot token from the encrypted store using the key name
    /// specified in `config.settings["bot_token_ref"]`.
    pub fn new(
        config: &ChannelConfig,
        dirs: &AivyxDirs,
        master_key: &MasterKey,
    ) -> aivyx_core::Result<Self> {
        let token_ref = config
            .settings
            .get("bot_token_ref")
            .ok_or_else(|| {
                AivyxError::Channel("Telegram channel requires 'bot_token_ref' in settings".into())
            })?
            .clone();

        let store = EncryptedStore::open(dirs.store_path())?;
        let token_bytes = store.get(&token_ref, master_key)?.ok_or_else(|| {
            AivyxError::Channel(format!(
                "bot token not found in encrypted store (key: '{token_ref}')"
            ))
        })?;
        let bot_token = String::from_utf8(token_bytes)
            .map_err(|_| AivyxError::Channel("bot token is not valid UTF-8".into()))?;

        let poll_timeout = config
            .settings
            .get("poll_timeout_secs")
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_POLL_TIMEOUT);

        let allowed_users: HashSet<String> = config.allowed_users.iter().cloned().collect();

        let (shutdown_tx, _) = tokio::sync::watch::channel(false);

        Ok(Self {
            name: config.name.clone(),
            bot_token,
            allowed_users,
            poll_timeout,
            client: reqwest::Client::new(),
            shutdown: shutdown_tx,
        })
    }

    /// Call `getMe` to verify the bot token.
    pub async fn verify(&self) -> aivyx_core::Result<String> {
        let url = format!("https://api.telegram.org/bot{}/getMe", self.bot_token);
        let resp: TgResponse<TgBotInfo> = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| AivyxError::Channel(format!("telegram getMe: {e}")))?
            .json()
            .await
            .map_err(|e| AivyxError::Channel(format!("telegram getMe parse: {e}")))?;

        if !resp.ok {
            return Err(AivyxError::Channel(format!(
                "telegram getMe failed: {}",
                resp.description.unwrap_or_default()
            )));
        }

        let bot = resp
            .result
            .ok_or_else(|| AivyxError::Channel("telegram getMe returned no result".into()))?;

        Ok(bot.username.unwrap_or_else(|| "unknown".into()))
    }

    /// Long-poll for updates from Telegram.
    async fn get_updates(&self, offset: i64) -> aivyx_core::Result<Vec<TgUpdate>> {
        let url = format!(
            "https://api.telegram.org/bot{}/getUpdates?offset={}&timeout={}",
            self.bot_token, offset, self.poll_timeout
        );

        let resp: TgResponse<Vec<TgUpdate>> = self
            .client
            .get(&url)
            .timeout(Duration::from_secs(self.poll_timeout + 10))
            .send()
            .await
            .map_err(|e| AivyxError::Channel(format!("telegram poll: {e}")))?
            .json()
            .await
            .map_err(|e| AivyxError::Channel(format!("telegram parse: {e}")))?;

        resp.result.ok_or_else(|| {
            AivyxError::Channel(
                resp.description
                    .unwrap_or_else(|| "unknown telegram error".into()),
            )
        })
    }

    /// Send a message to a Telegram chat.
    async fn send_message(&self, chat_id: i64, text: &str) -> aivyx_core::Result<()> {
        let chunks = chunk_message(text, MAX_TG_MESSAGE_LENGTH);

        for chunk in chunks {
            let url = format!("https://api.telegram.org/bot{}/sendMessage", self.bot_token);
            self.client
                .post(&url)
                .json(&serde_json::json!({
                    "chat_id": chat_id,
                    "text": chunk,
                }))
                .send()
                .await
                .map_err(|e| AivyxError::Channel(format!("telegram send: {e}")))?;
        }

        Ok(())
    }
}

#[async_trait]
impl InboundChannel for TelegramChannel {
    async fn run(&self, handler: Arc<MessageHandler>) -> aivyx_core::Result<()> {
        // Verify bot token on startup
        let bot_username = self.verify().await?;
        tracing::info!(
            "telegram channel '{}' connected as @{bot_username}",
            self.name
        );

        let mut offset: i64 = 0;
        let mut shutdown_rx = self.shutdown.subscribe();

        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    tracing::info!("telegram channel '{}' shutting down", self.name);
                    break;
                }
                result = self.get_updates(offset) => {
                    match result {
                        Ok(updates) => {
                            for update in updates {
                                offset = update.update_id + 1;

                                let Some(msg) = update.message else {
                                    continue;
                                };
                                let Some(ref from) = msg.from else {
                                    continue;
                                };
                                let Some(ref text) = msg.text else {
                                    continue;
                                };

                                let user_id = from.id.to_string();

                                // Skip users not in allowlist
                                if !self.allowed_users.is_empty()
                                    && !self.allowed_users.contains(&user_id)
                                {
                                    tracing::debug!(
                                        "telegram: ignoring message from unlisted user {user_id}"
                                    );
                                    continue;
                                }

                                // Process through message handler
                                match handler.handle_message(&user_id, text).await {
                                    Ok(response) => {
                                        if let Err(e) =
                                            self.send_message(msg.chat.id, &response).await
                                        {
                                            tracing::warn!(
                                                "telegram: failed to send reply: {e}"
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            "telegram: message handling error for user {user_id}: {e}"
                                        );
                                        // Send a generic error message back
                                        let _ = self
                                            .send_message(
                                                msg.chat.id,
                                                "Sorry, I encountered an error processing your message.",
                                            )
                                            .await;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("telegram poll error: {e}");
                            tokio::time::sleep(Duration::from_secs(ERROR_RETRY_INTERVAL)).await;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn stop(&self) {
        let _ = self.shutdown.send(true);
    }

    fn platform(&self) -> &str {
        "telegram"
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// Split a message into chunks that fit within the platform's message limit.
///
/// Breaks at the last newline before the limit, or at a char boundary if
/// no newline is found.
fn chunk_message(text: &str, max_len: usize) -> Vec<&str> {
    if text.len() <= max_len {
        return vec![text];
    }

    let mut chunks = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if remaining.len() <= max_len {
            chunks.push(remaining);
            break;
        }

        // Try to break at a newline
        let boundary = remaining[..max_len]
            .rfind('\n')
            .map(|pos| pos + 1) // include the newline in the current chunk
            .unwrap_or_else(|| remaining.floor_char_boundary(max_len));

        let (chunk, rest) = remaining.split_at(boundary);
        chunks.push(chunk);
        remaining = rest;
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_short_message() {
        let chunks = chunk_message("hello", 100);
        assert_eq!(chunks, vec!["hello"]);
    }

    #[test]
    fn chunk_exact_limit() {
        let msg = "a".repeat(100);
        let chunks = chunk_message(&msg, 100);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 100);
    }

    #[test]
    fn chunk_long_message_at_newline() {
        let msg = format!("{}\n{}", "a".repeat(50), "b".repeat(50));
        let chunks = chunk_message(&msg, 60);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].ends_with('\n'));
        assert!(chunks[1].starts_with('b'));
    }

    #[test]
    fn chunk_long_message_no_newline() {
        let msg = "a".repeat(250);
        let chunks = chunk_message(&msg, 100);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), 100);
        assert_eq!(chunks[1].len(), 100);
        assert_eq!(chunks[2].len(), 50);
    }

    #[test]
    fn deserialize_telegram_update() {
        let json = r#"{
            "ok": true,
            "result": [{
                "update_id": 12345,
                "message": {
                    "message_id": 1,
                    "from": {"id": 67890, "is_bot": false, "first_name": "Test"},
                    "chat": {"id": 67890, "type": "private"},
                    "text": "Hello bot"
                }
            }]
        }"#;

        let resp: TgResponse<Vec<TgUpdate>> = serde_json::from_str(json).unwrap();
        assert!(resp.ok);
        let updates = resp.result.unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].update_id, 12345);
        let msg = updates[0].message.as_ref().unwrap();
        assert_eq!(msg.from.as_ref().unwrap().id, 67890);
        assert_eq!(msg.chat.id, 67890);
        assert_eq!(msg.text.as_deref(), Some("Hello bot"));
    }

    #[test]
    fn deserialize_telegram_get_me() {
        let json = r#"{
            "ok": true,
            "result": {
                "id": 12345,
                "is_bot": true,
                "first_name": "AivyxBot",
                "username": "aivyx_bot"
            }
        }"#;

        let resp: TgResponse<TgBotInfo> = serde_json::from_str(json).unwrap();
        assert!(resp.ok);
        let bot = resp.result.unwrap();
        assert_eq!(bot.username.as_deref(), Some("aivyx_bot"));
    }

    #[test]
    fn deserialize_telegram_error_response() {
        let json = r#"{
            "ok": false,
            "description": "Unauthorized"
        }"#;

        let resp: TgResponse<Vec<TgUpdate>> = serde_json::from_str(json).unwrap();
        assert!(!resp.ok);
        assert_eq!(resp.description.as_deref(), Some("Unauthorized"));
        assert!(resp.result.is_none());
    }
}

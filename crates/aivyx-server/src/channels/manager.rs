//! Channel manager runtime.
//!
//! [`ChannelManager`] manages the lifecycle of inbound communication channels.
//! Each enabled channel runs as a background task that receives messages from
//! an external platform and routes them through the agent turn loop via
//! [`MessageHandler`].

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aivyx_audit::AuditEvent;
use aivyx_config::channel::ChannelConfig;
use aivyx_core::{AivyxError, AutonomyTier};
use async_trait::async_trait;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::app_state::AppState;
use crate::channels::session::derive_channel_session_id;

/// Maximum messages per user per minute before rate limiting kicks in.
const MAX_MESSAGES_PER_MINUTE: usize = 10;

/// Maximum input message length (characters) before truncation.
const MAX_MESSAGE_LENGTH: usize = 8000;

// ────────────────────────────────────────────────────────────────────
// InboundChannel trait
// ────────────────────────────────────────────────────────────────────

/// A platform adapter that receives messages from an external service
/// and sends responses back.
///
/// Each implementation handles one platform's protocol (long-polling,
/// WebSocket, IMAP, etc.) and delegates message processing to
/// [`MessageHandler::handle_message`].
#[async_trait]
pub trait InboundChannel: Send + Sync {
    /// Start the channel's message loop.
    ///
    /// This method runs until the channel is shut down (via `stop()`) or
    /// encounters a fatal error. Non-fatal errors (network blips, rate
    /// limits) should be logged and retried internally.
    async fn run(&self, handler: Arc<MessageHandler>) -> aivyx_core::Result<()>;

    /// Gracefully shut down the channel.
    async fn stop(&self);

    /// Platform identifier (e.g., `"telegram"`, `"email"`).
    fn platform(&self) -> &str;

    /// Channel configuration name.
    fn name(&self) -> &str;
}

// ────────────────────────────────────────────────────────────────────
// MessageHandler
// ────────────────────────────────────────────────────────────────────

/// Processes incoming messages through the agent turn loop.
///
/// This is the shared core that all platform adapters call. It handles:
/// 1. User allowlist validation
/// 2. Per-user rate limiting
/// 3. Special commands (`/new`, `/status`, `/help`)
/// 4. Deterministic session derivation
/// 5. Agent creation, session restore, memory wiring
/// 6. Agent turn execution
/// 7. Session persistence
/// 8. Audit logging
pub struct MessageHandler {
    state: Arc<AppState>,
    config: ChannelConfig,
    rate_limiter: Mutex<HashMap<String, Vec<Instant>>>,
}

/// Result of processing a special command (handled before agent turn).
enum SpecialCommand {
    /// The message was a special command and the response is ready.
    Handled(String),
    /// The message is not a special command; proceed with agent turn.
    NotSpecial,
}

impl MessageHandler {
    /// Create a new message handler for a channel.
    pub fn new(state: Arc<AppState>, config: ChannelConfig) -> Self {
        Self {
            state,
            config,
            rate_limiter: Mutex::new(HashMap::new()),
        }
    }

    /// Process an incoming message from a platform user.
    ///
    /// Returns the response text to send back, or an error if processing
    /// failed.
    pub async fn handle_message(&self, user_id: &str, text: &str) -> aivyx_core::Result<String> {
        // 1. Allowlist check (empty = deny all)
        if !self.is_user_allowed(user_id) {
            return Err(AivyxError::Channel(format!(
                "user '{user_id}' not in allowed_users for channel '{}'",
                self.config.name
            )));
        }

        // 2. Rate limit check
        if self.is_rate_limited(user_id).await {
            return Ok("You're sending messages too quickly. Please wait a moment.".into());
        }

        // 3. Audit: message received
        let _ = self
            .state
            .audit_log
            .append(AuditEvent::ChannelMessageReceived {
                channel_name: self.config.name.clone(),
                platform: self.config.platform.to_string(),
                user_id: user_id.to_string(),
            });

        // 4. Handle special commands
        let trimmed = text.trim();
        match self.handle_special_command(trimmed, user_id) {
            SpecialCommand::Handled(response) => return Ok(response),
            SpecialCommand::NotSpecial => {}
        }

        // 5. Truncate long messages
        let message = if trimmed.len() > MAX_MESSAGE_LENGTH {
            let boundary = trimmed.floor_char_boundary(MAX_MESSAGE_LENGTH);
            &trimmed[..boundary]
        } else {
            trimmed
        };

        // 6. Derive deterministic session ID
        let session_id = derive_channel_session_id(&self.config.platform.to_string(), user_id);

        // 7. Create agent
        let mut agent = self
            .state
            .agent_session
            .create_agent(&self.config.agent)
            .await
            .map_err(|e| {
                AivyxError::Channel(format!(
                    "failed to create agent '{}': {e}",
                    self.config.agent
                ))
            })?;

        // 8. Reject non-autonomous (Leash/Locked)
        match agent.autonomy_tier() {
            AutonomyTier::Locked | AutonomyTier::Leash => {
                return Err(AivyxError::Channel(format!(
                    "agent '{}' requires interactive approval (tier: {:?}), \
                     which is not supported via channels",
                    self.config.agent,
                    agent.autonomy_tier()
                )));
            }
            AutonomyTier::Trust | AutonomyTier::Free => {}
        }

        // 9. Restore session
        if let Some(persisted) = self.state.session_store.load(
            &session_id,
            &self.state.master_key,
            self.state.config.memory.session_max_age_hours,
        )? {
            agent.restore_conversation(persisted.messages);
        }

        // 10. Wire memory
        if let Some(ref mm) = self.state.memory_manager {
            agent.set_memory_manager(mm.clone());
        }

        // 11. Run turn
        let response = agent.turn(message, None).await?;

        // 12. Save session (use the deterministic ID, not the agent's random one)
        let mut persisted = agent.to_persisted_session();
        persisted.metadata.session_id = session_id;
        if let Err(e) = self
            .state
            .session_store
            .save(&persisted, &self.state.master_key)
        {
            tracing::warn!(
                "failed to save channel session for {}/{}: {e}",
                self.config.name,
                user_id
            );
        }

        // 13. Audit: response sent
        let _ = self.state.audit_log.append(AuditEvent::ChannelMessageSent {
            channel_name: self.config.name.clone(),
            platform: self.config.platform.to_string(),
            user_id: user_id.to_string(),
        });

        Ok(response)
    }

    /// Check if a user ID is in the allowed_users list.
    fn is_user_allowed(&self, user_id: &str) -> bool {
        // Empty list = deny all (fail closed)
        !self.config.allowed_users.is_empty()
            && self.config.allowed_users.iter().any(|u| u == user_id)
    }

    /// Check if a user has exceeded the rate limit.
    async fn is_rate_limited(&self, user_id: &str) -> bool {
        let mut limiter = self.rate_limiter.lock().await;
        let now = Instant::now();
        let window = Duration::from_secs(60);

        let timestamps = limiter.entry(user_id.to_string()).or_default();

        // Remove expired entries
        timestamps.retain(|t| now.duration_since(*t) < window);

        if timestamps.len() >= MAX_MESSAGES_PER_MINUTE {
            return true;
        }

        timestamps.push(now);
        false
    }

    /// Handle special commands that bypass the agent turn loop.
    fn handle_special_command(&self, text: &str, user_id: &str) -> SpecialCommand {
        match text {
            "/new" | "/reset" => {
                // Delete the session for this user
                let session_id =
                    derive_channel_session_id(&self.config.platform.to_string(), user_id);
                if let Err(e) = self.state.session_store.delete(&session_id) {
                    tracing::warn!("failed to delete session on /new: {e}");
                }
                SpecialCommand::Handled("Session cleared. Starting fresh conversation.".into())
            }
            "/status" => {
                let info = format!(
                    "Channel: {}\nPlatform: {}\nAgent: {}\nUser: {}",
                    self.config.name, self.config.platform, self.config.agent, user_id,
                );
                SpecialCommand::Handled(info)
            }
            "/help" => SpecialCommand::Handled(
                "Available commands:\n\
                     /new — Start a new conversation\n\
                     /status — Show channel info\n\
                     /help — Show this help message"
                    .into(),
            ),
            _ => SpecialCommand::NotSpecial,
        }
    }

    /// Get the channel config name.
    pub fn channel_name(&self) -> &str {
        &self.config.name
    }
}

// ────────────────────────────────────────────────────────────────────
// ChannelManager
// ────────────────────────────────────────────────────────────────────

/// Manages the lifecycle of all inbound communication channels.
///
/// Spawns each enabled channel as a background tokio task, tracks handles,
/// and provides graceful shutdown.
pub struct ChannelManager {
    state: Arc<AppState>,
    handles: Vec<(String, JoinHandle<()>)>,
}

impl ChannelManager {
    /// Create a new channel manager.
    pub fn new(state: Arc<AppState>) -> Self {
        Self {
            state,
            handles: Vec::new(),
        }
    }

    /// Start all enabled channels from the current config.
    pub async fn start_all(&mut self) -> aivyx_core::Result<()> {
        let config = aivyx_config::AivyxConfig::load(self.state.dirs.config_path())?;
        let enabled: Vec<_> = config
            .channels
            .iter()
            .filter(|c| c.enabled)
            .cloned()
            .collect();

        if enabled.is_empty() {
            tracing::info!("no enabled channels configured");
            return Ok(());
        }

        tracing::info!("starting {} channel(s)", enabled.len());

        for channel_config in enabled {
            if let Err(e) = self.start_channel(channel_config).await {
                tracing::error!("failed to start channel: {e}");
            }
        }

        Ok(())
    }

    /// Start a single channel.
    async fn start_channel(&mut self, config: ChannelConfig) -> aivyx_core::Result<()> {
        let name = config.name.clone();
        let platform = config.platform.to_string();

        let channel: Box<dyn InboundChannel> = match config.platform {
            #[cfg(feature = "telegram")]
            aivyx_config::ChannelPlatform::Telegram => {
                let tg = super::telegram::TelegramChannel::new(
                    &config,
                    &self.state.dirs,
                    &self.state.master_key,
                )?;
                Box::new(tg)
            }
            _ => {
                return Err(AivyxError::Channel(format!(
                    "platform '{}' is not supported (check feature flags)",
                    config.platform
                )));
            }
        };

        let handler = Arc::new(MessageHandler::new(self.state.clone(), config));
        let state = self.state.clone();

        // Audit: channel started
        let _ = self.state.audit_log.append(AuditEvent::ChannelStarted {
            channel_name: name.clone(),
            platform: platform.clone(),
        });

        let channel_name = name.clone();
        let channel_platform = platform.clone();

        let handle = tokio::spawn(async move {
            tracing::info!("channel '{channel_name}' ({channel_platform}) started");
            if let Err(e) = channel.run(handler).await {
                tracing::error!("channel '{channel_name}' error: {e}");
                let _ = state.audit_log.append(AuditEvent::ChannelStopped {
                    channel_name: channel_name.clone(),
                    platform: channel_platform,
                    reason: e.to_string(),
                });
            }
        });

        self.handles.push((name, handle));
        Ok(())
    }

    /// Stop all running channels.
    pub async fn stop_all(&mut self) {
        for (name, handle) in self.handles.drain(..) {
            tracing::info!("stopping channel '{name}'");
            handle.abort();
        }
    }

    /// Check if a channel is currently running.
    pub fn is_running(&self, name: &str) -> bool {
        self.handles
            .iter()
            .any(|(n, h)| n == name && !h.is_finished())
    }

    /// List running channel names.
    pub fn running_channels(&self) -> Vec<&str> {
        self.handles
            .iter()
            .filter(|(_, h)| !h.is_finished())
            .map(|(n, _)| n.as_str())
            .collect()
    }
}

/// Spawn the channel manager as a background task.
///
/// Reads channel configs, starts enabled channels, and keeps running
/// until the server shuts down. Errors in individual channels are logged
/// but do not crash the server.
pub fn spawn_channel_manager(state: Arc<AppState>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut manager = ChannelManager::new(state);
        if let Err(e) = manager.start_all().await {
            tracing::error!("channel manager startup failed: {e}");
        }

        // Keep alive — channels run in their own spawned tasks.
        // Wait for shutdown signal.
        tokio::signal::ctrl_c().await.ok();
        manager.stop_all().await;
        tracing::info!("channel manager stopped");
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn special_command_new() {
        let state = test_handler_state();
        let handler = MessageHandler::new(state, test_config());

        match handler.handle_special_command("/new", "user1") {
            SpecialCommand::Handled(msg) => assert!(msg.contains("cleared")),
            SpecialCommand::NotSpecial => panic!("should handle /new"),
        }
    }

    #[test]
    fn special_command_reset() {
        let state = test_handler_state();
        let handler = MessageHandler::new(state, test_config());

        match handler.handle_special_command("/reset", "user1") {
            SpecialCommand::Handled(msg) => assert!(msg.contains("cleared")),
            SpecialCommand::NotSpecial => panic!("should handle /reset"),
        }
    }

    #[test]
    fn special_command_status() {
        let state = test_handler_state();
        let handler = MessageHandler::new(state, test_config());

        match handler.handle_special_command("/status", "user1") {
            SpecialCommand::Handled(msg) => {
                assert!(msg.contains("tg-test"));
                assert!(msg.contains("assistant"));
            }
            SpecialCommand::NotSpecial => panic!("should handle /status"),
        }
    }

    #[test]
    fn special_command_help() {
        let state = test_handler_state();
        let handler = MessageHandler::new(state, test_config());

        match handler.handle_special_command("/help", "user1") {
            SpecialCommand::Handled(msg) => assert!(msg.contains("/new")),
            SpecialCommand::NotSpecial => panic!("should handle /help"),
        }
    }

    #[test]
    fn special_command_not_special() {
        let state = test_handler_state();
        let handler = MessageHandler::new(state, test_config());

        match handler.handle_special_command("Hello world", "user1") {
            SpecialCommand::Handled(_) => panic!("regular message should not be special"),
            SpecialCommand::NotSpecial => {} // expected
        }
    }

    #[test]
    fn allowed_users_empty_denies_all() {
        let state = test_handler_state();
        let mut config = test_config();
        config.allowed_users = vec![];
        let handler = MessageHandler::new(state, config);

        assert!(!handler.is_user_allowed("anyone"));
    }

    #[test]
    fn allowed_users_list_filters() {
        let state = test_handler_state();
        let mut config = test_config();
        config.allowed_users = vec!["123".into(), "456".into()];
        let handler = MessageHandler::new(state, config);

        assert!(handler.is_user_allowed("123"));
        assert!(handler.is_user_allowed("456"));
        assert!(!handler.is_user_allowed("789"));
    }

    #[tokio::test]
    async fn rate_limiter_allows_within_limit() {
        let state = test_handler_state();
        let handler = MessageHandler::new(state, test_config());

        for _ in 0..MAX_MESSAGES_PER_MINUTE {
            assert!(!handler.is_rate_limited("user1").await);
        }
    }

    #[tokio::test]
    async fn rate_limiter_blocks_over_limit() {
        let state = test_handler_state();
        let handler = MessageHandler::new(state, test_config());

        for _ in 0..MAX_MESSAGES_PER_MINUTE {
            handler.is_rate_limited("user1").await;
        }
        assert!(handler.is_rate_limited("user1").await);
    }

    #[tokio::test]
    async fn rate_limiter_per_user() {
        let state = test_handler_state();
        let handler = MessageHandler::new(state, test_config());

        for _ in 0..MAX_MESSAGES_PER_MINUTE {
            handler.is_rate_limited("user1").await;
        }
        // user1 is now rate limited, but user2 should be fine
        assert!(!handler.is_rate_limited("user2").await);
    }

    // ── Test helpers ─────────────────────────────────────────────

    fn test_config() -> ChannelConfig {
        use aivyx_config::ChannelPlatform;
        let mut config = ChannelConfig::new("tg-test", ChannelPlatform::Telegram, "assistant");
        config.allowed_users = vec!["123456".into()];
        config
    }

    /// Create a minimal AppState for unit tests.
    ///
    /// Uses the `#[cfg(test)]` build_app_state helper that creates an
    /// ephemeral state with no real LLM or crypto.
    fn test_handler_state() -> Arc<AppState> {
        // We need a minimal AppState — construct one using the test helper
        // from startup.rs. If that's not available, we create a bare minimum.
        use aivyx_agent::SessionStore;
        use aivyx_audit::AuditLog;
        use aivyx_config::{AivyxConfig, AivyxDirs};
        use aivyx_crypto::MasterKey;

        let dir = std::env::temp_dir().join(format!("aivyx-chan-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("agents")).ok();
        std::fs::create_dir_all(dir.join("keys")).ok();

        let dirs = AivyxDirs::new(&dir);
        let config = AivyxConfig::default();
        let master_key = MasterKey::from_bytes([42u8; 32]);
        let audit_key = aivyx_crypto::derive_audit_key(&MasterKey::from_bytes([42u8; 32]));

        let agent_session = Arc::new(aivyx_agent::AgentSession::new(
            dirs.clone(),
            config.clone(),
            MasterKey::from_bytes([42u8; 32]),
        ));

        let session_store =
            SessionStore::open(dir.join("sessions.db")).expect("session store open");
        let audit_log = AuditLog::new(dir.join("audit.log"), &audit_key);

        let bearer_hash = [0u8; 32];

        Arc::new(AppState {
            agent_session,
            session_store,
            memory_manager: None,
            audit_log,
            master_key,
            dirs,
            config,
            bearer_token_hash: tokio::sync::RwLock::new(bearer_hash),
            auth_rate_limiter: std::sync::Mutex::new(HashMap::new()),
            sidecar_mode: false,
            endpoint_rate_limiters: None,
            federation: None,
        })
    }
}

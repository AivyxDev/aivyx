//! Channel management endpoints.
//!
//! `GET /channels` — list all configured channels.
//! `POST /channels` — create a new channel.
//! `GET /channels/{name}` — get channel details.
//! `PUT /channels/{name}` — update a channel.
//! `DELETE /channels/{name}` — remove a channel.

use std::collections::HashMap;
use std::sync::Arc;

use aivyx_config::{ChannelConfig, ChannelPlatform};
use aivyx_core::AivyxError;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::app_state::AppState;
use crate::error::ServerError;
use crate::validation::validate_name;

/// Summary item for channel listing.
#[derive(Debug, Serialize)]
pub struct ChannelSummary {
    /// Unique channel name.
    pub name: String,
    /// Messaging platform (e.g., `"telegram"`, `"email"`).
    pub platform: String,
    /// Agent profile that handles incoming messages.
    pub agent: String,
    /// Whether the channel is currently active.
    pub enabled: bool,
    /// Allowed user identifiers for this channel.
    pub allowed_users: Vec<String>,
}

/// Request body for `POST /channels`.
#[derive(Debug, Deserialize)]
pub struct CreateChannelRequest {
    /// Channel name (slug-style).
    pub name: String,
    /// Platform name: `"Telegram"`, `"Email"`, `"Discord"`, `"Slack"`, or `"Matrix"`.
    pub platform: String,
    /// Agent profile to handle messages.
    pub agent: String,
    /// Allowed user identifiers.
    pub allowed_users: Vec<String>,
    /// Platform-specific key-value settings.
    #[serde(default)]
    pub settings: HashMap<String, String>,
}

/// Request body for `PUT /channels/{name}`.
#[derive(Debug, Deserialize)]
pub struct UpdateChannelRequest {
    /// New agent profile (if changing).
    pub agent: Option<String>,
    /// Enable or disable the channel.
    pub enabled: Option<bool>,
    /// Replace the allowed users list.
    pub allowed_users: Option<Vec<String>>,
    /// Replace the settings map.
    pub settings: Option<HashMap<String, String>>,
}

/// `GET /channels` -- list all configured channels.
pub async fn list_channels(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ServerError> {
    let config = aivyx_config::AivyxConfig::load(state.dirs.config_path())?;

    let channels: Vec<ChannelSummary> = config
        .channels
        .iter()
        .map(|c| ChannelSummary {
            name: c.name.clone(),
            platform: c.platform.to_string(),
            agent: c.agent.clone(),
            enabled: c.enabled,
            allowed_users: c.allowed_users.clone(),
        })
        .collect();

    Ok(axum::Json(channels))
}

/// `POST /channels` -- create a new channel.
pub async fn create_channel(
    State(state): State<Arc<AppState>>,
    axum::Json(req): axum::Json<CreateChannelRequest>,
) -> Result<impl IntoResponse, ServerError> {
    validate_name(&req.name)?;

    let platform = match req.platform.to_lowercase().as_str() {
        "telegram" => ChannelPlatform::Telegram,
        "email" => ChannelPlatform::Email,
        "discord" => ChannelPlatform::Discord,
        "slack" => ChannelPlatform::Slack,
        "matrix" => ChannelPlatform::Matrix,
        other => {
            return Err(ServerError(AivyxError::Config(format!(
                "unknown platform: {other}"
            ))));
        }
    };

    let mut channel = ChannelConfig::new(&req.name, platform, &req.agent);
    channel.allowed_users = req.allowed_users.clone();
    channel.settings = req.settings.clone();
    channel.created_at = Utc::now();

    let mut config = aivyx_config::AivyxConfig::load(state.dirs.config_path())?;
    config.add_channel(channel)?;
    config.save(state.dirs.config_path())?;

    let summary = ChannelSummary {
        name: req.name,
        platform: platform.to_string(),
        agent: req.agent,
        enabled: true,
        allowed_users: req.allowed_users,
    };
    Ok((axum::http::StatusCode::CREATED, axum::Json(summary)))
}

/// `GET /channels/{name}` -- get channel details.
pub async fn get_channel(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
    validate_name(&name)?;

    let config = aivyx_config::AivyxConfig::load(state.dirs.config_path())?;
    let channel = config
        .find_channel(&name)
        .ok_or_else(|| ServerError(AivyxError::Config(format!("channel not found: {name}"))))?;

    let summary = ChannelSummary {
        name: channel.name.clone(),
        platform: channel.platform.to_string(),
        agent: channel.agent.clone(),
        enabled: channel.enabled,
        allowed_users: channel.allowed_users.clone(),
    };
    Ok(axum::Json(summary))
}

/// `PUT /channels/{name}` -- update an existing channel.
pub async fn update_channel(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    axum::Json(req): axum::Json<UpdateChannelRequest>,
) -> Result<impl IntoResponse, ServerError> {
    validate_name(&name)?;

    let mut config = aivyx_config::AivyxConfig::load(state.dirs.config_path())?;
    let channel = config
        .find_channel_mut(&name)
        .ok_or_else(|| ServerError(AivyxError::Config(format!("channel not found: {name}"))))?;

    if let Some(agent) = req.agent {
        channel.agent = agent;
    }
    if let Some(enabled) = req.enabled {
        channel.enabled = enabled;
    }
    if let Some(allowed_users) = req.allowed_users {
        channel.allowed_users = allowed_users;
    }
    if let Some(settings) = req.settings {
        channel.settings = settings;
    }

    let summary = ChannelSummary {
        name: channel.name.clone(),
        platform: channel.platform.to_string(),
        agent: channel.agent.clone(),
        enabled: channel.enabled,
        allowed_users: channel.allowed_users.clone(),
    };

    config.save(state.dirs.config_path())?;

    Ok(axum::Json(summary))
}

/// `DELETE /channels/{name}` -- remove a channel.
pub async fn delete_channel(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
    validate_name(&name)?;

    let mut config = aivyx_config::AivyxConfig::load(state.dirs.config_path())?;
    config.remove_channel(&name)?;
    config.save(state.dirs.config_path())?;

    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_summary_serializes() {
        let summary = ChannelSummary {
            name: "tg-personal".into(),
            platform: "telegram".into(),
            agent: "assistant".into(),
            enabled: true,
            allowed_users: vec!["123456".into()],
        };
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["name"], "tg-personal");
        assert_eq!(json["platform"], "telegram");
        assert_eq!(json["agent"], "assistant");
        assert!(json["enabled"].as_bool().unwrap());
        assert_eq!(json["allowed_users"][0], "123456");
    }

    #[test]
    fn create_channel_request_deserializes() {
        let json = r#"{
            "name": "tg-bot",
            "platform": "Telegram",
            "agent": "assistant",
            "allowed_users": ["123456"]
        }"#;
        let req: CreateChannelRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "tg-bot");
        assert_eq!(req.platform, "Telegram");
        assert_eq!(req.agent, "assistant");
        assert_eq!(req.allowed_users, vec!["123456"]);
        assert!(req.settings.is_empty()); // default
    }

    #[test]
    fn create_channel_request_with_settings() {
        let json = r#"{
            "name": "email-work",
            "platform": "Email",
            "agent": "assistant",
            "allowed_users": ["user@example.com"],
            "settings": {"imap_host": "imap.example.com"}
        }"#;
        let req: CreateChannelRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.settings.get("imap_host").unwrap(), "imap.example.com");
    }

    #[test]
    fn create_channel_request_missing_required_field() {
        // Missing "agent" field should fail
        let json = r#"{"name":"test","platform":"Telegram","allowed_users":[]}"#;
        let result = serde_json::from_str::<CreateChannelRequest>(json);
        assert!(result.is_err());
    }

    #[test]
    fn update_channel_request_all_none() {
        let json = r#"{}"#;
        let req: UpdateChannelRequest = serde_json::from_str(json).unwrap();
        assert!(req.agent.is_none());
        assert!(req.enabled.is_none());
        assert!(req.allowed_users.is_none());
        assert!(req.settings.is_none());
    }

    #[test]
    fn update_channel_request_partial() {
        let json = r#"{"enabled": false, "agent": "researcher"}"#;
        let req: UpdateChannelRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.agent.as_deref(), Some("researcher"));
        assert_eq!(req.enabled, Some(false));
        assert!(req.allowed_users.is_none());
        assert!(req.settings.is_none());
    }

    #[test]
    fn channel_summary_disabled() {
        let summary = ChannelSummary {
            name: "slack-team".into(),
            platform: "slack".into(),
            agent: "ops".into(),
            enabled: false,
            allowed_users: vec![],
        };
        let json = serde_json::to_value(&summary).unwrap();
        assert!(!json["enabled"].as_bool().unwrap());
        assert!(json["allowed_users"].as_array().unwrap().is_empty());
    }
}

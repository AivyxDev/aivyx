//! System configuration management endpoints.
//!
//! `GET /config` — return the current system configuration.
//! `PATCH /config` — partially update configuration fields.

use std::sync::Arc;

use aivyx_config::{AivyxConfig, AutonomyPolicy, EmbeddingConfig, MemoryConfig, ServerConfig};
use axum::extract::State;
use axum::response::IntoResponse;
use serde::Deserialize;

use aivyx_core::AivyxError;

use crate::app_state::AppState;
use crate::error::ServerError;

/// Request body for `PATCH /config`.
///
/// Only non-null fields are applied. Provider and schedule changes are
/// excluded — providers require secret management and schedules have
/// dedicated endpoints.
#[derive(Debug, Deserialize)]
pub struct PatchConfigRequest {
    /// Agent autonomy constraints and rate limits.
    #[serde(default)]
    pub autonomy: Option<AutonomyPolicy>,
    /// Embedding provider configuration.
    #[serde(default)]
    pub embedding: Option<EmbeddingConfig>,
    /// Memory subsystem configuration.
    #[serde(default)]
    pub memory: Option<MemoryConfig>,
    /// HTTP server configuration.
    #[serde(default)]
    pub server: Option<ServerConfig>,
}

/// `GET /config` — return the current system configuration.
pub async fn get_config(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ServerError> {
    let config = AivyxConfig::load(state.dirs.config_path())?;
    Ok(axum::Json(config))
}

/// `PATCH /config` — partially update configuration fields.
///
/// Loads the current config from disk, merges the provided fields,
/// saves back, and returns the updated config.
pub async fn patch_config(
    State(state): State<Arc<AppState>>,
    axum::Json(req): axum::Json<PatchConfigRequest>,
) -> Result<impl IntoResponse, ServerError> {
    let mut config = AivyxConfig::load(state.dirs.config_path())?;

    if let Some(autonomy) = req.autonomy {
        config.autonomy = autonomy;
    }
    if let Some(embedding) = req.embedding {
        config.embedding = Some(embedding);
    }
    if let Some(memory) = req.memory {
        config.memory = memory;
    }
    if let Some(ref server) = req.server {
        // M8: Reject wildcard bind addresses — exposing the server to all
        // interfaces is dangerous without explicit intent.
        if server.bind_address == "0.0.0.0" || server.bind_address == "::" {
            return Err(ServerError(AivyxError::Config(format!(
                "bind_address '{}' would expose the server on all interfaces; use '127.0.0.1' or '::1' instead",
                server.bind_address
            ))));
        }
    }
    if let Some(server) = req.server {
        config.server = Some(server);
    }

    config.save(state.dirs.config_path())?;
    Ok(axum::Json(config))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_config_request_empty() {
        let json = r#"{}"#;
        let req: PatchConfigRequest = serde_json::from_str(json).unwrap();
        assert!(req.autonomy.is_none());
        assert!(req.embedding.is_none());
        assert!(req.memory.is_none());
        assert!(req.server.is_none());
    }

    #[test]
    fn patch_config_request_partial() {
        let json = r#"{"memory":{"max_memories":500}}"#;
        let req: PatchConfigRequest = serde_json::from_str(json).unwrap();
        assert!(req.memory.is_some());
        assert_eq!(req.memory.unwrap().max_memories, 500);
        assert!(req.autonomy.is_none());
    }

    #[test]
    fn patch_config_request_autonomy() {
        let json = r#"{"autonomy":{
            "default_tier":"Trust",
            "max_tool_calls_per_minute": 120,
            "max_cost_per_session_usd": 10.0,
            "require_approval_for_destructive": false,
            "max_retries": 5,
            "retry_base_delay_ms": 2000
        }}"#;
        let req: PatchConfigRequest = serde_json::from_str(json).unwrap();
        let autonomy = req.autonomy.unwrap();
        assert_eq!(autonomy.max_retries, 5);
        assert_eq!(autonomy.max_tool_calls_per_minute, 120);
    }

    #[test]
    fn patch_config_request_server_only() {
        let json = r#"{"server":{"bind_address":"127.0.0.1","port":8080,"cors_origins":[]}}"#;
        let req: PatchConfigRequest = serde_json::from_str(json).unwrap();
        assert!(req.server.is_some());
        let server = req.server.unwrap();
        assert_eq!(server.bind_address, "127.0.0.1");
        assert_eq!(server.port, 8080);
        assert!(req.autonomy.is_none());
        assert!(req.embedding.is_none());
        assert!(req.memory.is_none());
    }

    #[test]
    fn patch_config_request_all_fields() {
        let json = r#"{
            "autonomy": {
                "default_tier": "Free",
                "max_tool_calls_per_minute": 60,
                "max_cost_per_session_usd": 5.0,
                "require_approval_for_destructive": true,
                "max_retries": 3,
                "retry_base_delay_ms": 1000
            },
            "memory": {"max_memories": 2000},
            "server": {"bind_address":"127.0.0.1","port":3000,"cors_origins":[]}
        }"#;
        let req: PatchConfigRequest = serde_json::from_str(json).unwrap();
        assert!(req.autonomy.is_some());
        assert!(req.memory.is_some());
        assert!(req.server.is_some());
    }

    #[test]
    fn patch_config_request_ignores_unknown_fields() {
        // serde default behavior: unknown fields are ignored with deny_unknown_fields absent
        let json = r#"{"unknown_field": 42}"#;
        let req: PatchConfigRequest = serde_json::from_str(json).unwrap();
        assert!(req.autonomy.is_none());
        assert!(req.embedding.is_none());
        assert!(req.memory.is_none());
        assert!(req.server.is_none());
    }
}

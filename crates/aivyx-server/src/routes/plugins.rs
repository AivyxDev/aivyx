//! Plugin management endpoints.
//!
//! `GET /plugins` — list installed plugins.
//! `POST /plugins` — install a new plugin.
//! `DELETE /plugins/{name}` — remove an installed plugin.

use std::sync::Arc;

use std::collections::HashMap;

use aivyx_audit::AuditEvent;
use aivyx_config::mcp::{McpServerConfig, McpTransport};
use aivyx_config::plugin::{PluginEntry, PluginSource};
use aivyx_core::AivyxError;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::app_state::AppState;
use crate::error::ServerError;
use crate::extractors::AuthContextExt;
use crate::validation::validate_name;
use aivyx_tenant::AivyxRole;

/// Response item for plugin listing.
#[derive(Debug, Serialize)]
pub struct PluginSummary {
    /// Plugin name.
    pub name: String,
    /// Plugin version string.
    pub version: String,
    /// Human-readable description.
    pub description: String,
    /// Whether the plugin is enabled.
    pub enabled: bool,
    /// Plugin author (if known).
    pub author: Option<String>,
}

impl From<&PluginEntry> for PluginSummary {
    fn from(entry: &PluginEntry) -> Self {
        Self {
            name: entry.name.clone(),
            version: entry.version.clone(),
            description: entry.description.clone(),
            enabled: entry.enabled,
            author: entry.author.clone(),
        }
    }
}

/// Request body for `POST /plugins`.
#[derive(Debug, Deserialize)]
pub struct InstallPluginRequest {
    /// Unique plugin name.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Command to run the MCP server (Stdio transport).
    pub command: String,
    /// Arguments for the command.
    #[serde(default)]
    pub args: Vec<String>,
    /// Plugin version (defaults to `"0.1.0"`).
    pub version: Option<String>,
    /// Plugin author.
    pub author: Option<String>,
}

/// `GET /plugins` — list all installed plugins.
pub async fn list_plugins(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Viewer)?;
    let config = state.config.read().await;
    let summaries: Vec<PluginSummary> = config
        .plugins
        .iter()
        .map(PluginSummary::from)
        .collect();
    Ok(axum::Json(summaries))
}

/// `POST /plugins` — install a new plugin.
pub async fn install_plugin(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    axum::Json(req): axum::Json<InstallPluginRequest>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Admin)?;
    validate_name(&req.name)?;

    // Check for duplicates
    {
        let config = state.config.read().await;
        if config.find_plugin(&req.name).is_some() {
            return Err(ServerError(AivyxError::Config(format!(
                "plugin '{}' already installed",
                req.name
            ))));
        }
    }

    let version = req.version.unwrap_or_else(|| "0.1.0".into());

    let entry = PluginEntry {
        name: req.name.clone(),
        version,
        description: req.description.clone(),
        author: req.author,
        source: PluginSource::Local {
            path: req.command.clone(),
        },
        mcp_config: McpServerConfig {
            name: req.name.clone(),
            transport: McpTransport::Stdio {
                command: req.command,
                args: req.args,
            },
            env: HashMap::new(),
            timeout_secs: 30,
        },
        installed_at: Utc::now(),
        enabled: true,
    };

    // Load config, add plugin, save
    let mut config =
        aivyx_config::AivyxConfig::load(state.dirs.config_path()).map_err(ServerError)?;
    config.add_plugin(entry.clone());
    config.save(state.dirs.config_path()).map_err(ServerError)?;

    // Hot-reload: update in-memory config
    *state.config.write().await = config;

    // Audit
    if let Err(e) = state.audit_log.append(AuditEvent::PluginInstalled {
        plugin_name: req.name,
        source: req.description,
    }) {
        tracing::warn!("failed to audit plugin install: {e}");
    }

    Ok((
        axum::http::StatusCode::CREATED,
        axum::Json(PluginSummary::from(&entry)),
    ))
}

/// `DELETE /plugins/{name}` — remove an installed plugin.
pub async fn remove_plugin(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Admin)?;
    validate_name(&name)?;

    // Load config, remove plugin, save
    let mut config =
        aivyx_config::AivyxConfig::load(state.dirs.config_path()).map_err(ServerError)?;

    if config.remove_plugin(&name).is_none() {
        return Err(ServerError(AivyxError::Config(format!(
            "plugin not found: {name}"
        ))));
    }

    config.save(state.dirs.config_path()).map_err(ServerError)?;

    // Hot-reload: update in-memory config
    *state.config.write().await = config;

    // Audit
    if let Err(e) = state
        .audit_log
        .append(AuditEvent::PluginRemoved { plugin_name: name })
    {
        tracing::warn!("failed to audit plugin removal: {e}");
    }

    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_plugin_request_deserializes() {
        let json = r#"{
            "name": "my-plugin",
            "description": "A test plugin",
            "command": "npx",
            "args": ["-y", "@my/plugin"]
        }"#;
        let req: InstallPluginRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "my-plugin");
        assert_eq!(req.command, "npx");
        assert_eq!(req.args, vec!["-y", "@my/plugin"]);
        assert!(req.version.is_none());
    }

    #[test]
    fn install_plugin_request_with_all_fields() {
        let json = r#"{
            "name": "advanced",
            "description": "Advanced plugin",
            "command": "/usr/bin/plugin-server",
            "args": [],
            "version": "1.2.3",
            "author": "Test Author"
        }"#;
        let req: InstallPluginRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.version.as_deref(), Some("1.2.3"));
        assert_eq!(req.author.as_deref(), Some("Test Author"));
    }

    #[test]
    fn plugin_summary_from_entry() {
        let entry = PluginEntry {
            name: "test-plugin".into(),
            version: "0.1.0".into(),
            description: "A test".into(),
            author: Some("Author".into()),
            source: PluginSource::Local {
                path: "/usr/bin/test".into(),
            },
            mcp_config: McpServerConfig {
                name: "test-plugin".into(),
                transport: McpTransport::Stdio {
                    command: "test".into(),
                    args: vec![],
                },
                env: HashMap::new(),
                timeout_secs: 30,
            },
            installed_at: Utc::now(),
            enabled: true,
        };
        let summary = PluginSummary::from(&entry);
        assert_eq!(summary.name, "test-plugin");
        assert!(summary.enabled);
        assert_eq!(summary.author.as_deref(), Some("Author"));
    }

    #[test]
    fn validate_name_rejects_traversal() {
        assert!(validate_name("../etc/passwd").is_err());
        assert!(validate_name("foo/bar").is_err());
        assert!(validate_name("").is_err());
        assert!(validate_name("a".repeat(65).as_str()).is_err());
    }

    #[test]
    fn validate_name_accepts_valid() {
        assert!(validate_name("my-plugin").is_ok());
        assert!(validate_name("plugin_v2").is_ok());
        assert!(validate_name("test").is_ok());
    }
}

//! Tool introspection endpoint.
//!
//! `GET /tools` — list available built-in tools with schemas.

use std::sync::Arc;

use aivyx_agent::AgentProfile;
use aivyx_agent::built_in_tools::register_built_in_tools;
use aivyx_core::ToolRegistry;
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use serde::Deserialize;

use crate::app_state::AppState;
use crate::error::ServerError;

/// Query parameters for `GET /tools`.
#[derive(Debug, Deserialize)]
pub struct ToolsQuery {
    /// Filter tools by agent profile name. When set, returns only tools
    /// the agent is allowed to use (per its `tool_ids`).
    pub agent: Option<String>,
}

/// `GET /tools` — list available built-in tools.
///
/// Returns tool name, description, and JSON Schema for each tool's input.
/// Optionally filter by agent profile's allowed tool list.
pub async fn list_tools(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ToolsQuery>,
) -> Result<impl IntoResponse, ServerError> {
    let mut registry = ToolRegistry::new();

    // Determine which tool names to register
    let allowed_names = if let Some(ref agent_name) = query.agent {
        let path = state.dirs.agents_dir().join(format!("{agent_name}.toml"));
        if !path.exists() {
            return Err(crate::error::ServerError(aivyx_core::AivyxError::Config(
                format!("agent not found: {agent_name}"),
            )));
        }
        let profile = AgentProfile::load(&path)?;
        profile.tool_ids
    } else {
        vec![] // empty = all tools
    };

    register_built_in_tools(&mut registry, &allowed_names);
    let definitions = registry.tool_definitions();

    Ok(axum::Json(definitions))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_query_no_agent() {
        let json = r#"{}"#;
        let q: ToolsQuery = serde_json::from_str(json).unwrap();
        assert!(q.agent.is_none());
    }

    #[test]
    fn tools_query_with_agent() {
        let json = r#"{"agent":"coder"}"#;
        let q: ToolsQuery = serde_json::from_str(json).unwrap();
        assert_eq!(q.agent.as_deref(), Some("coder"));
    }

    #[test]
    fn all_tools_registered() {
        let mut registry = ToolRegistry::new();
        register_built_in_tools(&mut registry, &[]);
        let defs = registry.tool_definitions();
        // We have 22 built-in tools
        assert!(
            defs.len() >= 20,
            "expected at least 20 tools, got {}",
            defs.len()
        );
        // Each definition has name, description, input_schema
        for def in &defs {
            assert!(def["name"].is_string());
            assert!(def["description"].is_string());
            assert!(def["input_schema"].is_object());
        }
    }

    #[test]
    fn filtered_tools_registered() {
        let mut registry = ToolRegistry::new();
        let allowed = vec!["file_read".to_string(), "shell".to_string()];
        register_built_in_tools(&mut registry, &allowed);
        let defs = registry.tool_definitions();
        assert_eq!(defs.len(), 2);
    }

    #[test]
    fn tools_query_empty_string_agent() {
        let json = r#"{"agent":""}"#;
        let q: ToolsQuery = serde_json::from_str(json).unwrap();
        assert_eq!(q.agent.as_deref(), Some(""));
    }

    #[test]
    fn tools_query_ignores_unknown_fields() {
        let json = r#"{"extra":"value"}"#;
        let q: ToolsQuery = serde_json::from_str(json).unwrap();
        assert!(q.agent.is_none());
    }

    #[test]
    fn single_tool_filter() {
        let mut registry = ToolRegistry::new();
        let allowed = vec!["file_read".to_string()];
        register_built_in_tools(&mut registry, &allowed);
        let defs = registry.tool_definitions();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0]["name"], "file_read");
    }

    #[test]
    fn nonexistent_tool_filter_yields_empty() {
        let mut registry = ToolRegistry::new();
        let allowed = vec!["nonexistent_tool".to_string()];
        register_built_in_tools(&mut registry, &allowed);
        let defs = registry.tool_definitions();
        assert_eq!(defs.len(), 0);
    }
}

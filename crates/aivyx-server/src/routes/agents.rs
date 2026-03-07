//! Agent profile management endpoints.
//!
//! `GET /agents` — list all configured agents.
//! `GET /agents/:name` — get a single agent profile.
//! `POST /agents` — create a new agent profile.
//! `PUT /agents/:name` — update an agent profile (partial merge).
//! `DELETE /agents/:name` — delete an agent profile.
//! `POST /agents/:name/duplicate` — copy an agent to a new name.
//! `GET /agents/:name/persona` — get an agent's persona.
//! `PUT /agents/:name/capabilities` — replace an agent's capability set.
//! `PATCH /agents/:name/persona` — partially update an agent's persona.

use std::sync::Arc;

use aivyx_agent::{AgentProfile, Persona, ProfileCapability};
use aivyx_audit::AuditEvent;
use aivyx_config::McpServerConfig;
use aivyx_core::{AivyxError, AutonomyTier};
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use crate::app_state::AppState;
use crate::error::ServerError;
use crate::validation::validate_name;

/// Response item for agent listing.
#[derive(Debug, Serialize)]
pub struct AgentSummary {
    /// Agent profile name (filename without `.toml`).
    pub name: String,
    /// The agent's role description.
    pub role: String,
}

/// `GET /agents` — list all agent profiles.
pub async fn list_agents(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ServerError> {
    let agents_dir = state.dirs.agents_dir();
    let mut agents = Vec::new();

    if agents_dir.exists() {
        for entry in std::fs::read_dir(&agents_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "toml")
                && let Ok(profile) = AgentProfile::load(&path)
            {
                agents.push(AgentSummary {
                    name: profile.name.clone(),
                    role: profile.role.clone(),
                });
            }
        }
    }

    Ok(axum::Json(agents))
}

/// `GET /agents/:name` — get a single agent profile.
pub async fn get_agent(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
    validate_name(&name)?;
    let path = state.dirs.agents_dir().join(format!("{name}.toml"));
    if !path.exists() {
        return Err(ServerError(AivyxError::Config(format!(
            "agent not found: {name}"
        ))));
    }
    let profile = AgentProfile::load(&path)?;
    Ok(axum::Json(profile))
}

/// Request body for `POST /agents`.
#[derive(Debug, Deserialize)]
pub struct CreateAgentRequest {
    /// Agent name.
    pub name: String,
    /// Agent role description.
    pub role: String,
    /// System prompt (soul).
    #[serde(default)]
    pub soul: Option<String>,
    /// Structured persona (overrides soul when present).
    #[serde(default)]
    pub persona: Option<Persona>,
}

/// `POST /agents` — create a new agent profile from JSON.
pub async fn create_agent(
    State(state): State<Arc<AppState>>,
    axum::Json(req): axum::Json<CreateAgentRequest>,
) -> Result<impl IntoResponse, ServerError> {
    validate_name(&req.name)?;
    let path = state.dirs.agents_dir().join(format!("{}.toml", req.name));
    if path.exists() {
        return Err(ServerError(AivyxError::Config(format!(
            "agent already exists: {}",
            req.name
        ))));
    }

    let mut profile = AgentProfile::template(&req.name, &req.role);
    if let Some(persona) = req.persona {
        profile.persona = Some(persona);
    }
    if let Some(soul) = req.soul {
        profile.soul = soul;
    }
    profile.save(&path)?;

    // Audit
    if let Err(e) = state.audit_log.append(AuditEvent::AgentProfileCreated {
        agent_name: profile.name.clone(),
    }) {
        tracing::warn!("failed to audit agent creation: {e}");
    }

    Ok((axum::http::StatusCode::CREATED, axum::Json(profile)))
}

/// Request body for `PATCH /agents/:name/persona`.
#[derive(Debug, Deserialize)]
pub struct PatchPersonaRequest {
    /// Apply a preset as the base (assistant, coder, researcher, writer, ops).
    #[serde(default)]
    pub preset: Option<String>,
    /// Formality dimension (0.0 to 1.0).
    #[serde(default)]
    pub formality: Option<f32>,
    /// Verbosity dimension (0.0 to 1.0).
    #[serde(default)]
    pub verbosity: Option<f32>,
    /// Warmth dimension (0.0 to 1.0).
    #[serde(default)]
    pub warmth: Option<f32>,
    /// Humor dimension (0.0 to 1.0).
    #[serde(default)]
    pub humor: Option<f32>,
    /// Confidence dimension (0.0 to 1.0).
    #[serde(default)]
    pub confidence: Option<f32>,
    /// Curiosity dimension (0.0 to 1.0).
    #[serde(default)]
    pub curiosity: Option<f32>,
    /// Tone descriptor.
    #[serde(default)]
    pub tone: Option<String>,
    /// Language complexity level.
    #[serde(default)]
    pub language_level: Option<String>,
    /// Code style notes.
    #[serde(default)]
    pub code_style: Option<String>,
    /// Error reporting style.
    #[serde(default)]
    pub error_style: Option<String>,
    /// Greeting template.
    #[serde(default)]
    pub greeting: Option<String>,
    /// Use emoji in responses.
    #[serde(default)]
    pub uses_emoji: Option<bool>,
    /// Use analogies and metaphors.
    #[serde(default)]
    pub uses_analogies: Option<bool>,
    /// Ask follow-up questions.
    #[serde(default)]
    pub asks_followups: Option<bool>,
    /// Admit uncertainty.
    #[serde(default)]
    pub admits_uncertainty: Option<bool>,
}

/// `GET /agents/:name/persona` — get the persona for an agent.
pub async fn get_persona(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
    validate_name(&name)?;
    let path = state.dirs.agents_dir().join(format!("{name}.toml"));
    if !path.exists() {
        return Err(ServerError(AivyxError::Config(format!(
            "agent not found: {name}"
        ))));
    }
    let profile = AgentProfile::load(&path)?;
    match profile.persona {
        Some(persona) => Ok(axum::Json(
            serde_json::to_value(persona).unwrap_or_default(),
        )),
        None => Err(ServerError(AivyxError::Config(format!(
            "agent '{name}' has no persona configured"
        )))),
    }
}

/// `PATCH /agents/:name/persona` — partially update persona fields.
pub async fn patch_persona(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    axum::Json(req): axum::Json<PatchPersonaRequest>,
) -> Result<impl IntoResponse, ServerError> {
    validate_name(&name)?;
    let path = state.dirs.agents_dir().join(format!("{name}.toml"));
    if !path.exists() {
        return Err(ServerError(AivyxError::Config(format!(
            "agent not found: {name}"
        ))));
    }

    let mut profile = AgentProfile::load(&path)?;

    // Start from preset, existing persona, or default
    let mut persona = if let Some(ref preset_name) = req.preset {
        Persona::for_role(preset_name).ok_or_else(|| {
            ServerError(AivyxError::Config(format!("unknown preset: {preset_name}")))
        })?
    } else {
        profile.persona.clone().unwrap_or_default()
    };

    // Overlay provided fields
    if let Some(v) = req.formality {
        persona.formality = v;
    }
    if let Some(v) = req.verbosity {
        persona.verbosity = v;
    }
    if let Some(v) = req.warmth {
        persona.warmth = v;
    }
    if let Some(v) = req.humor {
        persona.humor = v;
    }
    if let Some(v) = req.confidence {
        persona.confidence = v;
    }
    if let Some(v) = req.curiosity {
        persona.curiosity = v;
    }
    if let Some(v) = req.tone {
        persona.tone = Some(v);
    }
    if let Some(v) = req.language_level {
        persona.language_level = Some(v);
    }
    if let Some(v) = req.code_style {
        persona.code_style = Some(v);
    }
    if let Some(v) = req.error_style {
        persona.error_style = Some(v);
    }
    if let Some(v) = req.greeting {
        persona.greeting = Some(v);
    }
    if let Some(v) = req.uses_emoji {
        persona.uses_emoji = v;
    }
    if let Some(v) = req.uses_analogies {
        persona.uses_analogies = v;
    }
    if let Some(v) = req.asks_followups {
        persona.asks_followups = v;
    }
    if let Some(v) = req.admits_uncertainty {
        persona.admits_uncertainty = v;
    }

    persona.normalize();
    profile.persona = Some(persona.clone());
    profile.save(&path)?;

    Ok(axum::Json(persona))
}

/// Request body for `PUT /agents/:name/capabilities`.
#[derive(Debug, Deserialize)]
pub struct UpdateCapabilitiesRequest {
    /// New capability set (replaces existing capabilities).
    pub capabilities: Vec<ProfileCapability>,
}

/// `PUT /agents/:name/capabilities` — replace an agent's capability set.
///
/// Separate from the general update endpoint because capability changes are
/// security-sensitive and warrant explicit handling and audit logging.
pub async fn update_capabilities(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    axum::Json(req): axum::Json<UpdateCapabilitiesRequest>,
) -> Result<impl IntoResponse, ServerError> {
    validate_name(&name)?;
    let path = state.dirs.agents_dir().join(format!("{name}.toml"));
    if !path.exists() {
        return Err(ServerError(AivyxError::Config(format!(
            "agent not found: {name}"
        ))));
    }

    let mut profile = AgentProfile::load(&path)?;
    profile.capabilities = req.capabilities;
    profile.save(&path)?;

    // Audit
    if let Err(e) = state.audit_log.append(AuditEvent::AgentProfileUpdated {
        agent_name: name,
        fields_changed: vec!["capabilities".into()],
    }) {
        tracing::warn!("failed to audit capability update: {e}");
    }

    Ok(axum::Json(profile))
}

/// Request body for `PUT /agents/:name`.
#[derive(Debug, Deserialize)]
pub struct UpdateAgentRequest {
    /// Agent role description.
    #[serde(default)]
    pub role: Option<String>,
    /// System prompt (soul).
    #[serde(default)]
    pub soul: Option<String>,
    /// Tool IDs this agent may use.
    #[serde(default)]
    pub tool_ids: Option<Vec<String>>,
    /// Skills list.
    #[serde(default)]
    pub skills: Option<Vec<String>>,
    /// Autonomy tier override.
    #[serde(default)]
    pub autonomy_tier: Option<AutonomyTier>,
    /// Named provider reference.
    #[serde(default)]
    pub provider: Option<String>,
    /// Maximum tokens for LLM responses.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Structured persona (replaces existing persona when present).
    #[serde(default)]
    pub persona: Option<Persona>,
    /// MCP server configurations.
    #[serde(default)]
    pub mcp_servers: Option<Vec<McpServerConfig>>,
}

/// `PUT /agents/:name` — update an agent profile (partial merge).
///
/// Only non-null fields are applied; omitted fields keep their current values.
pub async fn update_agent(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    axum::Json(req): axum::Json<UpdateAgentRequest>,
) -> Result<impl IntoResponse, ServerError> {
    validate_name(&name)?;
    let path = state.dirs.agents_dir().join(format!("{name}.toml"));
    if !path.exists() {
        return Err(ServerError(AivyxError::Config(format!(
            "agent not found: {name}"
        ))));
    }

    let mut profile = AgentProfile::load(&path)?;

    // Collect changed fields for audit before consuming req fields
    let mut fields_changed = Vec::new();
    if req.role.is_some() {
        fields_changed.push("role".into());
    }
    if req.soul.is_some() {
        fields_changed.push("soul".into());
    }
    if req.tool_ids.is_some() {
        fields_changed.push("tool_ids".into());
    }
    if req.skills.is_some() {
        fields_changed.push("skills".into());
    }
    if req.autonomy_tier.is_some() {
        fields_changed.push("autonomy_tier".into());
    }
    if req.provider.is_some() {
        fields_changed.push("provider".into());
    }
    if req.max_tokens.is_some() {
        fields_changed.push("max_tokens".into());
    }
    if req.persona.is_some() {
        fields_changed.push("persona".into());
    }

    if let Some(role) = req.role {
        profile.role = role;
    }
    if let Some(soul) = req.soul {
        profile.soul = soul;
    }
    if let Some(tool_ids) = req.tool_ids {
        profile.tool_ids = tool_ids;
    }
    if let Some(skills) = req.skills {
        profile.skills = skills;
    }
    if let Some(tier) = req.autonomy_tier {
        profile.autonomy_tier = Some(tier);
    }
    if let Some(provider) = req.provider {
        profile.provider = if provider.is_empty() {
            None
        } else {
            Some(provider)
        };
    }
    if let Some(max_tokens) = req.max_tokens {
        profile.max_tokens = max_tokens;
    }
    if let Some(persona) = req.persona {
        profile.persona = Some(persona);
    }
    // mcp_servers is intentionally NOT settable via API to prevent remote
    // command injection — MCP servers must be configured via TOML files only.

    profile.save(&path)?;

    // Audit
    if let Err(e) = state.audit_log.append(AuditEvent::AgentProfileUpdated {
        agent_name: name,
        fields_changed,
    }) {
        tracing::warn!("failed to audit agent update: {e}");
    }

    Ok(axum::Json(profile))
}

/// `DELETE /agents/:name` — delete an agent profile.
///
/// Refuses to delete the default "aivyx" agent.
pub async fn delete_agent(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
    validate_name(&name)?;
    if name == "aivyx" {
        return Err(ServerError(AivyxError::Config(
            "cannot delete the default agent".into(),
        )));
    }
    let path = state.dirs.agents_dir().join(format!("{name}.toml"));
    if !path.exists() {
        return Err(ServerError(AivyxError::Config(format!(
            "agent not found: {name}"
        ))));
    }
    std::fs::remove_file(&path)?;

    // Audit
    if let Err(e) = state
        .audit_log
        .append(AuditEvent::AgentProfileDeleted { agent_name: name })
    {
        tracing::warn!("failed to audit agent deletion: {e}");
    }

    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Request body for `POST /agents/:name/duplicate`.
#[derive(Debug, Deserialize)]
pub struct DuplicateAgentRequest {
    /// Name for the duplicated agent.
    pub new_name: String,
}

/// `POST /agents/:name/duplicate` — copy an agent profile to a new name.
pub async fn duplicate_agent(
    State(state): State<Arc<AppState>>,
    Path(source_name): Path<String>,
    axum::Json(req): axum::Json<DuplicateAgentRequest>,
) -> Result<impl IntoResponse, ServerError> {
    validate_name(&source_name)?;
    validate_name(&req.new_name)?;

    let source_path = state.dirs.agents_dir().join(format!("{source_name}.toml"));
    if !source_path.exists() {
        return Err(ServerError(AivyxError::Config(format!(
            "agent not found: {source_name}"
        ))));
    }

    let target_path = state
        .dirs
        .agents_dir()
        .join(format!("{}.toml", req.new_name));
    if target_path.exists() {
        return Err(ServerError(AivyxError::Config(format!(
            "agent already exists: {}",
            req.new_name
        ))));
    }

    let mut profile = AgentProfile::load(&source_path)?;
    profile.name = req.new_name;
    profile.save(&target_path)?;

    Ok((axum::http::StatusCode::CREATED, axum::Json(profile)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_summary_serializes() {
        let summary = AgentSummary {
            name: "coder".into(),
            role: "writes code".into(),
        };
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["name"], "coder");
    }

    #[test]
    fn create_agent_request_deserializes() {
        let json = r#"{"name":"test","role":"tester"}"#;
        let req: CreateAgentRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "test");
        assert!(req.soul.is_none());
        assert!(req.persona.is_none());
    }

    #[test]
    fn patch_persona_request_partial() {
        let json = r#"{"formality":0.9,"warmth":0.2}"#;
        let req: PatchPersonaRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.formality, Some(0.9));
        assert_eq!(req.warmth, Some(0.2));
        assert!(req.verbosity.is_none());
        assert!(req.preset.is_none());
        assert!(req.uses_emoji.is_none());
    }

    #[test]
    fn patch_persona_request_with_preset() {
        let json = r#"{"preset":"coder","humor":0.5}"#;
        let req: PatchPersonaRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.preset.as_deref(), Some("coder"));
        assert_eq!(req.humor, Some(0.5));
    }

    #[test]
    fn update_agent_request_partial() {
        let json = r#"{"role":"updated role"}"#;
        let req: UpdateAgentRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.role.as_deref(), Some("updated role"));
        assert!(req.soul.is_none());
        assert!(req.tool_ids.is_none());
        assert!(req.autonomy_tier.is_none());
        assert!(req.persona.is_none());
    }

    #[test]
    fn update_agent_request_full() {
        let json = r#"{
            "role": "coder",
            "soul": "You are a coder.",
            "tool_ids": ["file_read", "shell"],
            "autonomy_tier": "Trust",
            "provider": "my-provider",
            "max_tokens": 8192,
            "mcp_servers": []
        }"#;
        let req: UpdateAgentRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.role.as_deref(), Some("coder"));
        assert_eq!(req.tool_ids.as_ref().unwrap().len(), 2);
        assert_eq!(req.autonomy_tier, Some(AutonomyTier::Trust));
        assert_eq!(req.max_tokens, Some(8192));
    }

    #[test]
    fn update_capabilities_request_deserializes() {
        let json = r#"{"capabilities":[{"scope":{"Filesystem":{"root":"/"}},"pattern":"*"},{"scope":"Calendar","pattern":"read:*"}]}"#;
        let req: UpdateCapabilitiesRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.capabilities.len(), 2);
        assert_eq!(req.capabilities[0].pattern, "*");
        assert_eq!(req.capabilities[1].pattern, "read:*");
    }

    #[test]
    fn duplicate_agent_request_deserializes() {
        let json = r#"{"new_name":"my-clone"}"#;
        let req: DuplicateAgentRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.new_name, "my-clone");
    }
}

//! SKILL.md management endpoints.
//!
//! `GET /skills` — list installed skills.
//! `GET /skills/{name}` — get full skill details.
//! `DELETE /skills/{name}` — remove an installed skill.

use std::sync::Arc;

use aivyx_config::{discover_skills, load_skill};
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use serde::Serialize;

use crate::app_state::AppState;
use crate::error::ServerError;
use crate::validation::validate_name;

/// Summary item for skill listing.
#[derive(Debug, Serialize)]
pub struct SkillSummaryResponse {
    /// Skill name (lowercase + hyphens).
    pub name: String,
    /// Short description.
    pub description: String,
}

/// Full skill detail response.
#[derive(Debug, Serialize)]
pub struct SkillDetailResponse {
    /// Skill name.
    pub name: String,
    /// Short description.
    pub description: String,
    /// SPDX license identifier.
    pub license: Option<String>,
    /// Environment requirements.
    pub compatibility: Option<String>,
    /// Space-delimited tool allowlist.
    pub allowed_tools: Option<String>,
    /// Extension metadata.
    pub metadata: std::collections::HashMap<String, String>,
    /// Full markdown body.
    pub body: String,
    /// Source directory path.
    pub path: String,
}

/// `GET /skills` — list installed skills.
pub async fn list_skills(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ServerError> {
    let skills_dir = state.dirs.skills_dir();

    let summaries = if skills_dir.exists() {
        discover_skills(&skills_dir)?
    } else {
        Vec::new()
    };

    let response: Vec<SkillSummaryResponse> = summaries
        .into_iter()
        .map(|s| SkillSummaryResponse {
            name: s.name,
            description: s.description,
        })
        .collect();

    Ok(axum::Json(response))
}

/// `GET /skills/{name}` — get full skill details.
pub async fn get_skill(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
    validate_name(&name)?;

    let skill_path = state.dirs.skills_dir().join(&name).join("SKILL.md");
    if !skill_path.exists() {
        return Err(ServerError(aivyx_core::AivyxError::Config(format!(
            "skill not found: {name}"
        ))));
    }

    let loaded = load_skill(&skill_path)?;

    Ok(axum::Json(SkillDetailResponse {
        name: loaded.manifest.name,
        description: loaded.manifest.description,
        license: loaded.manifest.license,
        compatibility: loaded.manifest.compatibility,
        allowed_tools: loaded.manifest.allowed_tools,
        metadata: loaded.manifest.metadata,
        body: loaded.body,
        path: loaded.base_dir.to_string_lossy().to_string(),
    }))
}

/// `DELETE /skills/{name}` — remove an installed skill.
pub async fn delete_skill(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
    validate_name(&name)?;

    let skill_dir = state.dirs.skills_dir().join(&name);
    if !skill_dir.exists() {
        return Err(ServerError(aivyx_core::AivyxError::Config(format!(
            "skill not found: {name}"
        ))));
    }

    std::fs::remove_dir_all(&skill_dir)?;

    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_summary_serializes() {
        let summary = SkillSummaryResponse {
            name: "webapp-testing".into(),
            description: "Guide for testing web apps".into(),
        };
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["name"], "webapp-testing");
        assert_eq!(json["description"], "Guide for testing web apps");
    }

    #[test]
    fn skill_detail_serializes() {
        let detail = SkillDetailResponse {
            name: "webapp-testing".into(),
            description: "Guide for testing web apps".into(),
            license: Some("MIT".into()),
            compatibility: Some("Node.js >= 18".into()),
            allowed_tools: Some("Bash(npx:*) Read".into()),
            metadata: [("author".into(), "platform-team".into())]
                .into_iter()
                .collect(),
            body: "# Instructions\n\nDo the thing.".into(),
            path: "/home/user/.aivyx/skills/webapp-testing".into(),
        };
        let json = serde_json::to_value(&detail).unwrap();
        assert_eq!(json["name"], "webapp-testing");
        assert_eq!(json["license"], "MIT");
        assert_eq!(json["metadata"]["author"], "platform-team");
        assert!(json["body"].as_str().unwrap().contains("Instructions"));
    }

    #[test]
    fn skill_detail_without_optional_fields() {
        let detail = SkillDetailResponse {
            name: "minimal-skill".into(),
            description: "A minimal skill".into(),
            license: None,
            compatibility: None,
            allowed_tools: None,
            metadata: std::collections::HashMap::new(),
            body: "# Body".into(),
            path: "/tmp/skills/minimal-skill".into(),
        };
        let json = serde_json::to_value(&detail).unwrap();
        assert!(json["license"].is_null());
        assert!(json["compatibility"].is_null());
    }
}

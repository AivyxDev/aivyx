//! Project management endpoints.
//!
//! `GET /projects` — list all registered projects.
//! `POST /projects` — register a new project.
//! `DELETE /projects/{name}` — remove a registered project.

use std::sync::Arc;

use aivyx_config::ProjectConfig;
use aivyx_core::AivyxError;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use crate::app_state::AppState;
use crate::error::ServerError;
use crate::extractors::AuthContextExt;
use crate::validation::validate_name;
use aivyx_tenant::AivyxRole;

/// Summary item for project listing.
#[derive(Debug, Serialize)]
pub struct ProjectSummary {
    /// Project slug name.
    pub name: String,
    /// Primary programming language.
    pub language: Option<String>,
    /// Absolute path to the project root.
    pub path: String,
}

/// `GET /projects` — list all registered projects.
pub async fn list_projects(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Viewer)?;
    let config = aivyx_config::AivyxConfig::load(state.dirs.config_path())?;

    let projects: Vec<ProjectSummary> = config
        .projects
        .iter()
        .map(|p| ProjectSummary {
            name: p.name.clone(),
            language: p.language.clone(),
            path: p.path.to_string_lossy().to_string(),
        })
        .collect();

    Ok(axum::Json(projects))
}

/// Request body for `POST /projects`.
#[derive(Debug, Deserialize)]
pub struct CreateProjectRequest {
    /// Absolute path to the project root.
    pub path: String,
    /// Custom project name (defaults to directory name).
    #[serde(default)]
    pub name: Option<String>,
    /// Primary language.
    #[serde(default)]
    pub language: Option<String>,
}

/// `POST /projects` — register a new project.
pub async fn create_project(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    axum::Json(req): axum::Json<CreateProjectRequest>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Operator)?;
    let abs_path = std::path::PathBuf::from(&req.path);
    if !abs_path.is_dir() {
        return Err(ServerError(AivyxError::Config(format!(
            "not a directory: {}",
            req.path
        ))));
    }

    let project_name = req.name.unwrap_or_else(|| {
        abs_path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| "project".to_string())
    });
    validate_name(&project_name)?;

    let mut config = aivyx_config::AivyxConfig::load(state.dirs.config_path())?;
    let mut project = ProjectConfig::new(&project_name, &abs_path);
    project.language = req.language;

    config.add_project(project.clone())?;
    config.save(state.dirs.config_path())?;

    // Audit log
    let _ = state
        .audit_log
        .append(aivyx_audit::AuditEvent::ProjectRegistered {
            project_name: project.name.clone(),
            project_path: abs_path.to_string_lossy().to_string(),
        });

    let summary = ProjectSummary {
        name: project.name,
        language: project.language,
        path: abs_path.to_string_lossy().to_string(),
    };
    Ok((axum::http::StatusCode::CREATED, axum::Json(summary)))
}

/// `DELETE /projects/{name}` — remove a registered project.
pub async fn delete_project(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Operator)?;
    validate_name(&name)?;

    let mut config = aivyx_config::AivyxConfig::load(state.dirs.config_path())?;
    config.remove_project(&name)?;
    config.save(state.dirs.config_path())?;

    // Audit log
    let _ = state
        .audit_log
        .append(aivyx_audit::AuditEvent::ProjectRemoved {
            project_name: name.clone(),
        });

    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_summary_serializes() {
        let summary = ProjectSummary {
            name: "aivyx".into(),
            language: Some("Rust".into()),
            path: "/home/user/aivyx".into(),
        };
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["name"], "aivyx");
        assert_eq!(json["language"], "Rust");
    }

    #[test]
    fn create_project_request_deserializes() {
        let json = r#"{"path":"/tmp/myapp"}"#;
        let req: CreateProjectRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.path, "/tmp/myapp");
        assert!(req.name.is_none());
        assert!(req.language.is_none());
    }

    #[test]
    fn create_project_request_with_all_fields() {
        let json = r#"{"path":"/tmp/myapp","name":"my-app","language":"Rust"}"#;
        let req: CreateProjectRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name.as_deref(), Some("my-app"));
        assert_eq!(req.language.as_deref(), Some("Rust"));
    }

    #[test]
    fn create_project_request_missing_path_fails() {
        let json = r#"{"name":"my-app"}"#;
        let result = serde_json::from_str::<CreateProjectRequest>(json);
        assert!(result.is_err());
    }

    #[test]
    fn project_summary_without_language() {
        let summary = ProjectSummary {
            name: "scripts".into(),
            language: None,
            path: "/home/user/scripts".into(),
        };
        let json = serde_json::to_value(&summary).unwrap();
        assert!(json["language"].is_null());
        assert_eq!(json["path"], "/home/user/scripts");
    }

    #[test]
    fn validate_name_rejects_traversal() {
        assert!(validate_name("../etc").is_err());
        assert!(validate_name("foo/bar").is_err());
        assert!(validate_name("foo\\bar").is_err());
        assert!(validate_name("").is_err());
        assert!(validate_name("a\0b").is_err());
    }

    #[test]
    fn validate_name_rejects_too_long() {
        assert!(validate_name(&"x".repeat(65)).is_err());
    }

    #[test]
    fn validate_name_accepts_max_length() {
        assert!(validate_name(&"x".repeat(64)).is_ok());
    }

    #[test]
    fn validate_name_accepts_valid_names() {
        assert!(validate_name("my-project").is_ok());
        assert!(validate_name("project_v2").is_ok());
        assert!(validate_name("a").is_ok());
    }
}

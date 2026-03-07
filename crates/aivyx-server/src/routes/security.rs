//! Security audit endpoints.
//!
//! `GET /security/capability-audit` -- generate a capability audit report.

use std::sync::Arc;

use axum::extract::State;
use axum::Json;

use aivyx_tenant::AivyxRole;

use crate::app_state::AppState;
use crate::error::ServerError;
use crate::extractors::AuthContextExt;
use crate::security::capability_audit;

/// `GET /security/capability-audit` -- scan agent profiles and return a
/// capability audit report with security warnings.
pub async fn capability_audit_handler(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
) -> Result<Json<capability_audit::CapabilityAuditReport>, ServerError> {
    auth.require_role(AivyxRole::Admin)?;
    let report = capability_audit::audit_agent_capabilities(&state.dirs.agents_dir())?;
    Ok(Json(report))
}

//! Audit log endpoints.
//!
//! `GET /audit?last=10` — recent audit entries.
//! `POST /audit/verify` — verify HMAC chain integrity.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use chrono::DateTime;
use serde::{Deserialize, Serialize};

use aivyx_audit::AuditFilter;

use crate::app_state::AppState;
use crate::error::ServerError;
use crate::extractors::AuthContextExt;
use aivyx_tenant::AivyxRole;

/// Query parameters for `GET /audit`.
#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    /// Number of recent entries to return (default: 10).
    #[serde(default = "default_last")]
    pub last: usize,
}

fn default_last() -> usize {
    10
}

/// Response body for `POST /audit/verify`.
#[derive(Debug, Serialize)]
pub struct VerifyResponse {
    /// Whether the chain is valid.
    pub valid: bool,
    /// Number of entries checked.
    pub entries_checked: u64,
}

/// `GET /audit` — return recent audit entries.
pub async fn recent_audit(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Query(query): Query<AuditQuery>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Viewer)?;
    let entries = state.audit_log.recent(query.last.min(1000))?;
    Ok(axum::Json(entries))
}

/// `POST /audit/verify` — verify the HMAC chain integrity.
pub async fn verify_audit(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Admin)?;
    let result = state.audit_log.verify()?;
    Ok(axum::Json(VerifyResponse {
        valid: result.valid,
        entries_checked: result.entries_checked,
    }))
}

/// Query parameters for `GET /audit/search`.
#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    /// Filter by event type (serde tag name, e.g. "AgentCreated").
    #[serde(rename = "type")]
    pub event_type: Option<String>,
    /// Start of date range (RFC 3339).
    pub from: Option<String>,
    /// End of date range (RFC 3339).
    pub to: Option<String>,
    /// Maximum number of results (default: 100, capped at 1000).
    #[serde(default = "default_search_limit")]
    pub limit: usize,
}

fn default_search_limit() -> usize {
    100
}

/// `GET /audit/search` — search audit entries by type and date range.
pub async fn search_audit(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Query(query): Query<SearchQuery>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Viewer)?;
    let filter = AuditFilter {
        event_types: query.event_type.map(|t| vec![t]),
        from: query
            .from
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok().map(|dt| dt.to_utc())),
        to: query
            .to
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok().map(|dt| dt.to_utc())),
        limit: Some(query.limit.min(1000)),
    };

    let results = state.audit_log.search(&filter)?;
    Ok(axum::Json(results))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_query_default() {
        let q: AuditQuery = serde_json::from_str("{}").unwrap();
        assert_eq!(q.last, 10);
    }

    #[test]
    fn audit_query_custom() {
        let q: AuditQuery = serde_json::from_str(r#"{"last":5}"#).unwrap();
        assert_eq!(q.last, 5);
    }

    #[test]
    fn verify_response_serializes() {
        let r = VerifyResponse {
            valid: true,
            entries_checked: 42,
        };
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["valid"], true);
        assert_eq!(json["entries_checked"], 42);
    }

    #[test]
    fn search_query_defaults() {
        let q: SearchQuery = serde_json::from_str("{}").unwrap();
        assert!(q.event_type.is_none());
        assert!(q.from.is_none());
        assert!(q.to.is_none());
        assert_eq!(q.limit, 100);
    }

    #[test]
    fn search_query_with_params() {
        let q: SearchQuery = serde_json::from_str(r#"{"type":"AgentCreated","limit":50}"#).unwrap();
        assert_eq!(q.event_type.as_deref(), Some("AgentCreated"));
        assert_eq!(q.limit, 50);
    }
}

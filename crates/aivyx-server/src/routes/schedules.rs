//! Schedule and notification management endpoints.
//!
//! `GET /schedules` — list all schedule entries.
//! `POST /schedules` — create a new schedule entry.
//! `DELETE /schedules/{name}` — remove a schedule entry.
//! `GET /notifications` — list pending notifications.
//! `DELETE /notifications` — drain all pending notifications.
//! `PUT /notifications/{id}/rating` — rate a notification output.
//! `GET /notifications/history` — list rated notification history.

use std::sync::Arc;

use aivyx_config::ScheduleEntry;
use aivyx_core::AivyxError;
use aivyx_crypto::{MasterKey, derive_schedule_key};
use aivyx_memory::{NotificationStore, Rating};
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use crate::app_state::AppState;
use crate::error::ServerError;
use crate::extractors::AuthContextExt;
use crate::validation::validate_name;
use aivyx_tenant::AivyxRole;

/// Summary item for schedule listing.
#[derive(Debug, Serialize)]
pub struct ScheduleSummary {
    /// Schedule slug name.
    pub name: String,
    /// Cron expression.
    pub cron: String,
    /// Agent profile to run.
    pub agent: String,
    /// Prompt text.
    pub prompt: String,
    /// Whether results are stored as notifications.
    pub notify: bool,
    /// Whether the schedule is active.
    pub enabled: bool,
    /// Last execution timestamp (if any).
    pub last_run_at: Option<String>,
}

/// `GET /schedules` — list all configured schedule entries.
pub async fn list_schedules(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Viewer)?;
    let config = aivyx_config::AivyxConfig::load(state.dirs.config_path())?;

    let schedules: Vec<ScheduleSummary> = config
        .schedules
        .iter()
        .map(|e| ScheduleSummary {
            name: e.name.clone(),
            cron: e.cron.clone(),
            agent: e.agent.clone(),
            prompt: e.prompt.clone(),
            notify: e.notify,
            enabled: e.enabled,
            last_run_at: e.last_run_at.map(|t| t.to_rfc3339()),
        })
        .collect();

    Ok(axum::Json(schedules))
}

/// Request body for `POST /schedules`.
#[derive(Debug, Deserialize)]
pub struct CreateScheduleRequest {
    /// Schedule name (slug-style).
    pub name: String,
    /// Cron expression (5-field).
    pub cron: String,
    /// Agent profile to run.
    pub agent: String,
    /// Prompt to send.
    pub prompt: String,
    /// Whether to store results as notifications (default true).
    #[serde(default = "default_true")]
    pub notify: bool,
}

fn default_true() -> bool {
    true
}

/// `POST /schedules` — create a new schedule entry.
pub async fn create_schedule(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    axum::Json(req): axum::Json<CreateScheduleRequest>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Operator)?;
    validate_name(&req.name)?;
    aivyx_config::validate_cron(&req.cron)?;

    let mut config = aivyx_config::AivyxConfig::load(state.dirs.config_path())?;
    let mut entry = ScheduleEntry::new(&req.name, &req.cron, &req.agent, &req.prompt);
    entry.notify = req.notify;

    config.add_schedule(entry)?;
    config.save(state.dirs.config_path())?;

    let summary = ScheduleSummary {
        name: req.name,
        cron: req.cron,
        agent: req.agent,
        prompt: req.prompt,
        notify: req.notify,
        enabled: true,
        last_run_at: None,
    };
    Ok((axum::http::StatusCode::CREATED, axum::Json(summary)))
}

/// `DELETE /schedules/{name}` — remove a schedule entry.
pub async fn delete_schedule(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Operator)?;
    validate_name(&name)?;

    let mut config = aivyx_config::AivyxConfig::load(state.dirs.config_path())?;
    config.remove_schedule(&name)?;
    config.save(state.dirs.config_path())?;

    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Notification item for API responses.
#[derive(Debug, Serialize)]
pub struct NotificationItem {
    /// Unique notification ID.
    pub id: String,
    /// The schedule or source that produced this notification.
    pub source: String,
    /// Human-readable content.
    pub content: String,
    /// When the notification was created.
    pub created_at: String,
    /// Human feedback rating (null if unrated).
    pub rating: Option<String>,
}

/// `GET /notifications` — list all pending notifications.
pub async fn list_notifications(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Viewer)?;
    let store = open_notification_store(&state)?;
    let schedule_key = build_schedule_key(&state)?;

    let notifications = store.list(&schedule_key)?;
    let items: Vec<NotificationItem> = notifications
        .iter()
        .map(|n| NotificationItem {
            id: n.id.to_string(),
            source: n.source.clone(),
            content: n.content.clone(),
            created_at: n.created_at.to_rfc3339(),
            rating: n.rating.map(|r| r.to_string()),
        })
        .collect();

    Ok(axum::Json(items))
}

/// `DELETE /notifications` — drain all pending notifications.
pub async fn drain_notifications(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Operator)?;
    let store = open_notification_store(&state)?;
    let schedule_key = build_schedule_key(&state)?;

    let drained = store.drain(&schedule_key)?;

    // Audit log
    if !drained.is_empty() {
        let _ = state
            .audit_log
            .append(aivyx_audit::AuditEvent::NotificationsDrained {
                count: drained.len(),
            });
    }

    Ok(axum::Json(serde_json::json!({
        "drained": drained.len(),
    })))
}

/// Open the notification store from the schedules directory.
fn open_notification_store(state: &AppState) -> Result<NotificationStore, ServerError> {
    let notif_path = state.dirs.schedules_dir().join("notifications.db");
    NotificationStore::open(notif_path).map_err(ServerError::from)
}

/// Derive the schedule encryption key from the master key.
fn build_schedule_key(state: &AppState) -> Result<MasterKey, ServerError> {
    let key_bytes: [u8; 32] =
        state.master_key.expose_secret().try_into().map_err(|_| {
            ServerError(AivyxError::Crypto("master key byte length mismatch".into()))
        })?;
    Ok(derive_schedule_key(&MasterKey::from_bytes(key_bytes)))
}

// ─── Rating Endpoints ──────────────────────────────────────────────

/// Request body for `PUT /notifications/{id}/rating`.
#[derive(Debug, Deserialize)]
pub struct RateNotificationRequest {
    /// Rating value: "useful", "partial", or "useless".
    pub rating: Rating,
}

/// `PUT /notifications/{id}/rating` — rate a notification output.
///
/// Part of the agent feedback loop: human rates schedule outputs,
/// agents use these ratings during reflection to self-improve.
pub async fn rate_notification(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Path(id): Path<String>,
    axum::Json(req): axum::Json<RateNotificationRequest>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Operator)?;
    let store = open_notification_store(&state)?;
    let schedule_key = build_schedule_key(&state)?;

    let notif_id: aivyx_core::NotificationId = id
        .parse()
        .map_err(|_| ServerError(AivyxError::Other(format!("invalid notification ID: {id}"))))?;

    let updated = store
        .rate(&notif_id, req.rating, &schedule_key)
        .map_err(ServerError::from)?;

    // Audit
    let _ = state
        .audit_log
        .append(aivyx_audit::AuditEvent::NotificationRated {
            notification_id: id.clone(),
            rating: req.rating.to_string(),
        });

    let item = NotificationItem {
        id: updated.id.to_string(),
        source: updated.source,
        content: updated.content,
        created_at: updated.created_at.to_rfc3339(),
        rating: updated.rating.map(|r| r.to_string()),
    };

    Ok(axum::Json(item))
}

/// Query parameters for `GET /notifications/history`.
#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    /// Filter by source/agent name (partial match).
    pub agent: Option<String>,
    /// Filter by rating ("useful", "partial", "useless").
    pub rating: Option<Rating>,
    /// Maximum number of results (default 50).
    pub limit: Option<usize>,
}

/// `GET /notifications/history` — list rated notification history.
///
/// Used by the reflection schedule to review past outputs and their
/// human-assigned quality ratings. Supports filtering by agent and rating.
pub async fn notification_history(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    axum::extract::Query(query): axum::extract::Query<HistoryQuery>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Viewer)?;
    let store = open_notification_store(&state)?;
    let schedule_key = build_schedule_key(&state)?;

    let limit = query.limit.unwrap_or(50).min(200);
    let rated = store
        .list_rated(&schedule_key, query.agent.as_deref(), query.rating, limit)
        .map_err(ServerError::from)?;

    let items: Vec<NotificationItem> = rated
        .iter()
        .map(|n| NotificationItem {
            id: n.id.to_string(),
            source: n.source.clone(),
            content: n.content.clone(),
            created_at: n.created_at.to_rfc3339(),
            rating: n.rating.map(|r| r.to_string()),
        })
        .collect();

    Ok(axum::Json(items))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_summary_serializes() {
        let summary = ScheduleSummary {
            name: "morning-digest".into(),
            cron: "0 7 * * *".into(),
            agent: "assistant".into(),
            prompt: "Generate digest".into(),
            notify: true,
            enabled: true,
            last_run_at: None,
        };
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["name"], "morning-digest");
        assert_eq!(json["cron"], "0 7 * * *");
        assert!(json["notify"].as_bool().unwrap());
        assert!(json["last_run_at"].is_null());
    }

    #[test]
    fn create_schedule_request_deserializes() {
        let json = r#"{"name":"test","cron":"0 7 * * *","agent":"assistant","prompt":"hello"}"#;
        let req: CreateScheduleRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "test");
        assert!(req.notify); // default true
    }

    #[test]
    fn create_schedule_request_with_no_notify() {
        let json = r#"{"name":"test","cron":"* * * * *","agent":"a","prompt":"p","notify":false}"#;
        let req: CreateScheduleRequest = serde_json::from_str(json).unwrap();
        assert!(!req.notify);
    }

    #[test]
    fn notification_item_serializes() {
        let item = NotificationItem {
            id: "abc-123".into(),
            source: "morning-digest".into(),
            content: "All clear".into(),
            created_at: "2026-03-03T07:00:00Z".into(),
            rating: None,
        };
        let json = serde_json::to_value(&item).unwrap();
        assert_eq!(json["source"], "morning-digest");
        assert_eq!(json["content"], "All clear");
    }

    #[test]
    fn create_schedule_request_missing_required_field() {
        // Missing "prompt" field should fail
        let json = r#"{"name":"test","cron":"0 7 * * *","agent":"assistant"}"#;
        let result = serde_json::from_str::<CreateScheduleRequest>(json);
        assert!(result.is_err());
    }

    #[test]
    fn create_schedule_request_missing_name() {
        let json = r#"{"cron":"0 7 * * *","agent":"assistant","prompt":"hello"}"#;
        let result = serde_json::from_str::<CreateScheduleRequest>(json);
        assert!(result.is_err());
    }

    // validate_name tests are in crate::validation::tests

    #[test]
    fn schedule_summary_with_last_run() {
        let summary = ScheduleSummary {
            name: "nightly".into(),
            cron: "0 0 * * *".into(),
            agent: "ops".into(),
            prompt: "Run nightly checks".into(),
            notify: false,
            enabled: false,
            last_run_at: Some("2026-03-03T00:00:00Z".into()),
        };
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["last_run_at"], "2026-03-03T00:00:00Z");
        assert!(!json["notify"].as_bool().unwrap());
        assert!(!json["enabled"].as_bool().unwrap());
    }
}

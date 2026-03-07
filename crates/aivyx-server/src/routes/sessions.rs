//! Session management endpoints.
//!
//! `GET /sessions` — list all saved sessions.
//! `DELETE /sessions/:id` — delete a session.

use std::sync::Arc;

use aivyx_core::{AivyxError, SessionId};
use axum::extract::{Path, State};
use axum::response::IntoResponse;

use crate::app_state::AppState;
use crate::error::ServerError;

/// `GET /sessions` — list all saved sessions with metadata.
pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ServerError> {
    let sessions = state.session_store.list(&state.master_key)?;
    Ok(axum::Json(sessions))
}

/// `DELETE /sessions/:id` — delete a saved session.
pub async fn delete_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
    let session_id: SessionId = id
        .parse()
        .map_err(|_| ServerError(AivyxError::Config(format!("invalid session ID: {id}"))))?;
    state.session_store.delete(&session_id)?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_parse_valid_uuid() {
        let uuid_str = "550e8400-e29b-41d4-a716-446655440000";
        let result: Result<SessionId, _> = uuid_str.parse();
        assert!(result.is_ok());
    }

    #[test]
    fn session_id_parse_invalid_returns_err() {
        let bad = "not-a-uuid";
        let result: Result<SessionId, _> = bad.parse();
        assert!(result.is_err());
    }

    #[test]
    fn delete_session_rejects_invalid_id() {
        // Verify the ServerError construction works for invalid IDs
        let bad_id = "garbage";
        let err = ServerError(AivyxError::Config(format!("invalid session ID: {bad_id}")));
        let resp = err.into_response();
        assert_eq!(resp.status(), axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn delete_session_not_found_id_maps_correctly() {
        // A "not found" Config error maps to 404
        let err = ServerError(AivyxError::Config("session not found".into()));
        let resp = err.into_response();
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[test]
    fn session_id_new_produces_valid_uuid() {
        let id = SessionId::new();
        let s = id.to_string();
        // Should be a valid UUID string
        assert!(s.parse::<SessionId>().is_ok());
    }

    #[test]
    fn session_id_display_roundtrip() {
        let id = SessionId::new();
        let s = id.to_string();
        let parsed: SessionId = s.parse().unwrap();
        assert_eq!(id, parsed);
    }
}

//! Secret (API key) management endpoints.
//!
//! `GET /secrets` — list stored secret key names (not values).
//! `POST /secrets` — store a new secret.
//! `DELETE /secrets/{name}` — delete a secret.

use std::sync::Arc;

use aivyx_audit::AuditEvent;
use aivyx_crypto::EncryptedStore;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use crate::app_state::AppState;
use crate::error::ServerError;
use crate::validation::validate_secret_name;

/// Response for secret listing.
#[derive(Debug, Serialize)]
pub struct SecretListResponse {
    /// Key names stored in the encrypted store.
    pub keys: Vec<String>,
}

/// `GET /secrets` — list stored secret key names.
///
/// Returns only key names, never actual secret values.
pub async fn list_secrets(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ServerError> {
    let store = EncryptedStore::open(state.dirs.store_path())?;
    let keys = store.list_keys()?;

    Ok(axum::Json(SecretListResponse { keys }))
}

/// Request body for `POST /secrets`.
#[derive(Debug, Deserialize)]
pub struct SetSecretRequest {
    /// The key name (used as `api_key_ref` in provider config).
    pub name: String,
    /// The secret value to encrypt and store.
    pub value: String,
}

/// `POST /secrets` — store a new secret (or overwrite an existing one).
pub async fn set_secret(
    State(state): State<Arc<AppState>>,
    axum::Json(req): axum::Json<SetSecretRequest>,
) -> Result<impl IntoResponse, ServerError> {
    validate_secret_name(&req.name)?;

    let store = EncryptedStore::open(state.dirs.store_path())?;
    store.put(&req.name, req.value.as_bytes(), &state.master_key)?;

    // Audit
    if let Err(e) = state.audit_log.append(AuditEvent::SecretStored {
        key_name: req.name,
        changed_by: "api".into(),
    }) {
        tracing::warn!("failed to audit secret store: {e}");
    }

    Ok(axum::http::StatusCode::CREATED)
}

/// `DELETE /secrets/{name}` — delete a secret from the encrypted store.
pub async fn delete_secret(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
    validate_secret_name(&name)?;

    let store = EncryptedStore::open(state.dirs.store_path())?;
    store.delete(&name)?;

    // Audit
    if let Err(e) = state.audit_log.append(AuditEvent::SecretDeleted {
        key_name: name,
        changed_by: "api".into(),
    }) {
        tracing::warn!("failed to audit secret delete: {e}");
    }

    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_secret_request_deserializes() {
        let json = r#"{"name":"my-api-key","value":"sk-abc123"}"#;
        let req: SetSecretRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "my-api-key");
        assert_eq!(req.value, "sk-abc123");
    }

    #[test]
    fn secret_list_response_serializes() {
        let resp = SecretListResponse {
            keys: vec!["claude-key".into(), "openai-key".into()],
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["keys"][0], "claude-key");
    }

    // validate_secret_name tests are in crate::validation::tests
}

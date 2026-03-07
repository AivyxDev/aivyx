//! Administrative endpoints for server management.
//!
//! `POST /admin/rotate-token` — rotate the bearer token.

use std::sync::Arc;

use aivyx_audit::AuditEvent;
use aivyx_crypto::EncryptedStore;
use axum::extract::State;
use axum::response::IntoResponse;
use chrono::Utc;
use sha2::{Digest, Sha256};

use crate::app_state::AppState;
use crate::error::ServerError;
use crate::extractors::AuthContextExt;
use aivyx_tenant::AivyxRole;

/// Generate a cryptographically random 64-character hex token.
///
/// Uses two UUID v4s (backed by `OsRng`) to avoid adding `rand` as a
/// runtime dependency. Each UUID v4 provides 122 bits of entropy; two
/// concatenated give 244 bits — more than sufficient for a bearer token.
fn generate_random_token() -> String {
    let a = aivyx_core::SessionId::new();
    let b = aivyx_core::SessionId::new();
    format!(
        "{}{}",
        a.to_string().replace('-', ""),
        b.to_string().replace('-', "")
    )
}

/// `POST /admin/rotate-token` — generate a new bearer token.
///
/// Generates a fresh random bearer token, stores it in `EncryptedStore`,
/// and updates the in-memory hash atomically via `RwLock`. The old token
/// is immediately invalidated. Returns the new token in the response —
/// this is the **only** time the plaintext token is exposed.
pub async fn rotate_token(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Admin)?;
    let new_token = generate_random_token();

    // Store in EncryptedStore
    let store = EncryptedStore::open(state.dirs.store_path())?;
    store.put(
        "server-bearer-token",
        new_token.as_bytes(),
        &state.master_key,
    )?;

    // Update in-memory hash atomically
    let mut hasher = Sha256::new();
    hasher.update(new_token.as_bytes());
    let new_hash: [u8; 32] = hasher.finalize().into();
    *state.bearer_token_hash.write().await = new_hash;

    // Audit log
    if let Err(e) = state.audit_log.append(AuditEvent::BearerTokenRotated {
        timestamp: Utc::now(),
    }) {
        tracing::warn!("failed to audit token rotation: {e}");
    }

    Ok((
        axum::http::StatusCode::OK,
        axum::Json(serde_json::json!({
            "token": new_token,
            "message": "Bearer token rotated. Save this token — it will not be shown again."
        })),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_token_is_hex_and_long_enough() {
        let token = generate_random_token();
        assert!(token.len() >= 64);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn random_tokens_are_unique() {
        let a = generate_random_token();
        let b = generate_random_token();
        assert_ne!(a, b);
    }
}

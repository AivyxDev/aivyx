//! Inbound webhook trigger endpoints.
//!
//! `POST /webhooks/{trigger_name}` — receives webhook payloads and spawns
//! agent turns in the background.

use std::sync::Arc;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use crate::app_state::AppState;
use crate::error::ServerError;

/// `POST /webhooks/{trigger_name}` — receive a webhook and trigger an agent.
///
/// This is a PUBLIC endpoint (no auth required) — security is via HMAC signature.
/// When the trigger has `secret_ref` set, validates `X-Hub-Signature-256` header
/// using HMAC-SHA256. Returns 202 Accepted immediately, spawning the agent
/// in the background.
pub async fn receive_webhook(
    State(state): State<Arc<AppState>>,
    Path(trigger_name): Path<String>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, ServerError> {
    // Find the trigger config
    let config = state.config.read().await;
    let trigger = config.triggers.iter()
        .find(|t| t.name == trigger_name && t.enabled)
        .ok_or_else(|| ServerError(aivyx_core::AivyxError::Config(
            format!("webhook trigger not found: {trigger_name}")
        )))?
        .clone();
    drop(config);

    // HMAC verification if secret_ref is set
    if let Some(ref secret_ref) = trigger.secret_ref {
        let signature = headers
            .get("x-hub-signature-256")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| ServerError(aivyx_core::AivyxError::CapabilityDenied(
                "missing X-Hub-Signature-256 header".into()
            )))?;

        // Load secret from EncryptedStore
        let enc_store = aivyx_crypto::EncryptedStore::open(state.dirs.store_path())?;
        let secret_bytes = enc_store
            .get(secret_ref, &state.master_key)?
            .ok_or_else(|| ServerError(aivyx_core::AivyxError::Config(
                format!("webhook secret not found: {secret_ref}")
            )))?;

        // Verify HMAC-SHA256
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        type HmacSha256 = Hmac<Sha256>;

        let mut mac = HmacSha256::new_from_slice(&secret_bytes)
            .map_err(|e| ServerError(aivyx_core::AivyxError::Crypto(e.to_string())))?;
        mac.update(&body);

        // Signature format: "sha256=hex_digest"
        let expected = signature.strip_prefix("sha256=")
            .ok_or_else(|| ServerError(aivyx_core::AivyxError::CapabilityDenied(
                "invalid signature format".into()
            )))?;
        let expected_bytes = hex::decode(expected)
            .map_err(|_| ServerError(aivyx_core::AivyxError::CapabilityDenied(
                "invalid signature hex".into()
            )))?;

        mac.verify_slice(&expected_bytes)
            .map_err(|_| ServerError(aivyx_core::AivyxError::CapabilityDenied(
                "HMAC signature verification failed".into()
            )))?;
    }

    // Build prompt from template — sanitize payload to prevent prompt injection
    let payload_str = String::from_utf8_lossy(&body);
    let sanitized_payload = aivyx_agent::sanitize::sanitize_webhook_payload(&payload_str);
    let prompt = trigger.prompt_template.replace("{payload}", &sanitized_payload);

    // Spawn agent in background
    let state_clone = state.clone();
    let agent_name = trigger.agent.clone();
    tokio::spawn(async move {
        match state_clone.agent_session.create_agent_with_context(&agent_name, None).await {
            Ok(mut agent) => {
                if let Err(e) = agent.turn(&prompt, None).await {
                    tracing::error!("webhook agent '{}' failed: {e}", agent_name);
                }
            }
            Err(e) => {
                tracing::error!("failed to create webhook agent '{}': {e}", agent_name);
            }
        }
    });

    Ok((StatusCode::ACCEPTED, axum::Json(serde_json::json!({
        "status": "accepted",
        "trigger": trigger_name,
    }))))
}

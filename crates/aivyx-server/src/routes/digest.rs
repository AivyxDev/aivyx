//! Digest generation endpoint.
//!
//! `POST /digest` — generate an on-demand daily digest via a direct LLM call.

use std::sync::Arc;

use aivyx_core::AivyxError;
use aivyx_crypto::{EncryptedStore, MasterKey, derive_schedule_key};
use aivyx_memory::NotificationStore;
use axum::extract::State;
use axum::response::IntoResponse;
use serde::Deserialize;

use crate::app_state::AppState;
use crate::error::ServerError;
use crate::extractors::AuthContextExt;
use aivyx_tenant::AivyxRole;

/// Request body for `POST /digest`.
#[derive(Debug, Deserialize)]
pub struct DigestRequest {
    /// Agent profile name (defaults to "assistant").
    #[serde(default = "default_agent")]
    pub agent: String,
}

fn default_agent() -> String {
    "assistant".into()
}

/// `POST /digest` — generate an on-demand daily digest.
///
/// Drains pending notifications and uses them as context for the digest.
/// Returns the generated digest text.
pub async fn generate_digest(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    axum::Json(req): axum::Json<DigestRequest>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Operator)?;
    // Drain notifications for context
    let mut notification_context = String::new();
    let notif_path = state.dirs.schedules_dir().join("notifications.db");

    if notif_path.exists() {
        let key_bytes: [u8; 32] = state.master_key.expose_secret().try_into().map_err(|_| {
            ServerError(AivyxError::Crypto("master key byte length mismatch".into()))
        })?;
        let schedule_key = derive_schedule_key(&MasterKey::from_bytes(key_bytes));
        let store = NotificationStore::open(&notif_path)?;
        let notifications = store.drain(&schedule_key)?;

        if !notifications.is_empty() {
            let _ = state
                .audit_log
                .append(aivyx_audit::AuditEvent::NotificationsDrained {
                    count: notifications.len(),
                });

            for n in &notifications {
                notification_context.push_str(&format!("- {}: {}\n", n.source, n.content));
            }
        }
    }

    // Build the digest prompt
    let prompt = if notification_context.is_empty() {
        "Generate a daily digest. Summarize any relevant context.".to_string()
    } else {
        format!(
            "Generate a daily digest. Here are findings from background activity:\n\
             {notification_context}\n\
             Incorporate these findings into a concise briefing."
        )
    };

    // Resolve the LLM provider
    let profile_path = state.dirs.agents_dir().join(format!("{}.toml", req.agent));
    let profile = aivyx_agent::AgentProfile::load(&profile_path).map_err(|_| {
        ServerError(AivyxError::Config(format!(
            "agent profile not found: {}",
            req.agent
        )))
    })?;
    let config = state.config.read().await;
    let provider_config = config.resolve_provider(profile.provider.as_deref());
    let secrets_store = EncryptedStore::open(state.dirs.store_path())?;
    let provider = aivyx_llm::create_provider(provider_config, &secrets_store, &state.master_key)?;

    // Generate digest via direct LLM call
    let digest = aivyx_agent::generate_digest(provider.as_ref(), &prompt, None).await?;

    Ok(axum::Json(serde_json::json!({
        "digest": digest,
        "agent": req.agent,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_request_defaults() {
        let json = r#"{}"#;
        let req: DigestRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.agent, "assistant");
    }

    #[test]
    fn digest_request_with_agent() {
        let json = r#"{"agent":"researcher"}"#;
        let req: DigestRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.agent, "researcher");
    }

    #[test]
    fn digest_request_ignores_unknown_fields() {
        let json = r#"{"agent":"coder","extra":"ignored"}"#;
        let req: DigestRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.agent, "coder");
    }

    #[test]
    fn digest_request_empty_agent_string() {
        let json = r#"{"agent":""}"#;
        let req: DigestRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.agent, "");
    }
}

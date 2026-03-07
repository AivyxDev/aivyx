//! CLI handler for on-demand daily digest generation.

use aivyx_agent::AgentProfile;
use aivyx_config::{AivyxConfig, AivyxDirs};
use aivyx_core::{AivyxError, Result};
use aivyx_crypto::{EncryptedStore, MasterKey, derive_schedule_key};
use aivyx_memory::NotificationStore;

use crate::output;

/// Generate and display an on-demand daily digest.
///
/// If pending notifications exist, they are drained and displayed first,
/// then a digest is generated via a direct LLM call.
pub async fn run(agent: Option<&str>) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let master_key = unlock_master_key(&dirs)?;
    let config = AivyxConfig::load(dirs.config_path())?;

    let agent_name = agent.unwrap_or("assistant");

    // Drain pending notifications (if any)
    let notif_db = dirs.schedules_dir().join("notifications.db");
    let mut notification_context = String::new();

    if notif_db.exists() {
        let key_bytes: [u8; 32] = master_key
            .expose_secret()
            .try_into()
            .map_err(|_| AivyxError::Crypto("master key byte length mismatch".into()))?;
        let schedule_key = derive_schedule_key(&MasterKey::from_bytes(key_bytes));
        let store = NotificationStore::open(&notif_db)?;
        let notifications = store.drain(&schedule_key)?;

        if !notifications.is_empty() {
            output::header(&format!("Background findings ({})", notifications.len()));
            println!();

            for (i, n) in notifications.iter().enumerate() {
                let ts = n.created_at.format("%Y-%m-%d %H:%M");
                println!("  {}. [{}] {}: {}", i + 1, ts, n.source, n.content);
                notification_context.push_str(&format!("- {}: {}\n", n.source, n.content));
            }
            println!();
        }
    }

    // Build the digest prompt
    let prompt = if notification_context.is_empty() {
        "Generate a daily digest based on what you know about the user. \
         Summarize any relevant context."
            .to_string()
    } else {
        format!(
            "Generate a daily digest. Here are findings from background activity:\n\
             {notification_context}\n\
             Incorporate these findings into a concise briefing."
        )
    };

    // Resolve the LLM provider for the specified agent
    let profile_path = dirs.agents_dir().join(format!("{agent_name}.toml"));
    let profile = AgentProfile::load(&profile_path)
        .map_err(|_| AivyxError::Config(format!("agent profile not found: {agent_name}")))?;
    let provider_config = config.resolve_provider(profile.provider.as_deref());
    let secrets_store = EncryptedStore::open(dirs.store_path())?;
    let provider = aivyx_llm::create_provider(provider_config, &secrets_store, &master_key)?;

    output::header("Daily Digest");
    println!();

    let digest = aivyx_agent::generate_digest(
        provider.as_ref(),
        &prompt,
        None, // Memory context would require a full agent turn; digest uses direct LLM call
    )
    .await?;

    println!("{digest}");
    println!();

    Ok(())
}

fn check_initialized(dirs: &AivyxDirs) -> Result<()> {
    if !dirs.is_initialized() {
        output::error("aivyx is not initialized. Run `aivyx genesis` to get started.");
        return Err(AivyxError::NotInitialized(
            "run `aivyx genesis` first".into(),
        ));
    }
    Ok(())
}

fn unlock_master_key(dirs: &AivyxDirs) -> Result<MasterKey> {
    crate::unlock::unlock_master_key(dirs)
}

use aivyx_agent::SessionStore;
use aivyx_config::AivyxDirs;
use aivyx_core::{AivyxError, Result, SessionId};
use aivyx_crypto::MasterKey;

use crate::output;

pub fn list() -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;
    let master_key = unlock_master_key(&dirs)?;

    let store = SessionStore::open(dirs.sessions_dir().join("sessions.db"))?;
    let sessions = store.list(&master_key)?;

    if sessions.is_empty() {
        println!("  No saved sessions.");
        return Ok(());
    }

    output::header("Saved Sessions");
    println!();

    for meta in &sessions {
        output::kv("  Session", &meta.session_id.to_string());
        output::kv("  Agent", &meta.agent_name);
        output::kv("  Messages", &meta.message_count.to_string());
        output::kv(
            "  Updated",
            &meta.updated_at.format("%Y-%m-%d %H:%M:%S").to_string(),
        );
        println!();
    }

    Ok(())
}

pub fn delete(id: &str) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let session_id: SessionId = id
        .parse()
        .map_err(|_| AivyxError::Config(format!("invalid session ID: {id}")))?;

    let store = SessionStore::open(dirs.sessions_dir().join("sessions.db"))?;
    store.delete(&session_id)?;

    output::success(&format!("Deleted session {id}"));
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

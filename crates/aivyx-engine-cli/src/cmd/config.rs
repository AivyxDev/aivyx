use aivyx_audit::{AuditEvent, AuditLog};
use aivyx_config::{AivyxConfig, AivyxDirs};
use aivyx_core::{Principal, Result};
use aivyx_crypto::{MasterKey, derive_audit_key};
use sha2::{Digest, Sha256};

use crate::output;

pub fn get(key: &str) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    if !dirs.is_initialized() {
        output::error("aivyx is not initialized. Run `aivyx genesis` to get started.");
        return Ok(());
    }

    let config = AivyxConfig::load(dirs.config_path())?;
    match config.get(key) {
        Some(value) => println!("{value}"),
        None => output::error(&format!("unknown config key: {key}")),
    }
    Ok(())
}

pub fn set(key: &str, value: &str) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    if !dirs.is_initialized() {
        output::error("aivyx is not initialized. Run `aivyx genesis` to get started.");
        return Ok(());
    }

    let mut config = AivyxConfig::load(dirs.config_path())?;

    // Capture old value hash for audit.
    let old_value_hash = config
        .get(key)
        .map(|v| hex::encode(Sha256::digest(v.as_bytes())))
        .unwrap_or_default();

    config.set(key, value)?;
    config.save(dirs.config_path())?;
    output::success(&format!("{key} = {value}"));

    // Audit the config change.
    let new_value_hash = hex::encode(Sha256::digest(value.as_bytes()));
    let master_key = unlock_master_key(&dirs)?;
    let audit_key = derive_audit_key(&master_key);
    let log = AuditLog::new(dirs.audit_path(), &audit_key);
    log.append(AuditEvent::ConfigChanged {
        key: key.to_string(),
        old_value_hash,
        new_value_hash,
        changed_by: Principal::User("cli".into()),
    })?;

    Ok(())
}

fn unlock_master_key(dirs: &AivyxDirs) -> Result<MasterKey> {
    crate::unlock::unlock_master_key(dirs)
}

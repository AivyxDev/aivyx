use aivyx_config::AivyxDirs;
use aivyx_core::Result;
use aivyx_crypto::{EncryptedStore, MasterKey};

use crate::output;

pub fn set(name: &str) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let master_key = unlock_master_key(&dirs)?;
    let store = EncryptedStore::open(dirs.store_path())?;

    let value = rpassword::prompt_password(format!("  Enter value for '{name}': "))
        .map_err(|e| aivyx_core::AivyxError::Other(format!("failed to read value: {e}")))?;

    if value.is_empty() {
        output::error("value cannot be empty");
        return Ok(());
    }

    store.put(name, value.as_bytes(), &master_key)?;
    output::success(&format!("stored secret '{name}'"));
    Ok(())
}

pub fn get(name: &str) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let master_key = unlock_master_key(&dirs)?;
    let store = EncryptedStore::open(dirs.store_path())?;

    match store.get(name, &master_key)? {
        Some(bytes) => {
            let value = String::from_utf8_lossy(&bytes);
            let masked = mask_value(&value);
            println!("  {name} = {masked}");
        }
        None => {
            output::error(&format!("secret '{name}' not found"));
        }
    }
    Ok(())
}

pub fn list() -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let store = EncryptedStore::open(dirs.store_path())?;
    let keys = store.list_keys()?;

    if keys.is_empty() {
        println!("  No secrets stored.");
        return Ok(());
    }

    output::header("Stored secrets");
    for key in &keys {
        println!("  {key}");
    }
    println!();
    Ok(())
}

pub fn delete(name: &str) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let store = EncryptedStore::open(dirs.store_path())?;
    store.delete(name)?;
    output::success(&format!("deleted secret '{name}'"));
    Ok(())
}

/// Show first 4 characters followed by ****.
fn mask_value(value: &str) -> String {
    let prefix: String = value.chars().take(4).collect();
    if value.chars().count() <= 4 {
        "****".to_string()
    } else {
        format!("{prefix}****")
    }
}

fn check_initialized(dirs: &AivyxDirs) -> Result<()> {
    if !dirs.is_initialized() {
        output::error("aivyx is not initialized. Run `aivyx genesis` to get started.");
        return Err(aivyx_core::AivyxError::NotInitialized(
            "run `aivyx genesis` first".into(),
        ));
    }
    Ok(())
}

fn unlock_master_key(dirs: &AivyxDirs) -> Result<MasterKey> {
    crate::unlock::unlock_master_key(dirs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_short_value() {
        assert_eq!(mask_value("ab"), "****");
    }

    #[test]
    fn mask_long_value() {
        assert_eq!(mask_value("sk-1234567890"), "sk-1****");
    }
}

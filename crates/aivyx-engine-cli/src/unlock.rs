//! Centralized master key unlocking with process-level caching.
//!
//! All CLI commands that need the master key call [`unlock_master_key()`] from
//! this module. The passphrase is prompted once and the decrypted key bytes are
//! cached for the process lifetime. Cached bytes are zeroed on process exit via
//! [`zeroize::Zeroizing`].
//!
//! Passphrase source priority:
//! 1. `AIVYX_PASSPHRASE` environment variable
//! 2. Interactive prompt via `rpassword`

use std::sync::Mutex;

use aivyx_config::AivyxDirs;
use aivyx_core::{AivyxError, Result};
use aivyx_crypto::{MasterKey, MasterKeyEnvelope};
use zeroize::Zeroizing;

/// Process-level cache for the decrypted master key bytes.
///
/// Uses `Mutex<Option<...>>` so that failed attempts (e.g. wrong passphrase)
/// leave the cache empty, allowing the caller to retry. On success the bytes
/// are cached and all subsequent calls return immediately.
static CACHED_KEY_BYTES: Mutex<Option<Zeroizing<[u8; 32]>>> = Mutex::new(None);

/// Unlock the master key, caching the result for subsequent calls within
/// the same process.
///
/// Returns a fresh [`MasterKey`] constructed from the cached bytes on each
/// call (since `MasterKey` is not `Clone`).
pub fn unlock_master_key(dirs: &AivyxDirs) -> Result<MasterKey> {
    let mut guard = CACHED_KEY_BYTES.lock().unwrap_or_else(|e| e.into_inner());

    if let Some(ref bytes) = *guard {
        return Ok(MasterKey::from_bytes(**bytes));
    }

    // Not yet cached — decrypt from disk.
    let cached = decrypt_and_cache(dirs)?;
    let key = MasterKey::from_bytes(*cached);
    *guard = Some(cached);
    Ok(key)
}

/// Alias for [`unlock_master_key()`] — preserved for call sites that need
/// both the raw key and a domain-derived key (e.g., memory commands that
/// call `derive_memory_key()` on the result).
pub fn unlock_raw_master_key(dirs: &AivyxDirs) -> Result<MasterKey> {
    unlock_master_key(dirs)
}

/// Decrypt the master key from the on-disk envelope and return the raw bytes.
fn decrypt_and_cache(dirs: &AivyxDirs) -> Result<Zeroizing<[u8; 32]>> {
    let envelope_json = std::fs::read_to_string(dirs.master_key_path())?;
    let envelope: MasterKeyEnvelope = serde_json::from_str(&envelope_json)
        .map_err(|e| AivyxError::Crypto(format!("invalid master key envelope: {e}")))?;

    // Priority 1: AIVYX_PASSPHRASE env var (works for all commands, not just server)
    let passphrase = if let Ok(p) = std::env::var("AIVYX_PASSPHRASE") {
        p
    } else {
        // Priority 2: Interactive prompt
        rpassword::prompt_password("  Enter passphrase: ")
            .map_err(|e| AivyxError::Other(format!("failed to read passphrase: {e}")))?
    };

    let master_key = MasterKey::decrypt_from_envelope(passphrase.as_bytes(), &envelope)?;
    let raw_bytes: [u8; 32] = master_key
        .expose_secret()
        .try_into()
        .map_err(|_| AivyxError::Crypto("master key is not 32 bytes".into()))?;
    Ok(Zeroizing::new(raw_bytes))
}

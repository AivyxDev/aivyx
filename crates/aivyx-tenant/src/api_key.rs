//! API key management for multi-tenant authentication.
//!
//! Each tenant can have multiple API keys. Keys are stored as SHA-256 hashes
//! in `EncryptedStore`, keyed by `"apikey:{8-char-hex-prefix}"`. Lookup is
//! O(1) by hash prefix with a small bucket scan for collision resolution.

use std::path::Path;

use aivyx_core::{AivyxError, Result, TenantId};
use aivyx_crypto::{EncryptedStore, MasterKey};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::rbac::AivyxRole;

/// Metadata for a single API key (the plaintext is never stored).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyRecord {
    /// Unique key identifier (UUID).
    pub key_id: String,
    /// Human-readable label.
    pub name: String,
    /// SHA-256 hash of the plaintext token.
    pub sha256_hash: Vec<u8>,
    /// Tenant this key belongs to.
    pub tenant_id: TenantId,
    /// RBAC role granted by this key.
    pub role: AivyxRole,
    /// Permitted scopes (empty = all).
    pub scopes: Vec<ApiKeyScope>,
    /// When the key was created.
    pub created_at: DateTime<Utc>,
    /// When the key expires (None = never).
    pub expires_at: Option<DateTime<Utc>>,
    /// When the key was last used.
    pub last_used_at: Option<DateTime<Utc>>,
    /// Whether the key has been revoked.
    pub revoked: bool,
}

/// Scopes that can be assigned to an API key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApiKeyScope {
    /// Chat and LLM inference.
    Chat,
    /// Task creation and management.
    Tasks,
    /// Memory read/write.
    Memory,
    /// Administrative operations.
    Admin,
    /// All scopes.
    All,
}

/// Key prefix for API key records in `EncryptedStore`.
const APIKEY_PREFIX: &str = "apikey:";

/// Persistent API key store backed by `EncryptedStore`.
pub struct ApiKeyStore {
    inner: EncryptedStore,
}

impl ApiKeyStore {
    /// Open (or create) an API key store at the given path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            inner: EncryptedStore::open(path)?,
        })
    }

    /// Create a new API key for a tenant.
    ///
    /// Returns `(plaintext_token, record)`. The plaintext token is only returned
    /// once — it must be saved by the caller.
    pub fn create_key(
        &self,
        tenant_id: TenantId,
        name: &str,
        role: AivyxRole,
        scopes: Vec<ApiKeyScope>,
        expires_at: Option<DateTime<Utc>>,
        master_key: &MasterKey,
    ) -> Result<(String, ApiKeyRecord)> {
        // Generate a random token (two UUID v4s = 244 bits of entropy)
        let token = format!(
            "aivyx_{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple(),
        );

        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        let hash: Vec<u8> = hasher.finalize().to_vec();

        let record = ApiKeyRecord {
            key_id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            sha256_hash: hash.clone(),
            tenant_id,
            role,
            scopes,
            created_at: Utc::now(),
            expires_at,
            last_used_at: None,
            revoked: false,
        };

        // Store under the hash prefix for O(1) lookup
        let prefix = hex::encode(&hash[..4]);
        let store_key = format!("{}{}", APIKEY_PREFIX, prefix);

        // Load existing bucket (may have other keys with same prefix)
        let mut bucket = self.load_bucket(&store_key, master_key)?;
        bucket.push(record.clone());
        self.save_bucket(&store_key, &bucket, master_key)?;

        Ok((token, record))
    }

    /// Look up an API key by its plaintext token.
    ///
    /// Returns `None` if the token doesn't match any key, the key is revoked,
    /// or the key has expired.
    pub fn lookup_by_token(
        &self,
        token: &str,
        master_key: &MasterKey,
    ) -> Result<Option<ApiKeyRecord>> {
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        let hash: Vec<u8> = hasher.finalize().to_vec();

        let prefix = hex::encode(&hash[..4]);
        let store_key = format!("{}{}", APIKEY_PREFIX, prefix);

        let bucket = self.load_bucket(&store_key, master_key)?;
        let now = Utc::now();

        for record in bucket {
            if record.sha256_hash == hash {
                if record.revoked {
                    return Ok(None);
                }
                if record.expires_at.is_some_and(|exp| now > exp) {
                    return Ok(None);
                }
                return Ok(Some(record));
            }
        }

        Ok(None)
    }

    /// List all keys for a tenant (metadata only, no secrets).
    pub fn list_keys(
        &self,
        tenant_id: &TenantId,
        master_key: &MasterKey,
    ) -> Result<Vec<ApiKeyRecord>> {
        let all_keys = self.inner.list_keys()?;
        let mut result = Vec::new();

        for k in all_keys {
            if k.starts_with(APIKEY_PREFIX) {
                let bucket = self.load_bucket(&k, master_key)?;
                for record in bucket {
                    if record.tenant_id == *tenant_id && !record.revoked {
                        result.push(record);
                    }
                }
            }
        }

        Ok(result)
    }

    /// Revoke a key by its key_id.
    pub fn revoke_key(&self, key_id: &str, master_key: &MasterKey) -> Result<()> {
        let all_keys = self.inner.list_keys()?;

        for k in all_keys {
            if k.starts_with(APIKEY_PREFIX) {
                let mut bucket = self.load_bucket(&k, master_key)?;
                let mut modified = false;

                for record in &mut bucket {
                    if record.key_id == key_id {
                        record.revoked = true;
                        modified = true;
                    }
                }

                if modified {
                    self.save_bucket(&k, &bucket, master_key)?;
                    return Ok(());
                }
            }
        }

        Err(AivyxError::Config(format!("API key not found: {key_id}")))
    }

    fn load_bucket(&self, key: &str, master_key: &MasterKey) -> Result<Vec<ApiKeyRecord>> {
        match self.inner.get(key, master_key)? {
            Some(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| AivyxError::Storage(format!("apikey bucket deserialize: {e}"))),
            None => Ok(Vec::new()),
        }
    }

    fn save_bucket(
        &self,
        key: &str,
        bucket: &[ApiKeyRecord],
        master_key: &MasterKey,
    ) -> Result<()> {
        let json = serde_json::to_vec(bucket)
            .map_err(|e| AivyxError::Storage(format!("apikey bucket serialize: {e}")))?;
        self.inner.put(key, &json, master_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (ApiKeyStore, MasterKey) {
        let dir = tempfile::tempdir().unwrap();
        let store = ApiKeyStore::open(dir.path().join("apikeys.db")).unwrap();
        let key = MasterKey::from_bytes([42u8; 32]);
        (store, key)
    }

    #[test]
    fn create_and_lookup_key() {
        let (store, mk) = temp_store();
        let tid = TenantId::new();
        let (token, record) = store
            .create_key(
                tid,
                "test-key",
                AivyxRole::Operator,
                vec![ApiKeyScope::All],
                None,
                &mk,
            )
            .unwrap();

        assert!(token.starts_with("aivyx_"));
        assert_eq!(record.name, "test-key");
        assert_eq!(record.role, AivyxRole::Operator);

        let found = store.lookup_by_token(&token, &mk).unwrap().unwrap();
        assert_eq!(found.key_id, record.key_id);
        assert_eq!(found.tenant_id, tid);
    }

    #[test]
    fn lookup_nonexistent_returns_none() {
        let (store, mk) = temp_store();
        let result = store.lookup_by_token("aivyx_bogustoken", &mk).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn revoked_key_returns_none() {
        let (store, mk) = temp_store();
        let tid = TenantId::new();
        let (token, record) = store
            .create_key(
                tid,
                "revoke-me",
                AivyxRole::Viewer,
                vec![],
                None,
                &mk,
            )
            .unwrap();

        store.revoke_key(&record.key_id, &mk).unwrap();

        let result = store.lookup_by_token(&token, &mk).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn expired_key_returns_none() {
        let (store, mk) = temp_store();
        let tid = TenantId::new();
        // Create key that expired yesterday
        let yesterday = Utc::now() - chrono::Duration::days(1);
        let (token, _) = store
            .create_key(
                tid,
                "expired",
                AivyxRole::Viewer,
                vec![],
                Some(yesterday),
                &mk,
            )
            .unwrap();

        let result = store.lookup_by_token(&token, &mk).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn list_keys_filters_by_tenant() {
        let (store, mk) = temp_store();
        let tid_a = TenantId::new();
        let tid_b = TenantId::new();

        store
            .create_key(tid_a, "key-a1", AivyxRole::Operator, vec![], None, &mk)
            .unwrap();
        store
            .create_key(tid_a, "key-a2", AivyxRole::Viewer, vec![], None, &mk)
            .unwrap();
        store
            .create_key(tid_b, "key-b1", AivyxRole::Admin, vec![], None, &mk)
            .unwrap();

        let keys_a = store.list_keys(&tid_a, &mk).unwrap();
        assert_eq!(keys_a.len(), 2);

        let keys_b = store.list_keys(&tid_b, &mk).unwrap();
        assert_eq!(keys_b.len(), 1);
    }

    #[test]
    fn list_keys_excludes_revoked() {
        let (store, mk) = temp_store();
        let tid = TenantId::new();

        let (_, r1) = store
            .create_key(tid, "keep", AivyxRole::Operator, vec![], None, &mk)
            .unwrap();
        let (_, r2) = store
            .create_key(tid, "revoke", AivyxRole::Viewer, vec![], None, &mk)
            .unwrap();

        store.revoke_key(&r2.key_id, &mk).unwrap();

        let keys = store.list_keys(&tid, &mk).unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key_id, r1.key_id);
    }

    #[test]
    fn api_key_scope_serde_roundtrip() {
        for scope in [
            ApiKeyScope::Chat,
            ApiKeyScope::Tasks,
            ApiKeyScope::Memory,
            ApiKeyScope::Admin,
            ApiKeyScope::All,
        ] {
            let json = serde_json::to_string(&scope).unwrap();
            let parsed: ApiKeyScope = serde_json::from_str(&json).unwrap();
            assert_eq!(scope, parsed);
        }
    }
}

//! Persistent tenant storage backed by `EncryptedStore`.
//!
//! Each tenant is stored as `"tenant:{uuid}"` → serialized [`TenantRecord`].

use std::path::Path;

use aivyx_core::{AivyxError, Result, TenantId};
use aivyx_crypto::{EncryptedStore, MasterKey};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Persistent metadata for a registered tenant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantRecord {
    /// Unique tenant identifier.
    pub id: TenantId,
    /// Human-readable name.
    pub name: String,
    /// Current lifecycle status.
    pub status: TenantStatus,
    /// Resource quotas for this tenant.
    pub quotas: ResourceQuotas,
    /// When the tenant was created.
    pub created_at: DateTime<Utc>,
    /// When the tenant was last modified.
    pub updated_at: DateTime<Utc>,
}

/// Tenant lifecycle status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TenantStatus {
    /// Tenant is active and can make requests.
    Active,
    /// Tenant is suspended — API keys are disabled, requests are rejected.
    Suspended {
        /// Why the tenant was suspended.
        reason: String,
    },
    /// Tenant is soft-deleted — data retained but inaccessible.
    Deleted,
}

/// Configurable resource limits per tenant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceQuotas {
    /// Maximum number of agents the tenant can create.
    #[serde(default)]
    pub max_agents: Option<u32>,
    /// Maximum sessions the tenant can create per day.
    #[serde(default)]
    pub max_sessions_per_day: Option<u32>,
    /// Maximum storage in megabytes.
    #[serde(default)]
    pub max_storage_mb: Option<u64>,
    /// Maximum LLM tokens per day.
    #[serde(default)]
    pub max_llm_tokens_per_day: Option<u64>,
    /// Maximum LLM tokens per month.
    #[serde(default)]
    pub max_llm_tokens_per_month: Option<u64>,
}

impl Default for ResourceQuotas {
    fn default() -> Self {
        Self {
            max_agents: None,
            max_sessions_per_day: None,
            max_storage_mb: None,
            max_llm_tokens_per_day: None,
            max_llm_tokens_per_month: None,
        }
    }
}

/// Key prefix for tenant records in `EncryptedStore`.
const TENANT_PREFIX: &str = "tenant:";

/// Persistent tenant store backed by `EncryptedStore`.
pub struct TenantStore {
    inner: EncryptedStore,
}

impl TenantStore {
    /// Open (or create) a tenant store at the given path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            inner: EncryptedStore::open(path)?,
        })
    }

    /// Create a new tenant with the given name and quotas.
    pub fn create_tenant(
        &self,
        name: &str,
        quotas: ResourceQuotas,
        key: &MasterKey,
    ) -> Result<TenantRecord> {
        let now = Utc::now();
        let record = TenantRecord {
            id: TenantId::new(),
            name: name.to_string(),
            status: TenantStatus::Active,
            quotas,
            created_at: now,
            updated_at: now,
        };

        let store_key = format!("{}{}", TENANT_PREFIX, record.id);
        let json = serde_json::to_vec(&record)
            .map_err(|e| AivyxError::Storage(format!("failed to serialize tenant: {e}")))?;
        self.inner.put(&store_key, &json, key)?;

        Ok(record)
    }

    /// Get a tenant by ID.
    pub fn get_tenant(&self, id: &TenantId, key: &MasterKey) -> Result<Option<TenantRecord>> {
        let store_key = format!("{}{}", TENANT_PREFIX, id);
        match self.inner.get(&store_key, key)? {
            Some(bytes) => {
                let record: TenantRecord = serde_json::from_slice(&bytes)
                    .map_err(|e| AivyxError::Storage(format!("tenant deserialize: {e}")))?;
                Ok(Some(record))
            }
            None => Ok(None),
        }
    }

    /// List all tenants (non-deleted).
    pub fn list_tenants(&self, key: &MasterKey) -> Result<Vec<TenantRecord>> {
        let all_keys = self.inner.list_keys()?;
        let mut tenants = Vec::new();
        for k in all_keys {
            if let Some(_id_str) = k.strip_prefix(TENANT_PREFIX) {
                if let Some(bytes) = self.inner.get(&k, key)? {
                    if let Ok(record) = serde_json::from_slice::<TenantRecord>(&bytes) {
                        if record.status != TenantStatus::Deleted {
                            tenants.push(record);
                        }
                    }
                }
            }
        }
        Ok(tenants)
    }

    /// Update a tenant's status.
    pub fn update_status(
        &self,
        id: &TenantId,
        status: TenantStatus,
        key: &MasterKey,
    ) -> Result<()> {
        let store_key = format!("{}{}", TENANT_PREFIX, id);
        let bytes = self
            .inner
            .get(&store_key, key)?
            .ok_or_else(|| AivyxError::Config(format!("tenant not found: {id}")))?;
        let mut record: TenantRecord = serde_json::from_slice(&bytes)
            .map_err(|e| AivyxError::Storage(format!("tenant deserialize: {e}")))?;

        record.status = status;
        record.updated_at = Utc::now();

        let json = serde_json::to_vec(&record)
            .map_err(|e| AivyxError::Storage(format!("failed to serialize tenant: {e}")))?;
        self.inner.put(&store_key, &json, key)?;
        Ok(())
    }

    /// Soft-delete a tenant.
    pub fn delete_tenant(&self, id: &TenantId, key: &MasterKey) -> Result<()> {
        self.update_status(id, TenantStatus::Deleted, key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (TenantStore, MasterKey) {
        let dir = tempfile::tempdir().unwrap();
        let store = TenantStore::open(dir.path().join("tenants.db")).unwrap();
        let key = MasterKey::from_bytes([42u8; 32]);
        (store, key)
    }

    #[test]
    fn create_and_get_tenant() {
        let (store, key) = temp_store();
        let record = store
            .create_tenant("Acme Corp", ResourceQuotas::default(), &key)
            .unwrap();
        assert_eq!(record.name, "Acme Corp");
        assert_eq!(record.status, TenantStatus::Active);

        let fetched = store.get_tenant(&record.id, &key).unwrap().unwrap();
        assert_eq!(fetched.id, record.id);
        assert_eq!(fetched.name, "Acme Corp");
    }

    #[test]
    fn list_tenants_excludes_deleted() {
        let (store, key) = temp_store();
        let t1 = store
            .create_tenant("Tenant A", ResourceQuotas::default(), &key)
            .unwrap();
        let t2 = store
            .create_tenant("Tenant B", ResourceQuotas::default(), &key)
            .unwrap();

        store.delete_tenant(&t1.id, &key).unwrap();

        let tenants = store.list_tenants(&key).unwrap();
        assert_eq!(tenants.len(), 1);
        assert_eq!(tenants[0].id, t2.id);
    }

    #[test]
    fn suspend_and_unsuspend() {
        let (store, key) = temp_store();
        let record = store
            .create_tenant("Test", ResourceQuotas::default(), &key)
            .unwrap();

        store
            .update_status(
                &record.id,
                TenantStatus::Suspended {
                    reason: "payment overdue".into(),
                },
                &key,
            )
            .unwrap();

        let fetched = store.get_tenant(&record.id, &key).unwrap().unwrap();
        assert!(matches!(fetched.status, TenantStatus::Suspended { .. }));

        store
            .update_status(&record.id, TenantStatus::Active, &key)
            .unwrap();
        let fetched = store.get_tenant(&record.id, &key).unwrap().unwrap();
        assert_eq!(fetched.status, TenantStatus::Active);
    }

    #[test]
    fn tenant_record_serde_roundtrip() {
        let record = TenantRecord {
            id: TenantId::new(),
            name: "Test Corp".into(),
            status: TenantStatus::Active,
            quotas: ResourceQuotas {
                max_agents: Some(10),
                max_sessions_per_day: Some(100),
                ..Default::default()
            },
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let json = serde_json::to_string(&record).unwrap();
        let parsed: TenantRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, record.id);
        assert_eq!(parsed.name, "Test Corp");
        assert_eq!(parsed.quotas.max_agents, Some(10));
    }

    #[test]
    fn resource_quotas_default_is_unlimited() {
        let q = ResourceQuotas::default();
        assert!(q.max_agents.is_none());
        assert!(q.max_sessions_per_day.is_none());
        assert!(q.max_storage_mb.is_none());
        assert!(q.max_llm_tokens_per_day.is_none());
        assert!(q.max_llm_tokens_per_month.is_none());
    }

    #[test]
    fn get_nonexistent_tenant_returns_none() {
        let (store, key) = temp_store();
        let result = store.get_tenant(&TenantId::new(), &key).unwrap();
        assert!(result.is_none());
    }
}

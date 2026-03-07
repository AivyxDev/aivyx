//! Per-tenant directory layout.
//!
//! When multi-tenancy is enabled, each tenant gets an isolated directory tree
//! under `~/.aivyx/tenants/{tenant_id}/` with subdirectories for sessions,
//! memory, agents, audit, tasks, and the encrypted store.

use std::path::{Path, PathBuf};

use aivyx_core::{Result, TenantId};

/// Per-tenant directory structure.
pub struct TenantDirs {
    root: PathBuf,
}

impl TenantDirs {
    /// Create a `TenantDirs` for a specific tenant under the aivyx data root.
    pub fn new(aivyx_root: &Path, tenant_id: &TenantId) -> Self {
        Self {
            root: aivyx_root.join("tenants").join(tenant_id.to_string()),
        }
    }

    /// Create all required subdirectories.
    pub fn ensure_dirs(&self) -> Result<()> {
        let dirs = [
            self.sessions_dir(),
            self.memory_dir(),
            self.agents_dir(),
            self.tasks_dir(),
        ];
        for dir in &dirs {
            std::fs::create_dir_all(dir)?;
        }
        // Ensure parent of audit_path and store_path exist
        if let Some(parent) = self.audit_path().parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(())
    }

    /// Root directory for this tenant.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Sessions directory for this tenant.
    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    /// Memory directory for this tenant.
    pub fn memory_dir(&self) -> PathBuf {
        self.root.join("memory")
    }

    /// Agents directory for this tenant.
    pub fn agents_dir(&self) -> PathBuf {
        self.root.join("agents")
    }

    /// Audit log path for this tenant.
    pub fn audit_path(&self) -> PathBuf {
        self.root.join("audit.jsonl")
    }

    /// Tasks directory for this tenant.
    pub fn tasks_dir(&self) -> PathBuf {
        self.root.join("tasks")
    }

    /// Encrypted store path for this tenant.
    pub fn store_path(&self) -> PathBuf {
        self.root.join("store.db")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tenant_dirs_layout() {
        let tid = TenantId::new();
        let dirs = TenantDirs::new(Path::new("/data/.aivyx"), &tid);
        let expected_root = format!("/data/.aivyx/tenants/{}", tid);
        assert_eq!(dirs.root().to_string_lossy(), expected_root);
        assert!(dirs.sessions_dir().starts_with(&expected_root));
        assert!(dirs.memory_dir().starts_with(&expected_root));
        assert!(dirs.agents_dir().starts_with(&expected_root));
        assert!(dirs.audit_path().starts_with(&expected_root));
        assert!(dirs.tasks_dir().starts_with(&expected_root));
        assert!(dirs.store_path().starts_with(&expected_root));
    }

    #[test]
    fn ensure_dirs_creates_structure() {
        let tmp = tempfile::tempdir().unwrap();
        let tid = TenantId::new();
        let dirs = TenantDirs::new(tmp.path(), &tid);
        dirs.ensure_dirs().unwrap();

        assert!(dirs.sessions_dir().exists());
        assert!(dirs.memory_dir().exists());
        assert!(dirs.agents_dir().exists());
        assert!(dirs.tasks_dir().exists());
    }
}

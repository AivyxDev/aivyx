//! Authentication context for request handling.
//!
//! [`AuthContext`] carries the authenticated principal, optional tenant scope,
//! RBAC role, and cost allocation tags through the request lifecycle. It is
//! inserted by the auth middleware and extracted by route handlers via Axum's
//! `FromRequestParts` trait (implemented in `aivyx-server`).

use std::collections::HashMap;

use aivyx_core::{AivyxError, Principal, TenantId};
use aivyx_crypto::{MasterKey, derive_tenant_key};

use crate::rbac::AivyxRole;

/// Authentication and authorization context for the current request.
///
/// Produced by the auth middleware and consumed by route handlers.
/// In single-user mode, this is `AuthContext::single_user()` with `Admin` role
/// and no tenant scope.
#[derive(Debug, Clone)]
pub struct AuthContext {
    /// The authenticated identity.
    pub principal: Principal,
    /// Tenant scope — `None` in single-user mode.
    pub tenant: Option<TenantContext>,
    /// RBAC role determining what operations are permitted.
    pub role: AivyxRole,
    /// Cost allocation tags from the `X-Aivyx-Tags` header.
    pub tags: HashMap<String, String>,
}

/// Tenant-specific context for multi-tenant requests.
#[derive(Debug, Clone)]
pub struct TenantContext {
    /// The tenant this request belongs to.
    pub tenant_id: TenantId,
    /// Human-readable tenant name.
    pub tenant_name: String,
}

impl AuthContext {
    /// Create the default single-user `AuthContext` with `Admin` role.
    ///
    /// Used by the legacy bearer token auth middleware when multi-tenancy
    /// is not enabled. Gives full access with no tenant scope.
    pub fn single_user() -> Self {
        Self {
            principal: Principal::System,
            tenant: None,
            role: AivyxRole::Admin,
            tags: HashMap::new(),
        }
    }

    /// Create an `AuthContext` for a tenant user.
    pub fn tenant_user(
        tenant_id: TenantId,
        tenant_name: String,
        user_id: String,
        role: AivyxRole,
    ) -> Self {
        Self {
            principal: Principal::TenantUser { tenant_id, user_id },
            tenant: Some(TenantContext {
                tenant_id,
                tenant_name,
            }),
            role,
            tags: HashMap::new(),
        }
    }

    /// Check that the current role meets the minimum required level.
    ///
    /// Returns `Ok(())` if `self.role >= minimum`, otherwise returns an error
    /// suitable for mapping to HTTP 403 Forbidden.
    pub fn require_role(&self, minimum: AivyxRole) -> Result<(), AivyxError> {
        if self.role >= minimum {
            Ok(())
        } else {
            Err(AivyxError::CapabilityDenied(format!(
                "insufficient permissions: role '{}' required, have '{}'",
                minimum, self.role,
            )))
        }
    }

    /// Derive the effective master key for this request.
    ///
    /// In single-user mode (no tenant), returns a key derived from the global
    /// master key using the standard domain. In multi-tenant mode, derives a
    /// tenant-specific key via HKDF with the tenant ID as context.
    pub fn effective_master_key(&self, global_key: &MasterKey) -> MasterKey {
        match &self.tenant {
            Some(ctx) => derive_tenant_key(global_key, &ctx.tenant_id.to_string()),
            None => {
                // In single-user mode, just re-derive from the global key.
                // Callers typically use the global key directly, but this
                // provides a uniform interface.
                let bytes = global_key.expose_secret();
                let mut key_bytes = [0u8; 32];
                key_bytes.copy_from_slice(&bytes[..32]);
                MasterKey::from_bytes(key_bytes)
            }
        }
    }

    /// Get the tenant ID if in multi-tenant mode.
    pub fn tenant_id(&self) -> Option<&TenantId> {
        self.tenant.as_ref().map(|t| &t.tenant_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_user_has_admin_role() {
        let ctx = AuthContext::single_user();
        assert_eq!(ctx.role, AivyxRole::Admin);
        assert!(ctx.tenant.is_none());
    }

    #[test]
    fn require_role_succeeds_for_equal_or_higher() {
        let admin = AuthContext::single_user();
        assert!(admin.require_role(AivyxRole::Admin).is_ok());
        assert!(admin.require_role(AivyxRole::Operator).is_ok());
        assert!(admin.require_role(AivyxRole::Viewer).is_ok());
        assert!(admin.require_role(AivyxRole::Billing).is_ok());
    }

    #[test]
    fn require_role_fails_for_insufficient() {
        let viewer_ctx = AuthContext {
            principal: Principal::User("alice".into()),
            tenant: None,
            role: AivyxRole::Viewer,
            tags: HashMap::new(),
        };
        assert!(viewer_ctx.require_role(AivyxRole::Viewer).is_ok());
        assert!(viewer_ctx.require_role(AivyxRole::Operator).is_err());
        assert!(viewer_ctx.require_role(AivyxRole::Admin).is_err());
    }

    #[test]
    fn tenant_user_context() {
        let tid = TenantId::new();
        let ctx =
            AuthContext::tenant_user(tid, "Acme Corp".into(), "alice".into(), AivyxRole::Operator);
        assert_eq!(ctx.role, AivyxRole::Operator);
        assert!(ctx.tenant.is_some());
        assert_eq!(ctx.tenant.as_ref().unwrap().tenant_id, tid);
        assert_eq!(ctx.tenant_id(), Some(&tid));
    }

    #[test]
    fn effective_master_key_single_user() {
        let global = MasterKey::from_bytes([42u8; 32]);
        let ctx = AuthContext::single_user();
        let effective = ctx.effective_master_key(&global);
        // In single-user mode, the effective key is the same bytes as global
        assert_eq!(effective.expose_secret(), global.expose_secret());
    }

    #[test]
    fn effective_master_key_tenant() {
        let global = MasterKey::from_bytes([42u8; 32]);
        let tid = TenantId::new();
        let ctx = AuthContext::tenant_user(tid, "test".into(), "bob".into(), AivyxRole::Operator);
        let effective = ctx.effective_master_key(&global);
        // Tenant key must differ from global key
        assert_ne!(effective.expose_secret(), global.expose_secret());
        // Must be deterministic
        let effective2 = ctx.effective_master_key(&global);
        assert_eq!(effective.expose_secret(), effective2.expose_secret());
    }

    #[test]
    fn different_tenants_get_different_keys() {
        let global = MasterKey::from_bytes([42u8; 32]);
        let ctx_a = AuthContext::tenant_user(
            TenantId::new(),
            "Tenant A".into(),
            "alice".into(),
            AivyxRole::Operator,
        );
        let ctx_b = AuthContext::tenant_user(
            TenantId::new(),
            "Tenant B".into(),
            "bob".into(),
            AivyxRole::Operator,
        );
        let key_a = ctx_a.effective_master_key(&global);
        let key_b = ctx_b.effective_master_key(&global);
        assert_ne!(key_a.expose_secret(), key_b.expose_secret());
    }
}

//! Tenant management and authorization for the Aivyx platform.
//!
//! Provides [`AuthContext`] for request-scoped authentication/authorization,
//! [`AivyxRole`] for RBAC permission checks, [`TenantContext`] for
//! multi-tenant request scoping, [`TenantStore`] for persistent tenant
//! management, [`TenantDirs`] for per-tenant directory isolation, and
//! [`ApiKeyStore`] for tenant API key management.

pub mod api_key;
pub mod context;
pub mod dirs;
pub mod rbac;
pub mod store;

pub use api_key::{ApiKeyRecord, ApiKeyScope, ApiKeyStore};
pub use context::{AuthContext, TenantContext};
pub use dirs::TenantDirs;
pub use rbac::AivyxRole;
pub use store::{ResourceQuotas, TenantRecord, TenantStatus, TenantStore};

//! Role-based access control for the Aivyx platform.
//!
//! Defines [`AivyxRole`], an ordered enum representing four access levels.
//! Roles are ordered from least to most privileged: `Billing < Viewer < Operator < Admin`.
//! Permission checks use `PartialOrd` comparison against a minimum required role.

use serde::{Deserialize, Serialize};

/// Access role for an authenticated principal.
///
/// Roles are ordered by privilege level, enabling simple `>=` comparisons
/// for permission checks. The integer discriminants encode the ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum AivyxRole {
    /// View usage and billing data only.
    Billing = 0,
    /// Read-only access to all data (agents, sessions, memory, audit).
    Viewer = 1,
    /// Read + write access (create agents, run tasks, chat, manage memory).
    Operator = 2,
    /// Full access including tenant management, config, secrets, and plugins.
    Admin = 3,
}

impl AivyxRole {
    /// Whether this role has read access to system data.
    pub fn can_read(&self) -> bool {
        *self >= Self::Viewer
    }

    /// Whether this role can create/modify agents, tasks, and sessions.
    pub fn can_write(&self) -> bool {
        *self >= Self::Operator
    }

    /// Whether this role can perform administrative operations.
    pub fn can_admin(&self) -> bool {
        *self == Self::Admin
    }

    /// Whether this role can invoke LLM inference (chat, tasks, teams).
    pub fn can_run_llm(&self) -> bool {
        *self >= Self::Operator
    }

    /// Whether this role can view billing and usage metrics.
    /// All roles have this permission.
    pub fn can_view_billing(&self) -> bool {
        true
    }
}

impl std::fmt::Display for AivyxRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Billing => write!(f, "billing"),
            Self::Viewer => write!(f, "viewer"),
            Self::Operator => write!(f, "operator"),
            Self::Admin => write!(f, "admin"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_ordering() {
        assert!(AivyxRole::Billing < AivyxRole::Viewer);
        assert!(AivyxRole::Viewer < AivyxRole::Operator);
        assert!(AivyxRole::Operator < AivyxRole::Admin);
    }

    #[test]
    fn role_permissions() {
        // Billing: only billing
        assert!(!AivyxRole::Billing.can_read());
        assert!(!AivyxRole::Billing.can_write());
        assert!(!AivyxRole::Billing.can_admin());
        assert!(!AivyxRole::Billing.can_run_llm());
        assert!(AivyxRole::Billing.can_view_billing());

        // Viewer: read + billing
        assert!(AivyxRole::Viewer.can_read());
        assert!(!AivyxRole::Viewer.can_write());
        assert!(!AivyxRole::Viewer.can_admin());
        assert!(!AivyxRole::Viewer.can_run_llm());

        // Operator: read + write + llm + billing
        assert!(AivyxRole::Operator.can_read());
        assert!(AivyxRole::Operator.can_write());
        assert!(!AivyxRole::Operator.can_admin());
        assert!(AivyxRole::Operator.can_run_llm());

        // Admin: everything
        assert!(AivyxRole::Admin.can_read());
        assert!(AivyxRole::Admin.can_write());
        assert!(AivyxRole::Admin.can_admin());
        assert!(AivyxRole::Admin.can_run_llm());
    }

    #[test]
    fn role_serde_roundtrip() {
        for role in [
            AivyxRole::Billing,
            AivyxRole::Viewer,
            AivyxRole::Operator,
            AivyxRole::Admin,
        ] {
            let json = serde_json::to_string(&role).unwrap();
            let parsed: AivyxRole = serde_json::from_str(&json).unwrap();
            assert_eq!(role, parsed);
        }
    }

    #[test]
    fn role_display() {
        assert_eq!(AivyxRole::Billing.to_string(), "billing");
        assert_eq!(AivyxRole::Viewer.to_string(), "viewer");
        assert_eq!(AivyxRole::Operator.to_string(), "operator");
        assert_eq!(AivyxRole::Admin.to_string(), "admin");
    }
}

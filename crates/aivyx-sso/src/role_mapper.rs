//! Maps OIDC group claims to Aivyx roles.
//!
//! The [`RoleMapper`] evaluates a user's group memberships against a set of
//! configured [`GroupRoleMapping`] entries and returns the highest matching role.

use aivyx_tenant::AivyxRole;
use serde::{Deserialize, Serialize};

use crate::oidc::OidcClaims;

/// A mapping from an OIDC group name to an Aivyx role.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupRoleMapping {
    /// The OIDC group name to match.
    pub group: String,
    /// The Aivyx role to assign when the group matches.
    pub role: AivyxRole,
}

/// Maps OIDC group claims to the highest matching Aivyx role.
///
/// When multiple groups match, the highest-privilege role wins.
/// If no groups match, defaults to [`AivyxRole::Viewer`].
pub struct RoleMapper {
    mappings: Vec<GroupRoleMapping>,
}

impl RoleMapper {
    /// Create a new role mapper with the given group-to-role mappings.
    pub fn new(mappings: Vec<GroupRoleMapping>) -> Self {
        Self { mappings }
    }

    /// Map OIDC claims to the highest matching Aivyx role.
    ///
    /// Iterates through the user's group memberships and finds the highest
    /// role from the configured mappings. Returns [`AivyxRole::Viewer`] if
    /// no groups match.
    pub fn map_claims(&self, claims: &OidcClaims) -> AivyxRole {
        let mut best_role = None;

        for group in &claims.groups {
            for mapping in &self.mappings {
                if mapping.group == *group {
                    best_role = Some(match best_role {
                        Some(current) if mapping.role > current => mapping.role,
                        Some(current) => current,
                        None => mapping.role,
                    });
                }
            }
        }

        best_role.unwrap_or(AivyxRole::Viewer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_mappings() -> Vec<GroupRoleMapping> {
        vec![
            GroupRoleMapping {
                group: "viewers".into(),
                role: AivyxRole::Viewer,
            },
            GroupRoleMapping {
                group: "operators".into(),
                role: AivyxRole::Operator,
            },
            GroupRoleMapping {
                group: "admins".into(),
                role: AivyxRole::Admin,
            },
            GroupRoleMapping {
                group: "billing".into(),
                role: AivyxRole::Billing,
            },
        ]
    }

    #[test]
    fn single_group_maps_correctly() {
        let mapper = RoleMapper::new(test_mappings());

        let claims = OidcClaims {
            sub: "user-1".into(),
            email: None,
            groups: vec!["operators".into()],
            tenant_hint: None,
            exp: u64::MAX,
        };

        assert_eq!(mapper.map_claims(&claims), AivyxRole::Operator);
    }

    #[test]
    fn multiple_groups_highest_role_wins() {
        let mapper = RoleMapper::new(test_mappings());

        let claims = OidcClaims {
            sub: "user-2".into(),
            email: None,
            groups: vec!["viewers".into(), "admins".into(), "billing".into()],
            tenant_hint: None,
            exp: u64::MAX,
        };

        assert_eq!(mapper.map_claims(&claims), AivyxRole::Admin);
    }

    #[test]
    fn no_matching_groups_defaults_to_viewer() {
        let mapper = RoleMapper::new(test_mappings());

        let claims = OidcClaims {
            sub: "user-3".into(),
            email: None,
            groups: vec!["unknown-group".into()],
            tenant_hint: None,
            exp: u64::MAX,
        };

        assert_eq!(mapper.map_claims(&claims), AivyxRole::Viewer);
    }

    #[test]
    fn empty_groups_defaults_to_viewer() {
        let mapper = RoleMapper::new(test_mappings());

        let claims = OidcClaims {
            sub: "user-4".into(),
            email: None,
            groups: vec![],
            tenant_hint: None,
            exp: u64::MAX,
        };

        assert_eq!(mapper.map_claims(&claims), AivyxRole::Viewer);
    }

    #[test]
    fn empty_mappings_defaults_to_viewer() {
        let mapper = RoleMapper::new(vec![]);

        let claims = OidcClaims {
            sub: "user-5".into(),
            email: None,
            groups: vec!["admins".into()],
            tenant_hint: None,
            exp: u64::MAX,
        };

        assert_eq!(mapper.map_claims(&claims), AivyxRole::Viewer);
    }

    #[test]
    fn billing_group_maps_to_billing() {
        let mapper = RoleMapper::new(test_mappings());

        let claims = OidcClaims {
            sub: "user-6".into(),
            email: None,
            groups: vec!["billing".into()],
            tenant_hint: None,
            exp: u64::MAX,
        };

        assert_eq!(mapper.map_claims(&claims), AivyxRole::Billing);
    }
}

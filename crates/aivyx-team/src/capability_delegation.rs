use aivyx_capability::{ActionPattern, Capability, CapabilityScope, CapabilitySet};
use aivyx_core::{CapabilityId, Principal};
use chrono::Utc;

/// Delegate capabilities from a parent set to a team member.
///
/// For each parent capability, checks whether the requested scope is a valid
/// narrowing (via [`CapabilityScope::attenuate()`]) and the action pattern is
/// a subset. If both checks pass, creates a new capability granted to the
/// `member_principal` with a parent link back to the original.
///
/// Unlike [`Capability::attenuate()`], this function allows the member
/// principal to be **different** from the parent's `granted_to` — this is
/// the delegation act where the lead agent grants authority to a specialist.
/// The scope narrowing invariant is still enforced: specialists can never
/// exceed the lead's scope.
pub fn attenuate_for_member(
    parent_set: &CapabilitySet,
    member_principal: &Principal,
    allowed_scopes: &[CapabilityScope],
    action_pattern: &str,
) -> CapabilitySet {
    let mut result = CapabilitySet::new();

    let pattern = match ActionPattern::new(action_pattern) {
        Some(p) => p,
        None => return result,
    };

    for parent_cap in parent_set.iter() {
        // Skip revoked or expired capabilities.
        if !parent_cap.is_valid() {
            continue;
        }

        for scope in allowed_scopes {
            // Scope must be a valid narrowing of the parent's scope.
            if parent_cap.scope.attenuate(scope).is_none() {
                continue;
            }

            // Pattern must be a subset of the parent's pattern.
            if !pattern.is_subset_of(&parent_cap.pattern) {
                continue;
            }

            // Create a delegated capability for the specialist.
            let child = Capability {
                id: CapabilityId::new(),
                scope: scope.clone(),
                pattern: pattern.clone(),
                granted_to: vec![member_principal.clone()],
                granted_by: Principal::System,
                created_at: Utc::now(),
                expires_at: parent_cap.expires_at,
                revoked: false,
                parent_id: Some(parent_cap.id),
            };
            result.grant(child);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivyx_core::AgentId;
    use std::path::PathBuf;

    /// Create a lead agent's capability set with Filesystem { root: "/home" }.
    fn lead_caps() -> (CapabilitySet, Principal) {
        let lead_id = AgentId::new();
        let lead_principal = Principal::Agent(lead_id);
        let mut set = CapabilitySet::new();
        set.grant(Capability {
            id: CapabilityId::new(),
            scope: CapabilityScope::Filesystem {
                root: PathBuf::from("/home"),
            },
            pattern: ActionPattern::new("*").unwrap(),
            granted_to: vec![lead_principal.clone()],
            granted_by: Principal::System,
            created_at: Utc::now(),
            expires_at: None,
            revoked: false,
            parent_id: None,
        });
        (set, lead_principal)
    }

    #[test]
    fn delegate_narrows_scope() {
        let (parent_set, _lead) = lead_caps();
        let specialist = Principal::Agent(AgentId::new());

        // Specialist requests a narrower scope (subdirectory of /home)
        let narrowed = attenuate_for_member(
            &parent_set,
            &specialist,
            &[CapabilityScope::Filesystem {
                root: PathBuf::from("/home/docs"),
            }],
            "read:*",
        );

        assert_eq!(narrowed.len(), 1);

        // Specialist can use the delegated capability
        assert!(
            narrowed
                .check(
                    &specialist,
                    &CapabilityScope::Filesystem {
                        root: PathBuf::from("/home/docs"),
                    },
                    "read:file",
                )
                .is_ok()
        );
    }

    #[test]
    fn delegate_rejects_broader_scope() {
        let (parent_set, _lead) = lead_caps();
        let specialist = Principal::Agent(AgentId::new());

        // Specialist requests broader scope than lead has (/ vs /home)
        let narrowed = attenuate_for_member(
            &parent_set,
            &specialist,
            &[CapabilityScope::Filesystem {
                root: PathBuf::from("/"),
            }],
            "*",
        );

        assert!(narrowed.is_empty());
    }

    #[test]
    fn delegate_grants_to_different_principal() {
        let (parent_set, lead) = lead_caps();
        let specialist = Principal::Agent(AgentId::new());
        assert_ne!(
            lead, specialist,
            "lead and specialist must be different agents"
        );

        // Delegation allows granting to a different principal (the specialist)
        let narrowed = attenuate_for_member(
            &parent_set,
            &specialist,
            &[CapabilityScope::Filesystem {
                root: PathBuf::from("/home"),
            }],
            "*",
        );

        assert_eq!(narrowed.len(), 1);

        // The capability is granted to the specialist, not the lead
        assert!(
            narrowed
                .check(
                    &specialist,
                    &CapabilityScope::Filesystem {
                        root: PathBuf::from("/home"),
                    },
                    "execute:file_read",
                )
                .is_ok()
        );

        // The lead's own principal doesn't match the delegated capability
        assert!(
            narrowed
                .check(
                    &lead,
                    &CapabilityScope::Filesystem {
                        root: PathBuf::from("/home"),
                    },
                    "execute:file_read",
                )
                .is_err()
        );
    }

    #[test]
    fn delegate_cross_scope_rejected() {
        let (parent_set, _lead) = lead_caps();
        let specialist = Principal::Agent(AgentId::new());

        // Parent has Filesystem but specialist requests Shell — should be rejected
        let narrowed = attenuate_for_member(
            &parent_set,
            &specialist,
            &[CapabilityScope::Shell {
                allowed_commands: vec![],
            }],
            "*",
        );

        assert!(narrowed.is_empty());
    }
}

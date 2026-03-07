//! Capability audit reports for agent profiles.
//!
//! Scans all agent TOML profiles and produces a structured report that
//! inventories each agent's capabilities and flags overly permissive grants.

use std::path::Path;

use aivyx_agent::AgentProfile;
use aivyx_core::{AivyxError, AutonomyTier, CapabilityScope};
use chrono::{DateTime, Utc};
use serde::Serialize;

/// Full capability audit report across all agents.
#[derive(Debug, Serialize)]
pub struct CapabilityAuditReport {
    /// When the report was generated.
    pub generated_at: DateTime<Utc>,
    /// Number of agent profiles scanned.
    pub agents_scanned: usize,
    /// Per-agent capability summaries.
    pub agents: Vec<AgentCapabilityEntry>,
    /// Security warnings found during the scan.
    pub warnings: Vec<CapabilityWarning>,
}

/// Capability summary for a single agent.
#[derive(Debug, Serialize)]
pub struct AgentCapabilityEntry {
    /// Agent profile name.
    pub agent_name: String,
    /// Agent role description.
    pub role: String,
    /// Display string for the agent's autonomy tier.
    pub autonomy_tier: String,
    /// Capabilities granted to this agent.
    pub capabilities: Vec<CapabilitySummary>,
    /// Number of tools available to this agent.
    pub tool_count: usize,
}

/// A single capability expressed as display strings.
#[derive(Debug, Serialize)]
pub struct CapabilitySummary {
    /// Human-readable scope description.
    pub scope: String,
    /// Action glob pattern.
    pub pattern: String,
}

/// A security warning found during capability audit.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum CapabilityWarning {
    /// Agent has shell access with no command restrictions.
    WildcardShell { agent: String },
    /// Agent has filesystem access rooted at `/`.
    WildcardFilesystem { agent: String, root: String },
    /// Agent has network access with no host restrictions.
    WildcardNetwork { agent: String },
    /// Agent has a custom scope that is not namespaced (no `:` separator).
    UnrestrictedCustom { agent: String, scope: String },
    /// Agent with Trust or Free autonomy has 3 or more capabilities.
    HighAutonomyWithBroadScope {
        agent: String,
        tier: String,
        scope_count: usize,
    },
}

/// Format a `CapabilityScope` as a human-readable display string.
fn format_scope(scope: &CapabilityScope) -> String {
    match scope {
        CapabilityScope::Filesystem { root } => {
            format!("Filesystem({})", root.display())
        }
        CapabilityScope::Network { hosts, ports } => {
            if hosts.is_empty() && ports.is_empty() {
                "Network(*)".to_string()
            } else if ports.is_empty() {
                format!("Network(hosts=[{}])", hosts.join(", "))
            } else if hosts.is_empty() {
                let ports_str: Vec<String> = ports.iter().map(|p| p.to_string()).collect();
                format!("Network(ports=[{}])", ports_str.join(", "))
            } else {
                let ports_str: Vec<String> = ports.iter().map(|p| p.to_string()).collect();
                format!(
                    "Network(hosts=[{}], ports=[{}])",
                    hosts.join(", "),
                    ports_str.join(", ")
                )
            }
        }
        CapabilityScope::Shell { allowed_commands } => {
            if allowed_commands.is_empty() {
                "Shell(*)".to_string()
            } else {
                format!("Shell([{}])", allowed_commands.join(", "))
            }
        }
        CapabilityScope::Email { allowed_recipients } => {
            if allowed_recipients.is_empty() {
                "Email(*)".to_string()
            } else {
                format!("Email([{}])", allowed_recipients.join(", "))
            }
        }
        CapabilityScope::Calendar => "Calendar".to_string(),
        CapabilityScope::Custom(s) => format!("Custom({s})"),
    }
}

/// Format an optional `AutonomyTier` as a display string.
fn format_tier(tier: Option<AutonomyTier>) -> String {
    match tier {
        Some(t) => t.to_string(),
        None => "Default".to_string(),
    }
}

/// Scan all agent profiles in `agents_dir` and produce a capability audit report.
///
/// If the directory does not exist or contains no `.toml` files, returns an
/// empty report with no warnings.
pub fn audit_agent_capabilities(agents_dir: &Path) -> Result<CapabilityAuditReport, AivyxError> {
    let mut agents = Vec::new();
    let mut warnings = Vec::new();

    if agents_dir.is_dir() {
        let entries = std::fs::read_dir(agents_dir)?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }

            let profile = match AgentProfile::load(&path) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "skipping unparseable agent profile");
                    continue;
                }
            };

            // Build capability summaries and check for warnings.
            let mut cap_summaries = Vec::new();
            for cap in &profile.capabilities {
                cap_summaries.push(CapabilitySummary {
                    scope: format_scope(&cap.scope),
                    pattern: cap.pattern.clone(),
                });

                // Check for wildcard shell.
                if let CapabilityScope::Shell { allowed_commands } = &cap.scope
                    && allowed_commands.is_empty()
                {
                    warnings.push(CapabilityWarning::WildcardShell {
                        agent: profile.name.clone(),
                    });
                }

                // Check for wildcard filesystem (root = /).
                if let CapabilityScope::Filesystem { root } = &cap.scope
                    && root.as_os_str() == "/"
                {
                    warnings.push(CapabilityWarning::WildcardFilesystem {
                        agent: profile.name.clone(),
                        root: root.display().to_string(),
                    });
                }

                // Check for wildcard network.
                if let CapabilityScope::Network { hosts, .. } = &cap.scope
                    && hosts.is_empty()
                {
                    warnings.push(CapabilityWarning::WildcardNetwork {
                        agent: profile.name.clone(),
                    });
                }

                // Check for unrestricted custom scope (not namespaced).
                if let CapabilityScope::Custom(s) = &cap.scope
                    && !s.contains(':')
                {
                    warnings.push(CapabilityWarning::UnrestrictedCustom {
                        agent: profile.name.clone(),
                        scope: s.clone(),
                    });
                }
            }

            // Check for high autonomy with broad scope.
            let tier = profile.autonomy_tier;
            if matches!(tier, Some(AutonomyTier::Trust) | Some(AutonomyTier::Free))
                && profile.capabilities.len() >= 3
            {
                warnings.push(CapabilityWarning::HighAutonomyWithBroadScope {
                    agent: profile.name.clone(),
                    tier: format_tier(tier),
                    scope_count: profile.capabilities.len(),
                });
            }

            agents.push(AgentCapabilityEntry {
                agent_name: profile.name,
                role: profile.role,
                autonomy_tier: format_tier(tier),
                capabilities: cap_summaries,
                tool_count: profile.tool_ids.len(),
            });
        }
    }

    Ok(CapabilityAuditReport {
        generated_at: Utc::now(),
        agents_scanned: agents.len(),
        agents,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_agents_dir() {
        let dir = tempfile::tempdir().unwrap();
        let report = audit_agent_capabilities(dir.path()).unwrap();
        assert_eq!(report.agents_scanned, 0);
        assert!(report.agents.is_empty());
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn nonexistent_dir_returns_empty_report() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("no-such-dir");
        let report = audit_agent_capabilities(&missing).unwrap();
        assert_eq!(report.agents_scanned, 0);
    }

    #[test]
    fn wildcard_shell_warning() {
        let dir = tempfile::tempdir().unwrap();
        let toml_content = r#"
name = "shell-agent"
role = "ops"
soul = "You run commands."
tool_ids = ["shell"]

[[capabilities]]
pattern = "*"

[capabilities.scope]
Shell = { allowed_commands = [] }
"#;
        std::fs::write(dir.path().join("shell-agent.toml"), toml_content).unwrap();

        let report = audit_agent_capabilities(dir.path()).unwrap();
        assert_eq!(report.agents_scanned, 1);
        assert_eq!(report.agents[0].agent_name, "shell-agent");
        assert!(report.warnings.iter().any(|w| matches!(
            w,
            CapabilityWarning::WildcardShell { agent } if agent == "shell-agent"
        )));
    }

    #[test]
    fn wildcard_filesystem_warning() {
        let dir = tempfile::tempdir().unwrap();
        let toml_content = r#"
name = "fs-agent"
role = "writer"
soul = "You write files."
tool_ids = ["file_write"]

[[capabilities]]
pattern = "*"

[capabilities.scope]
Filesystem = { root = "/" }
"#;
        std::fs::write(dir.path().join("fs-agent.toml"), toml_content).unwrap();

        let report = audit_agent_capabilities(dir.path()).unwrap();
        assert_eq!(report.agents_scanned, 1);
        assert!(report.warnings.iter().any(|w| matches!(
            w,
            CapabilityWarning::WildcardFilesystem { agent, root }
                if agent == "fs-agent" && root == "/"
        )));
    }

    #[test]
    fn wildcard_network_warning() {
        let dir = tempfile::tempdir().unwrap();
        let toml_content = r#"
name = "net-agent"
role = "fetcher"
soul = "You fetch things."
tool_ids = ["http_fetch"]

[[capabilities]]
pattern = "*"

[capabilities.scope]
Network = { hosts = [], ports = [] }
"#;
        std::fs::write(dir.path().join("net-agent.toml"), toml_content).unwrap();

        let report = audit_agent_capabilities(dir.path()).unwrap();
        assert!(report.warnings.iter().any(|w| matches!(
            w,
            CapabilityWarning::WildcardNetwork { agent } if agent == "net-agent"
        )));
    }

    #[test]
    fn unrestricted_custom_warning() {
        let dir = tempfile::tempdir().unwrap();
        let toml_content = r#"
name = "custom-agent"
role = "helper"
soul = "You help."
tool_ids = []

[[capabilities]]
pattern = "*"

[capabilities.scope]
Custom = "memory"
"#;
        std::fs::write(dir.path().join("custom-agent.toml"), toml_content).unwrap();

        let report = audit_agent_capabilities(dir.path()).unwrap();
        assert!(report.warnings.iter().any(|w| matches!(
            w,
            CapabilityWarning::UnrestrictedCustom { agent, scope }
                if agent == "custom-agent" && scope == "memory"
        )));
    }

    #[test]
    fn namespaced_custom_no_warning() {
        let dir = tempfile::tempdir().unwrap();
        let toml_content = r#"
name = "ns-agent"
role = "helper"
soul = "You help."
tool_ids = []

[[capabilities]]
pattern = "*"

[capabilities.scope]
Custom = "mcp:github"
"#;
        std::fs::write(dir.path().join("ns-agent.toml"), toml_content).unwrap();

        let report = audit_agent_capabilities(dir.path()).unwrap();
        assert!(
            !report
                .warnings
                .iter()
                .any(|w| matches!(w, CapabilityWarning::UnrestrictedCustom { .. }))
        );
    }

    #[test]
    fn high_autonomy_broad_scope_warning() {
        let dir = tempfile::tempdir().unwrap();
        let toml_content = r#"
name = "free-agent"
role = "ops"
soul = "You do everything."
tool_ids = ["shell", "file_read", "http_fetch"]
autonomy_tier = "Free"

[[capabilities]]
pattern = "*"

[capabilities.scope]
Shell = { allowed_commands = [] }

[[capabilities]]
pattern = "*"

[capabilities.scope]
Filesystem = { root = "/" }

[[capabilities]]
pattern = "*"

[capabilities.scope]
Network = { hosts = [], ports = [] }
"#;
        std::fs::write(dir.path().join("free-agent.toml"), toml_content).unwrap();

        let report = audit_agent_capabilities(dir.path()).unwrap();
        assert!(report.warnings.iter().any(|w| matches!(
            w,
            CapabilityWarning::HighAutonomyWithBroadScope { agent, tier, scope_count }
                if agent == "free-agent" && tier == "Free" && *scope_count == 3
        )));
    }

    #[test]
    fn leash_tier_no_high_autonomy_warning() {
        let dir = tempfile::tempdir().unwrap();
        let toml_content = r#"
name = "leash-agent"
role = "ops"
soul = "You are cautious."
tool_ids = ["shell", "file_read", "http_fetch"]
autonomy_tier = "Leash"

[[capabilities]]
pattern = "*"

[capabilities.scope]
Shell = { allowed_commands = [] }

[[capabilities]]
pattern = "*"

[capabilities.scope]
Filesystem = { root = "/" }

[[capabilities]]
pattern = "*"

[capabilities.scope]
Network = { hosts = [], ports = [] }
"#;
        std::fs::write(dir.path().join("leash-agent.toml"), toml_content).unwrap();

        let report = audit_agent_capabilities(dir.path()).unwrap();
        assert!(
            !report
                .warnings
                .iter()
                .any(|w| matches!(w, CapabilityWarning::HighAutonomyWithBroadScope { .. }))
        );
    }

    #[test]
    fn multiple_agents_scanned() {
        let dir = tempfile::tempdir().unwrap();

        let agent1 = r#"
name = "agent1"
role = "coder"
soul = "Code things."
tool_ids = ["file_read"]
"#;
        let agent2 = r#"
name = "agent2"
role = "writer"
soul = "Write things."
tool_ids = ["file_write", "file_read"]
"#;
        std::fs::write(dir.path().join("agent1.toml"), agent1).unwrap();
        std::fs::write(dir.path().join("agent2.toml"), agent2).unwrap();

        let report = audit_agent_capabilities(dir.path()).unwrap();
        assert_eq!(report.agents_scanned, 2);
    }

    #[test]
    fn non_toml_files_ignored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("readme.md"), "# Agents").unwrap();
        std::fs::write(dir.path().join("config.json"), "{}").unwrap();

        let report = audit_agent_capabilities(dir.path()).unwrap();
        assert_eq!(report.agents_scanned, 0);
    }

    #[test]
    fn format_scope_display() {
        assert_eq!(
            format_scope(&CapabilityScope::Filesystem {
                root: std::path::PathBuf::from("/home")
            }),
            "Filesystem(/home)"
        );
        assert_eq!(
            format_scope(&CapabilityScope::Shell {
                allowed_commands: vec![]
            }),
            "Shell(*)"
        );
        assert_eq!(
            format_scope(&CapabilityScope::Shell {
                allowed_commands: vec!["ls".into(), "cat".into()]
            }),
            "Shell([ls, cat])"
        );
        assert_eq!(
            format_scope(&CapabilityScope::Network {
                hosts: vec![],
                ports: vec![]
            }),
            "Network(*)"
        );
        assert_eq!(format_scope(&CapabilityScope::Calendar), "Calendar");
        assert_eq!(
            format_scope(&CapabilityScope::Custom("memory".into())),
            "Custom(memory)"
        );
    }
}

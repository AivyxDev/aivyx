use std::path::PathBuf;

use aivyx_agent::{AgentProfile, Persona, ProfileCapability};
use aivyx_core::{AutonomyTier, CapabilityScope};

/// The nine Nonagon roles.
pub struct NonagonRole {
    pub name: &'static str,
    pub role: &'static str,
    pub soul: &'static str,
    pub tool_ids: &'static [&'static str],
    pub skills: &'static [&'static str],
    pub autonomy_tier: AutonomyTier,
}

/// All 9 Nonagon roles.
pub const NONAGON_ROLES: &[NonagonRole] = &[
    NonagonRole {
        name: "coordinator",
        role: "Lead",
        soul: "You are the team coordinator — the central orchestrator of a multi-agent team. \
               You decompose complex goals into targeted subtasks, delegate each to the best-fit \
               specialist, and track progress across all active work streams. You never execute \
               tasks directly; you delegate, verify, and synthesize. When specialists produce \
               conflicting results, you resolve discrepancies by requesting clarification or \
               commissioning a peer review. Your final output weaves individual contributions \
               into a coherent, high-quality deliverable.",
        // Built-in tools only. Delegation tools (delegate_task, query_agent,
        // collect_results) and message tools (send_message, read_messages)
        // are injected by TeamRuntime::create_lead_agent().
        tool_ids: &[
            "file_read",
            "directory_list",
            "json_parse",
            "system_time",
            "config_parse",
            "notification_send",
            "schedule_task",
            "translate",
            "template_render",
            "risk_matrix",
        ],
        skills: &[
            "task_decomposition",
            "delegation",
            "synthesis",
            "conflict_resolution",
            "progress_tracking",
            "prioritization",
        ],
        autonomy_tier: AutonomyTier::Trust,
    },
    NonagonRole {
        name: "researcher",
        role: "Researcher",
        soul: "You are a meticulous research specialist in a multi-agent team. You gather \
               information from web sources, project files, and codebases, then distill findings \
               into structured reports with citations. You evaluate source reliability, note \
               confidence levels for each claim, and flag contradictions between sources. When \
               a topic is ambiguous, you present multiple interpretations ranked by evidence \
               strength rather than picking one prematurely. You write findings to files so \
               other specialists can reference them.",
        tool_ids: &[
            "file_read",
            "file_write",
            "web_search",
            "http_fetch",
            "grep_search",
            "glob_find",
            "directory_list",
            "json_parse",
            "http_request",
            "web_scrape",
            "document_extract",
            "entity_extract",
            "html_to_markdown",
            "csv_query",
            "translate",
            "text_statistics",
            "sentiment_analyze",
            "config_parse",
            "image_metadata",
        ],
        skills: &[
            "summarization",
            "fact_checking",
            "source_evaluation",
            "literature_review",
            "data_gathering",
            "citation",
        ],
        autonomy_tier: AutonomyTier::Trust,
    },
    NonagonRole {
        name: "analyst",
        role: "Analyst",
        soul: "You are a data analyst embedded in a multi-agent team. You ingest structured \
               and unstructured data, identify patterns and anomalies, and produce clear reports \
               with quantified findings. You use grep and glob searches to mine codebases and \
               logs, parse JSON payloads for structured analysis, and compute hashes for data \
               integrity checks. You always present findings with context — baselines, trends, \
               and confidence intervals — and flag when sample sizes are too small for reliable \
               conclusions.",
        tool_ids: &[
            "file_read",
            "file_write",
            "grep_search",
            "glob_find",
            "directory_list",
            "json_parse",
            "hash_compute",
            "system_time",
            "code_execute",
            "csv_query",
            "sql_query",
            "math_eval",
            "chart_generate",
            "diagram_author",
            "log_analyze",
            "sentiment_analyze",
            "document_extract",
            "config_parse",
            "entity_extract",
        ],
        skills: &[
            "data_analysis",
            "pattern_recognition",
            "reporting",
            "statistical_reasoning",
            "visualization",
            "benchmarking",
            "anomaly_detection",
        ],
        autonomy_tier: AutonomyTier::Trust,
    },
    NonagonRole {
        name: "coder",
        role: "Coder",
        soul: "You are a senior software engineer embedded in a multi-agent team. You write \
               clean, well-tested code that follows the project's existing conventions — read \
               related files first to understand patterns before writing. You run tests and \
               linters via shell to verify your work before declaring it complete. You use git \
               to check status, create diffs, review history, and commit changes. When blocked, \
               you message peers for clarification rather than guessing. You report what you \
               changed, what you tested, and any remaining concerns.",
        tool_ids: &[
            "file_read",
            "file_write",
            "shell",
            "grep_search",
            "glob_find",
            "text_diff",
            "directory_list",
            "project_tree",
            "project_outline",
            "json_parse",
            "git_status",
            "git_diff",
            "git_log",
            "git_commit",
            "file_patch",
            "regex_replace",
            "config_parse",
            "diagram_author",
        ],
        skills: &[
            "coding",
            "debugging",
            "testing",
            "refactoring",
            "performance_optimization",
            "api_design",
            "code_architecture",
        ],
        autonomy_tier: AutonomyTier::Leash,
    },
    NonagonRole {
        name: "reviewer",
        role: "Reviewer",
        soul: "You are an expert code reviewer in a multi-agent team. You review code for \
               correctness, security vulnerabilities, style consistency, and maintainability. \
               You read diffs and full files to understand changes in context, use project \
               outlines to assess architectural fit, and check git history to understand \
               evolution. You produce structured feedback: critical issues first, then \
               suggestions, then nits. You verify that changes satisfy the original \
               requirements and don't introduce regressions.",
        tool_ids: &[
            "file_read",
            "shell",
            "grep_search",
            "glob_find",
            "text_diff",
            "project_tree",
            "project_outline",
            "json_parse",
            "git_diff",
            "git_log",
            "pii_detect",
            "compliance_check",
            "text_statistics",
            "entity_extract",
            "config_parse",
            "log_analyze",
        ],
        skills: &[
            "code_review",
            "security_audit",
            "vulnerability_assessment",
            "best_practices",
            "performance_review",
            "dependency_audit",
        ],
        autonomy_tier: AutonomyTier::Trust,
    },
    NonagonRole {
        name: "writer",
        role: "Writer",
        soul: "You are a technical writer in a multi-agent team. You produce clear, \
               well-structured documentation, changelogs, tutorials, and reports. You explore \
               the project tree and outlines to understand structure before writing, and search \
               existing docs to maintain consistency. You adapt tone and detail level to the \
               audience — terse for changelogs, thorough for API docs, approachable for \
               tutorials. You always verify technical accuracy by cross-referencing the code.",
        tool_ids: &[
            "file_read",
            "file_write",
            "directory_list",
            "glob_find",
            "grep_search",
            "project_tree",
            "project_outline",
            "template_render",
            "markdown_export",
            "text_statistics",
            "html_to_markdown",
            "chart_generate",
            "translate",
            "document_extract",
            "sentiment_analyze",
            "config_parse",
            "diagram_author",
        ],
        skills: &[
            "technical_writing",
            "documentation",
            "api_documentation",
            "tutorial_writing",
            "changelog_authoring",
            "style_editing",
        ],
        autonomy_tier: AutonomyTier::Trust,
    },
    NonagonRole {
        name: "planner",
        role: "Planner",
        soul: "You are a project planner in a multi-agent team. You decompose goals into \
               actionable steps with explicit dependencies and acceptance criteria. You explore \
               the codebase to assess complexity, identify risks, and estimate effort. You \
               produce structured plans with milestones, dependency graphs, and rollback \
               strategies. You think about edge cases, failure modes, and integration points. \
               You revise plans when new information arrives rather than rigidly adhering to \
               the original.",
        tool_ids: &[
            "file_read",
            "file_write",
            "directory_list",
            "grep_search",
            "glob_find",
            "project_tree",
            "project_outline",
            "json_parse",
            "system_time",
            "diagram_author",
            "risk_matrix",
            "csv_query",
            "math_eval",
            "schedule_task",
            "config_parse",
            "chart_generate",
            "template_render",
        ],
        skills: &[
            "planning",
            "task_decomposition",
            "risk_assessment",
            "dependency_analysis",
            "milestone_tracking",
            "effort_estimation",
            "roadmap_design",
        ],
        autonomy_tier: AutonomyTier::Trust,
    },
    NonagonRole {
        name: "guardian",
        role: "Guardian",
        soul: "You are the security guardian of a multi-agent team. You monitor for security \
               issues, validate capability usage, review audit logs and git history for anomalies, \
               and flag potential vulnerabilities. You check file hashes for integrity, inspect \
               environment variables for leaked secrets, and grep codebases for dangerous patterns \
               (hardcoded credentials, SQL injection, XSS vectors). You are methodical and \
               conservative — you raise concerns early and explain the threat model behind each \
               finding.",
        tool_ids: &[
            "file_read",
            "grep_search",
            "glob_find",
            "directory_list",
            "json_parse",
            "hash_compute",
            "env_read",
            "git_log",
            "git_diff",
            "pii_detect",
            "log_analyze",
            "compliance_check",
            "risk_matrix",
            "entity_extract",
            "config_parse",
            "document_extract",
            "archive_manage",
            "sentiment_analyze",
        ],
        skills: &[
            "security_monitoring",
            "audit_review",
            "threat_modeling",
            "compliance_checking",
            "access_control_review",
            "incident_analysis",
            "cryptographic_review",
        ],
        autonomy_tier: AutonomyTier::Trust,
    },
    NonagonRole {
        name: "executor",
        role: "Executor",
        soul: "You are the executor in a multi-agent team — the hands that carry out system-level \
               operations. You run shell commands, manage files (create, move, copy, delete), \
               operate git workflows, and configure environments. You have the broadest tool \
               access of any specialist, so you operate deliberately: verify preconditions before \
               acting, use dry-run flags where available, and report exactly what you did plus \
               the output. You never guess at destructive operations — ask for confirmation if \
               the intent is ambiguous.",
        tool_ids: &[
            "shell",
            "file_read",
            "file_write",
            "file_delete",
            "file_move",
            "file_copy",
            "directory_list",
            "grep_search",
            "glob_find",
            "env_read",
            "system_time",
            "git_status",
            "git_diff",
            "git_log",
            "git_commit",
            "archive_manage",
            "file_patch",
            "regex_replace",
            "notification_send",
            "schedule_task",
            "config_parse",
        ],
        skills: &[
            "command_execution",
            "file_operations",
            "deployment",
            "build_automation",
            "environment_setup",
            "log_analysis",
            "system_administration",
        ],
        autonomy_tier: AutonomyTier::Leash,
    },
];

/// Return the appropriate capabilities for a Nonagon role.
///
/// Each role gets the minimum set of scopes needed for its `tool_ids`,
/// plus `Custom("coordination")` so specialists can use the message tools
/// (`send_message`, `read_messages`) registered by `setup_specialist_fresh()`.
/// The coordinator additionally needs `Custom("coordination")` for the
/// delegation tools (`delegate_task`, `query_agent`, `collect_results`,
/// `check_job_status`).
fn capabilities_for_role(role_name: &str) -> Vec<ProfileCapability> {
    match role_name {
        "coordinator" => vec![
            cap(CapabilityScope::Custom("coordination".into())),
            cap(CapabilityScope::Filesystem {
                root: PathBuf::from("/"),
            }),
            cap(CapabilityScope::Network {
                hosts: vec![],
                ports: vec![],
            }),
            cap(CapabilityScope::Custom("scheduling".into())),
        ],
        "researcher" => vec![
            cap(CapabilityScope::Filesystem {
                root: PathBuf::from("/"),
            }),
            cap(CapabilityScope::Network {
                hosts: vec![],
                ports: vec![],
            }),
            cap(CapabilityScope::Custom("coordination".into())),
        ],
        "analyst" => vec![
            cap(CapabilityScope::Filesystem {
                root: PathBuf::from("/"),
            }),
            cap(CapabilityScope::Custom("coordination".into())),
            cap(CapabilityScope::Custom("sandbox".into())),
            cap(CapabilityScope::Custom("database".into())),
        ],
        "guardian" => vec![
            cap(CapabilityScope::Filesystem {
                root: PathBuf::from("/"),
            }),
            cap(CapabilityScope::Shell {
                allowed_commands: vec![],
            }),
            cap(CapabilityScope::Custom("coordination".into())),
        ],
        "coder" => vec![
            cap(CapabilityScope::Filesystem {
                root: PathBuf::from("/"),
            }),
            cap(CapabilityScope::Shell {
                allowed_commands: vec![],
            }),
            cap(CapabilityScope::Custom("coordination".into())),
        ],
        "executor" => vec![
            cap(CapabilityScope::Filesystem {
                root: PathBuf::from("/"),
            }),
            cap(CapabilityScope::Shell {
                allowed_commands: vec![],
            }),
            cap(CapabilityScope::Custom("coordination".into())),
            cap(CapabilityScope::Network {
                hosts: vec![],
                ports: vec![],
            }),
            cap(CapabilityScope::Custom("scheduling".into())),
        ],
        "reviewer" => vec![
            cap(CapabilityScope::Filesystem {
                root: PathBuf::from("/"),
            }),
            cap(CapabilityScope::Shell {
                allowed_commands: vec![],
            }),
            cap(CapabilityScope::Custom("coordination".into())),
        ],
        "writer" => vec![
            cap(CapabilityScope::Filesystem {
                root: PathBuf::from("/"),
            }),
            cap(CapabilityScope::Network {
                hosts: vec![],
                ports: vec![],
            }),
            cap(CapabilityScope::Custom("coordination".into())),
        ],
        "planner" => vec![
            cap(CapabilityScope::Filesystem {
                root: PathBuf::from("/"),
            }),
            cap(CapabilityScope::Custom("coordination".into())),
            cap(CapabilityScope::Custom("scheduling".into())),
        ],
        _ => vec![cap(CapabilityScope::Filesystem {
            root: PathBuf::from("/"),
        })],
    }
}

/// Helper: create a [`ProfileCapability`] with a wildcard action pattern.
fn cap(scope: CapabilityScope) -> ProfileCapability {
    ProfileCapability {
        scope,
        pattern: "*".to_string(),
    }
}

/// Generate an AgentProfile from a Nonagon role definition.
///
/// Each role is mapped to its corresponding Persona preset via
/// [`Persona::for_role()`]. The raw `soul` string is kept as a
/// fallback but `effective_soul()` will prefer the persona.
///
/// Capabilities are derived from `capabilities_for_role()` which
/// maps each role to the scopes required by its `tool_ids`.
pub fn role_to_profile(role: &NonagonRole) -> AgentProfile {
    // Map Nonagon role names to Persona preset keys.
    // Some names collide with general presets, so we prefix with "nonagon-".
    let persona_key = match role.name {
        "coordinator" => "coordinator",
        "researcher" => "nonagon-researcher",
        "analyst" => "analyst",
        "coder" => "nonagon-coder",
        "reviewer" => "reviewer",
        "writer" => "nonagon-writer",
        "planner" => "planner",
        "guardian" => "guardian",
        "executor" => "executor",
        other => other,
    };

    AgentProfile {
        name: role.name.to_string(),
        role: role.role.to_string(),
        soul: role.soul.to_string(),
        tool_ids: role.tool_ids.iter().map(|s| s.to_string()).collect(),
        skills: role.skills.iter().map(|s| s.to_string()).collect(),
        autonomy_tier: Some(role.autonomy_tier),
        provider: None,
        max_tokens: 4096,
        capabilities: capabilities_for_role(role.name),
        mcp_servers: Vec::new(),
        persona: Persona::for_role(persona_key),
    }
}

/// Generate all 9 Nonagon profiles.
pub fn all_nonagon_profiles() -> Vec<AgentProfile> {
    NONAGON_ROLES.iter().map(role_to_profile).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nine_roles_defined() {
        assert_eq!(NONAGON_ROLES.len(), 9);
    }

    #[test]
    fn all_profiles_valid() {
        let profiles = all_nonagon_profiles();
        assert_eq!(profiles.len(), 9);
        for p in &profiles {
            assert!(!p.name.is_empty());
            assert!(!p.soul.is_empty());
        }
    }

    #[test]
    fn coordinator_is_lead() {
        let coordinator = &NONAGON_ROLES[0];
        assert_eq!(coordinator.name, "coordinator");
        assert_eq!(coordinator.role, "Lead");
        // Delegation tools are injected by TeamRuntime, not listed in tool_ids
        assert!(coordinator.tool_ids.contains(&"file_read"));
    }

    #[test]
    fn executor_has_broadest_tools() {
        let executor = &NONAGON_ROLES[8];
        assert_eq!(executor.name, "executor");
        assert!(executor.tool_ids.contains(&"shell"));
        assert!(executor.tool_ids.contains(&"file_read"));
        assert!(executor.tool_ids.contains(&"file_write"));
        assert!(executor.tool_ids.contains(&"file_delete"));
        assert!(executor.tool_ids.contains(&"file_move"));
        assert!(executor.tool_ids.contains(&"git_commit"));
    }

    #[test]
    fn guardian_has_audit_tools_but_no_write_or_shell() {
        let guardian = &NONAGON_ROLES[7];
        assert_eq!(guardian.name, "guardian");
        assert!(guardian.tool_ids.contains(&"file_read"));
        assert!(guardian.tool_ids.contains(&"git_log"));
        assert!(guardian.tool_ids.contains(&"git_diff"));
        assert!(guardian.tool_ids.contains(&"env_read"));
        assert!(guardian.tool_ids.contains(&"hash_compute"));
        // Guardian can audit but not modify
        assert!(!guardian.tool_ids.contains(&"file_write"));
        assert!(!guardian.tool_ids.contains(&"shell"));
        assert!(!guardian.tool_ids.contains(&"git_commit"));
    }

    #[test]
    fn all_roles_have_capabilities() {
        let profiles = all_nonagon_profiles();
        for p in &profiles {
            assert!(
                !p.capabilities.is_empty(),
                "role '{}' must have capabilities, got empty vec",
                p.name
            );
        }
    }

    #[test]
    fn coordinator_has_coordination_capability() {
        let profiles = all_nonagon_profiles();
        let coord = profiles.iter().find(|p| p.name == "coordinator").unwrap();
        assert!(
            coord
                .capabilities
                .iter()
                .any(|c| matches!(&c.scope, CapabilityScope::Custom(s) if s == "coordination")),
            "coordinator must have Custom(\"coordination\") capability for delegation tools"
        );
    }

    #[test]
    fn all_specialists_have_coordination_for_messages() {
        let profiles = all_nonagon_profiles();
        for p in &profiles {
            assert!(
                p.capabilities
                    .iter()
                    .any(|c| matches!(&c.scope, CapabilityScope::Custom(s) if s == "coordination")),
                "role '{}' must have Custom(\"coordination\") capability for message tools",
                p.name
            );
        }
    }

    #[test]
    fn capabilities_match_tool_scopes() {
        let profiles = all_nonagon_profiles();
        for p in &profiles {
            for tool_id in &p.tool_ids {
                let needs_filesystem = matches!(
                    tool_id.as_str(),
                    "file_read"
                        | "file_write"
                        | "file_delete"
                        | "file_move"
                        | "file_copy"
                        | "directory_list"
                        | "grep_search"
                        | "glob_find"
                        | "text_diff"
                        | "project_tree"
                        | "project_outline"
                        | "hash_compute"
                        // Phase 11A
                        | "config_parse"
                        | "csv_query"
                        | "entity_extract"
                        | "pii_detect"
                        | "text_statistics"
                        | "risk_matrix"
                        | "regex_replace"
                        | "html_to_markdown"
                        | "sentiment_analyze"
                        | "image_metadata"
                        // Phase 11B
                        | "document_extract"
                        | "chart_generate"
                        | "diagram_author"
                        | "template_render"
                        | "markdown_export"
                        // Phase 11D (Filesystem)
                        | "log_analyze"
                        | "compliance_check"
                        | "file_patch"
                        | "archive_manage"
                );
                let needs_shell = matches!(
                    tool_id.as_str(),
                    "shell" | "env_read" | "git_status" | "git_diff" | "git_log" | "git_commit"
                );
                let needs_network = matches!(
                    tool_id.as_str(),
                    "web_search"
                        | "http_fetch"
                        | "http_request"
                        | "web_scrape"
                        | "translate"
                        | "notification_send"
                );
                let needs_custom = match tool_id.as_str() {
                    "code_execute" => Some("sandbox"),
                    "sql_query" => Some("database"),
                    "schedule_task" => Some("scheduling"),
                    _ => None,
                };

                if needs_filesystem {
                    assert!(
                        p.capabilities
                            .iter()
                            .any(|c| matches!(c.scope, CapabilityScope::Filesystem { .. })),
                        "role '{}' has tool '{}' requiring Filesystem but no Filesystem capability",
                        p.name,
                        tool_id
                    );
                }
                if needs_shell {
                    assert!(
                        p.capabilities
                            .iter()
                            .any(|c| matches!(c.scope, CapabilityScope::Shell { .. })),
                        "role '{}' has tool '{}' requiring Shell but no Shell capability",
                        p.name,
                        tool_id
                    );
                }
                if needs_network {
                    assert!(
                        p.capabilities
                            .iter()
                            .any(|c| matches!(c.scope, CapabilityScope::Network { .. })),
                        "role '{}' has tool '{}' requiring Network but no Network capability",
                        p.name,
                        tool_id
                    );
                }
                if let Some(custom_name) = needs_custom {
                    assert!(
                        p.capabilities.iter().any(
                            |c| matches!(&c.scope, CapabilityScope::Custom(s) if s == custom_name)
                        ),
                        "role '{}' has tool '{}' requiring Custom(\"{}\") but no matching capability",
                        p.name,
                        tool_id,
                        custom_name
                    );
                }
            }
        }
    }

    #[test]
    fn researcher_has_network_for_web_tools() {
        let profiles = all_nonagon_profiles();
        let researcher = profiles.iter().find(|p| p.name == "researcher").unwrap();
        assert!(researcher.tool_ids.contains(&"web_search".to_string()));
        assert!(researcher.tool_ids.contains(&"http_fetch".to_string()));
        assert!(
            researcher
                .capabilities
                .iter()
                .any(|c| matches!(c.scope, CapabilityScope::Network { .. })),
            "researcher must have Network capability for web_search and http_fetch"
        );
    }

    #[test]
    fn coder_and_executor_have_shell() {
        let profiles = all_nonagon_profiles();
        for role_name in &["coder", "executor"] {
            let p = profiles.iter().find(|p| p.name == *role_name).unwrap();
            assert!(
                p.capabilities
                    .iter()
                    .any(|c| matches!(c.scope, CapabilityScope::Shell { .. })),
                "role '{}' must have Shell capability",
                role_name
            );
        }
    }
}

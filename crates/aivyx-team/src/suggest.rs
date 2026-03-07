//! Capability-aware specialist suggestion for team coordination.
//!
//! [`SuggestSpecialistTool`] helps the coordinator choose the right specialist
//! for a task by matching required tools, capabilities, and skills against
//! each team member's profile. Scoring is deterministic — no LLM call needed.

use std::path::PathBuf;

use async_trait::async_trait;

use aivyx_core::{CapabilityScope, Result, Tool, ToolId};

use crate::config::TeamMemberConfig;
use crate::nonagon::NONAGON_ROLES;

/// A team member's profile data used for capability matching.
#[derive(Debug, Clone)]
pub struct MemberProfile {
    /// Agent name (e.g. "coder", "researcher").
    pub name: String,
    /// Human-readable role label (e.g. "Coder", "Researcher").
    pub role: String,
    /// Tool IDs the member has access to (e.g. "shell", "file_read").
    pub tool_ids: Vec<String>,
    /// Skills the member declares (e.g. "coding", "debugging").
    pub skills: Vec<String>,
    /// Capability scope names (e.g. "Filesystem", "Shell", "Network").
    pub capability_scopes: Vec<String>,
}

/// Scoring weights for specialist matching.
const TOOL_MATCH_WEIGHT: usize = 3;
const CAPABILITY_MATCH_WEIGHT: usize = 2;
const SKILL_MATCH_WEIGHT: usize = 2;
const KEYWORD_MATCH_WEIGHT: usize = 1;

/// Tool that suggests the best specialist for a given task based on
/// required tools, capabilities, and skills.
///
/// Registered only on the coordinator agent. Uses deterministic scoring
/// against the team members' Nonagon profiles — no LLM call needed.
pub struct SuggestSpecialistTool {
    id: ToolId,
    /// Team member profiles with their tools, skills, and capabilities.
    members: Vec<MemberProfile>,
}

impl SuggestSpecialistTool {
    /// Create a new suggestion tool with the given member profiles.
    pub fn new(members: Vec<MemberProfile>) -> Self {
        Self {
            id: ToolId::new(),
            members,
        }
    }
}

#[async_trait]
impl Tool for SuggestSpecialistTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "suggest_specialist"
    }

    fn description(&self) -> &str {
        "Suggest the best specialist for a task based on required tools, capabilities, and skills. \
         Returns a ranked list of team members with match scores."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "required_tools": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Tools the specialist must have (e.g. ['shell', 'file_write'])"
                },
                "required_capabilities": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Capability scopes needed (e.g. ['Shell', 'Network'])"
                },
                "required_skills": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Skills the task requires (e.g. ['coding', 'debugging'])"
                },
                "task_description": {
                    "type": "string",
                    "description": "Optional: natural language task description for keyword matching"
                }
            }
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("coordination".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let required_tools = extract_string_array(&input, "required_tools");
        let required_capabilities = extract_string_array(&input, "required_capabilities");
        let required_skills = extract_string_array(&input, "required_skills");
        let task_description = input["task_description"].as_str().unwrap_or("");

        let keywords: Vec<String> = task_description
            .split_whitespace()
            .filter(|w| w.len() >= 3)
            .map(|w| w.to_lowercase())
            .collect();

        let mut scored: Vec<(usize, &MemberProfile)> = self
            .members
            .iter()
            .map(|m| {
                let score = score_member(
                    m,
                    &required_tools,
                    &required_capabilities,
                    &required_skills,
                    &keywords,
                );
                (score, m)
            })
            .filter(|(score, _)| *score > 0)
            .collect();

        // Sort by score descending, then by name for stability
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));

        let suggestions: Vec<serde_json::Value> = scored
            .iter()
            .map(|(score, m)| {
                let matched_tools: Vec<&str> = required_tools
                    .iter()
                    .filter(|t| m.tool_ids.iter().any(|mt| mt == *t))
                    .map(|s| s.as_str())
                    .collect();
                let matched_caps: Vec<&str> = required_capabilities
                    .iter()
                    .filter(|c| {
                        m.capability_scopes
                            .iter()
                            .any(|mc| mc.eq_ignore_ascii_case(c))
                    })
                    .map(|s| s.as_str())
                    .collect();
                let matched_skills: Vec<&str> = required_skills
                    .iter()
                    .filter(|s| m.skills.iter().any(|ms| ms.eq_ignore_ascii_case(s)))
                    .map(|s| s.as_str())
                    .collect();

                serde_json::json!({
                    "name": m.name,
                    "role": m.role,
                    "score": score,
                    "matched_tools": matched_tools,
                    "matched_capabilities": matched_caps,
                    "matched_skills": matched_skills,
                })
            })
            .collect();

        Ok(serde_json::json!({
            "suggestions": suggestions,
            "total_candidates": suggestions.len(),
        }))
    }
}

/// Score a member against the requirements.
pub(crate) fn score_member(
    member: &MemberProfile,
    required_tools: &[String],
    required_capabilities: &[String],
    required_skills: &[String],
    keywords: &[String],
) -> usize {
    let mut score = 0usize;

    // Tool matches
    for tool in required_tools {
        if member.tool_ids.iter().any(|t| t == tool) {
            score = score.saturating_add(TOOL_MATCH_WEIGHT);
        }
    }

    // Capability matches (case-insensitive)
    for cap in required_capabilities {
        if member
            .capability_scopes
            .iter()
            .any(|c| c.eq_ignore_ascii_case(cap))
        {
            score = score.saturating_add(CAPABILITY_MATCH_WEIGHT);
        }
    }

    // Skill matches (case-insensitive)
    for skill in required_skills {
        if member.skills.iter().any(|s| s.eq_ignore_ascii_case(skill)) {
            score = score.saturating_add(SKILL_MATCH_WEIGHT);
        }
    }

    // Keyword matches against role and skills
    let searchable = format!(
        "{} {} {}",
        member.role.to_lowercase(),
        member.name.to_lowercase(),
        member.skills.join(" ").to_lowercase()
    );
    for kw in keywords {
        if searchable.contains(kw.as_str()) {
            score = score.saturating_add(KEYWORD_MATCH_WEIGHT);
        }
    }

    score
}

/// Extract a string array from a JSON value, returning empty vec if missing.
fn extract_string_array(input: &serde_json::Value, key: &str) -> Vec<String> {
    input[key]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Map a [`CapabilityScope`] to a human-readable string for matching.
fn scope_to_string(scope: &CapabilityScope) -> String {
    match scope {
        CapabilityScope::Filesystem { .. } => "Filesystem".to_string(),
        CapabilityScope::Shell { .. } => "Shell".to_string(),
        CapabilityScope::Network { .. } => "Network".to_string(),
        CapabilityScope::Email { .. } => "Email".to_string(),
        CapabilityScope::Calendar => "Calendar".to_string(),
        CapabilityScope::Custom(name) => name.clone(),
    }
}

/// Build [`MemberProfile`]s from team config members by looking up their
/// Nonagon role definitions.
///
/// For members whose name matches a Nonagon role, the tool_ids, skills, and
/// capabilities are extracted from the static role data. Members without a
/// matching Nonagon role get a minimal profile with just their name and role.
pub fn build_member_profiles(members: &[TeamMemberConfig]) -> Vec<MemberProfile> {
    members
        .iter()
        .map(|m| {
            if let Some(role) = NONAGON_ROLES.iter().find(|r| r.name == m.name) {
                // Map capabilities for this role to scope strings
                let cap_scopes = capabilities_for_role_scopes(role.name);

                MemberProfile {
                    name: m.name.clone(),
                    role: m.role.clone(),
                    tool_ids: role.tool_ids.iter().map(|s| s.to_string()).collect(),
                    skills: role.skills.iter().map(|s| s.to_string()).collect(),
                    capability_scopes: cap_scopes,
                }
            } else {
                // Non-Nonagon member: minimal profile
                MemberProfile {
                    name: m.name.clone(),
                    role: m.role.clone(),
                    tool_ids: Vec::new(),
                    skills: Vec::new(),
                    capability_scopes: Vec::new(),
                }
            }
        })
        .collect()
}

/// Get capability scope strings for a Nonagon role.
///
/// Replicates the logic from `nonagon::capabilities_for_role()` but returns
/// string labels for matching instead of `ProfileCapability` instances.
fn capabilities_for_role_scopes(role_name: &str) -> Vec<String> {
    // Build the same scope list as nonagon::capabilities_for_role()
    let scopes: Vec<CapabilityScope> = match role_name {
        "coordinator" => vec![
            CapabilityScope::Custom("coordination".into()),
            CapabilityScope::Filesystem {
                root: PathBuf::from("/"),
            },
            CapabilityScope::Network {
                hosts: vec![],
                ports: vec![],
            },
            CapabilityScope::Custom("scheduling".into()),
        ],
        "researcher" => vec![
            CapabilityScope::Filesystem {
                root: PathBuf::from("/"),
            },
            CapabilityScope::Network {
                hosts: vec![],
                ports: vec![],
            },
            CapabilityScope::Custom("coordination".into()),
        ],
        "analyst" => vec![
            CapabilityScope::Filesystem {
                root: PathBuf::from("/"),
            },
            CapabilityScope::Custom("coordination".into()),
            CapabilityScope::Custom("sandbox".into()),
            CapabilityScope::Custom("database".into()),
        ],
        "guardian" => vec![
            CapabilityScope::Filesystem {
                root: PathBuf::from("/"),
            },
            CapabilityScope::Shell {
                allowed_commands: vec![],
            },
            CapabilityScope::Custom("coordination".into()),
        ],
        "coder" => vec![
            CapabilityScope::Filesystem {
                root: PathBuf::from("/"),
            },
            CapabilityScope::Shell {
                allowed_commands: vec![],
            },
            CapabilityScope::Custom("coordination".into()),
        ],
        "executor" => vec![
            CapabilityScope::Filesystem {
                root: PathBuf::from("/"),
            },
            CapabilityScope::Shell {
                allowed_commands: vec![],
            },
            CapabilityScope::Custom("coordination".into()),
            CapabilityScope::Network {
                hosts: vec![],
                ports: vec![],
            },
            CapabilityScope::Custom("scheduling".into()),
        ],
        "reviewer" => vec![
            CapabilityScope::Filesystem {
                root: PathBuf::from("/"),
            },
            CapabilityScope::Shell {
                allowed_commands: vec![],
            },
            CapabilityScope::Custom("coordination".into()),
        ],
        "writer" => vec![
            CapabilityScope::Filesystem {
                root: PathBuf::from("/"),
            },
            CapabilityScope::Network {
                hosts: vec![],
                ports: vec![],
            },
            CapabilityScope::Custom("coordination".into()),
        ],
        "planner" => vec![
            CapabilityScope::Filesystem {
                root: PathBuf::from("/"),
            },
            CapabilityScope::Custom("coordination".into()),
            CapabilityScope::Custom("scheduling".into()),
        ],
        _ => vec![CapabilityScope::Filesystem {
            root: PathBuf::from("/"),
        }],
    };

    // Deduplicate scope strings
    let mut result: Vec<String> = scopes.iter().map(scope_to_string).collect();
    result.sort();
    result.dedup();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TeamMemberConfig;

    fn sample_members() -> Vec<MemberProfile> {
        vec![
            MemberProfile {
                name: "coder".into(),
                role: "Coder".into(),
                tool_ids: vec![
                    "file_read".into(),
                    "file_write".into(),
                    "shell".into(),
                    "grep_search".into(),
                    "glob_find".into(),
                    "text_diff".into(),
                    "directory_list".into(),
                    "git_status".into(),
                    "git_diff".into(),
                    "git_log".into(),
                    "git_commit".into(),
                ],
                skills: vec![
                    "coding".into(),
                    "debugging".into(),
                    "testing".into(),
                    "refactoring".into(),
                ],
                capability_scopes: vec!["Filesystem".into(), "Shell".into(), "coordination".into()],
            },
            MemberProfile {
                name: "researcher".into(),
                role: "Researcher".into(),
                tool_ids: vec![
                    "file_read".into(),
                    "file_write".into(),
                    "web_search".into(),
                    "http_fetch".into(),
                    "grep_search".into(),
                    "glob_find".into(),
                ],
                skills: vec![
                    "summarization".into(),
                    "fact_checking".into(),
                    "source_evaluation".into(),
                ],
                capability_scopes: vec![
                    "Filesystem".into(),
                    "Network".into(),
                    "coordination".into(),
                ],
            },
            MemberProfile {
                name: "writer".into(),
                role: "Writer".into(),
                tool_ids: vec![
                    "file_read".into(),
                    "file_write".into(),
                    "directory_list".into(),
                    "glob_find".into(),
                    "grep_search".into(),
                ],
                skills: vec![
                    "technical_writing".into(),
                    "documentation".into(),
                    "api_documentation".into(),
                ],
                capability_scopes: vec!["Filesystem".into(), "coordination".into()],
            },
        ]
    }

    #[test]
    fn score_tool_match() {
        let members = sample_members();
        let score = score_member(
            &members[0], // coder
            &["shell".to_string(), "file_write".to_string()],
            &[],
            &[],
            &[],
        );
        // 2 tools matched × 3 weight = 6
        assert_eq!(score, 6);
    }

    #[test]
    fn score_capability_match() {
        let members = sample_members();
        let score = score_member(
            &members[1], // researcher
            &[],
            &["Network".to_string()],
            &[],
            &[],
        );
        // 1 capability × 2 weight = 2
        assert_eq!(score, 2);
    }

    #[test]
    fn score_skill_match() {
        let members = sample_members();
        let score = score_member(
            &members[0], // coder
            &[],
            &[],
            &["coding".to_string(), "debugging".to_string()],
            &[],
        );
        // 2 skills × 2 weight = 4
        assert_eq!(score, 4);
    }

    #[test]
    fn score_keyword_match() {
        let members = sample_members();
        let score = score_member(
            &members[2], // writer
            &[],
            &[],
            &[],
            &["writing".to_string(), "documentation".to_string()],
        );
        // "writing" found in skills ("technical_writing"), "documentation" found in skills
        // 2 keywords × 1 weight = 2
        assert_eq!(score, 2);
    }

    #[test]
    fn no_match_returns_empty() {
        let members = sample_members();
        let score = score_member(
            &members[2], // writer
            &["shell".to_string()],
            &[],
            &[],
            &[],
        );
        assert_eq!(score, 0);
    }

    #[tokio::test]
    async fn sorted_by_score_descending() {
        let members = sample_members();
        let tool = SuggestSpecialistTool::new(members);

        let result = tool
            .execute(serde_json::json!({
                "required_tools": ["shell", "file_write"],
                "required_skills": ["coding"]
            }))
            .await
            .unwrap();

        let suggestions = result["suggestions"].as_array().unwrap();
        // Coder should be first (shell + file_write = 6 tool, coding = 2 skill = 8)
        assert!(!suggestions.is_empty());
        assert_eq!(suggestions[0]["name"], "coder");
        assert_eq!(suggestions[0]["score"], 8);
    }

    #[tokio::test]
    async fn no_match_excludes_zero_score() {
        let members = sample_members();
        let tool = SuggestSpecialistTool::new(members);

        let result = tool
            .execute(serde_json::json!({
                "required_capabilities": ["Network"]
            }))
            .await
            .unwrap();

        let suggestions = result["suggestions"].as_array().unwrap();
        // Only researcher has Network capability
        assert_eq!(result["total_candidates"], 1);
        assert_eq!(suggestions[0]["name"], "researcher");
    }

    #[test]
    fn build_profiles_from_nonagon() {
        let members: Vec<TeamMemberConfig> = crate::nonagon::NONAGON_ROLES
            .iter()
            .map(|r| TeamMemberConfig {
                name: r.name.to_string(),
                role: r.role.to_string(),
            })
            .collect();

        let profiles = build_member_profiles(&members);
        assert_eq!(profiles.len(), 9);

        // Verify coder has shell
        let coder = profiles.iter().find(|p| p.name == "coder").unwrap();
        assert!(coder.tool_ids.contains(&"shell".to_string()));
        assert!(coder.capability_scopes.contains(&"Shell".to_string()));

        // Verify researcher has network
        let researcher = profiles.iter().find(|p| p.name == "researcher").unwrap();
        assert!(
            researcher
                .capability_scopes
                .contains(&"Network".to_string())
        );
        assert!(researcher.tool_ids.contains(&"web_search".to_string()));
    }

    #[test]
    fn build_profiles_non_nonagon_member() {
        let members = vec![TeamMemberConfig {
            name: "custom-agent".into(),
            role: "Custom".into(),
        }];

        let profiles = build_member_profiles(&members);
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].name, "custom-agent");
        assert!(profiles[0].tool_ids.is_empty());
        assert!(profiles[0].skills.is_empty());
    }

    #[test]
    fn capability_match_case_insensitive() {
        let members = sample_members();
        let score = score_member(
            &members[1], // researcher
            &[],
            &["network".to_string()], // lowercase
            &[],
            &[],
        );
        // Should match "Network" case-insensitively
        assert_eq!(score, 2);
    }

    #[tokio::test]
    async fn matched_fields_populated() {
        let members = sample_members();
        let tool = SuggestSpecialistTool::new(members);

        let result = tool
            .execute(serde_json::json!({
                "required_tools": ["shell"],
                "required_capabilities": ["Shell"],
                "required_skills": ["coding"]
            }))
            .await
            .unwrap();

        let suggestions = result["suggestions"].as_array().unwrap();
        let coder = &suggestions[0];
        assert_eq!(coder["matched_tools"].as_array().unwrap(), &["shell"]);
        assert_eq!(
            coder["matched_capabilities"].as_array().unwrap(),
            &["Shell"]
        );
        assert_eq!(coder["matched_skills"].as_array().unwrap(), &["coding"]);
    }
}

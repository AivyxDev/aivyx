//! LLM-driven task decomposition for team coordination.
//!
//! [`DecomposeTaskTool`] helps the coordinator break a complex goal into
//! specialist-annotated subtasks with dependency information. Each subtask
//! is mapped to a team member and given a priority, enabling the coordinator
//! to delegate work systematically rather than ad-hoc.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use tracing::info;

use aivyx_agent::AgentSession;
use aivyx_core::{AivyxError, CapabilityScope, Result, Tool, ToolId};
use aivyx_llm::{ChatMessage, ChatRequest};

/// System prompt that instructs the LLM to decompose a goal into subtasks.
const DECOMPOSE_SYSTEM_PROMPT: &str = r#"You are a team task decomposer. Given a complex goal and a list of available team members with their roles, decompose the goal into subtasks. Each subtask should be assigned to the most appropriate team member.

Output ONLY a JSON array where each element has:
- "description": a clear, actionable description of the subtask
- "assigned_to": the name of the team member best suited for this subtask
- "depends_on": an array of subtask indices (0-based) that must complete before this one starts (empty array if none)
- "priority": "high", "medium", or "low"

Rules:
- Each subtask should be completable by a single specialist
- Prefer parallel subtasks where possible (fewer dependencies)
- Do NOT assign subtasks to the coordinator — only specialists
- Keep descriptions specific and actionable
- 3-10 subtasks depending on goal complexity

Do NOT include any text before or after the JSON array.
Do NOT wrap it in markdown code fences."#;

/// Intermediate struct for deserializing the LLM's JSON output.
#[derive(Debug, Deserialize)]
struct PlannedSubtask {
    description: String,
    assigned_to: String,
    #[serde(default)]
    depends_on: Vec<usize>,
    #[serde(default = "default_priority")]
    priority: String,
}

fn default_priority() -> String {
    "medium".to_string()
}

/// Tool that decomposes a complex goal into specialist-annotated subtasks.
///
/// Registered only on the coordinator agent. Uses the LLM to analyze a goal
/// and produce a structured plan where each subtask is mapped to a team member
/// with dependency information and priority levels.
///
/// The coordinator can then use `delegate_task` to execute the plan step by step.
pub struct DecomposeTaskTool {
    id: ToolId,
    session: Arc<AgentSession>,
    /// Available team members: `(name, role)`.
    available_roles: Vec<(String, String)>,
}

impl DecomposeTaskTool {
    /// Create a new decomposition tool.
    ///
    /// `available_roles` lists the team members the coordinator can delegate to,
    /// as `(name, role)` pairs. These are included in the LLM prompt so it can
    /// assign subtasks to real team members.
    pub fn new(session: Arc<AgentSession>, available_roles: Vec<(String, String)>) -> Self {
        Self {
            id: ToolId::new(),
            session,
            available_roles,
        }
    }
}

#[async_trait]
impl Tool for DecomposeTaskTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "decompose_task"
    }

    fn description(&self) -> &str {
        "Decompose a complex goal into specialist-annotated subtasks with dependencies and priorities. \
         Returns a structured plan mapping each subtask to the best team member."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "goal": {
                    "type": "string",
                    "description": "The complex goal to decompose into subtasks"
                },
                "max_subtasks": {
                    "type": "integer",
                    "description": "Maximum number of subtasks (default: 7, max: 12)"
                }
            },
            "required": ["goal"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("coordination".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let goal = input["goal"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("decompose_task: missing 'goal'".into()))?;
        let max_subtasks = input["max_subtasks"].as_u64().unwrap_or(7).clamp(3, 12) as usize;

        info!(
            "Decomposing goal into up to {max_subtasks} subtasks for {} team members",
            self.available_roles.len()
        );

        // Build user message with goal and team member roster
        let mut user_msg = format!("Goal: {goal}\n\nAvailable team members:\n");
        for (name, role) in &self.available_roles {
            user_msg.push_str(&format!("- {name} (role: {role})\n"));
        }
        user_msg.push_str(&format!(
            "\nDecompose into at most {max_subtasks} subtasks."
        ));

        // Call LLM for decomposition
        let provider = self.session.create_llm_provider()?;
        let request = ChatRequest {
            system_prompt: Some(DECOMPOSE_SYSTEM_PROMPT.to_string()),
            messages: vec![ChatMessage::user(&user_msg)],
            tools: vec![],
            model: None,
            max_tokens: 4096,
        };

        let response = provider.chat(&request).await?;
        let text = &response.message.content;

        if text.is_empty() {
            return Err(AivyxError::Agent(
                "decompose_task: LLM returned empty response".into(),
            ));
        }

        // Parse the JSON response
        let subtasks = parse_decomposition(text)?;

        // Build parallel groups from the dependency graph
        let groups = compute_parallel_groups(&subtasks);

        // Build the structured output
        let plan: Vec<serde_json::Value> = subtasks
            .iter()
            .enumerate()
            .map(|(i, s)| {
                serde_json::json!({
                    "index": i,
                    "description": s.description,
                    "assigned_to": s.assigned_to,
                    "depends_on": s.depends_on,
                    "priority": s.priority,
                })
            })
            .collect();

        Ok(serde_json::json!({
            "plan": plan,
            "total": subtasks.len(),
            "parallel_groups": groups,
        }))
    }
}

/// Parse the LLM response into a list of subtasks.
///
/// Handles both raw JSON arrays and markdown-fenced JSON.
fn parse_decomposition(text: &str) -> Result<Vec<PlannedSubtask>> {
    let trimmed = strip_code_fences(text.trim());

    let planned: Vec<PlannedSubtask> = serde_json::from_str(trimmed)
        .map_err(|e| AivyxError::Agent(format!("decompose_task: failed to parse JSON: {e}")))?;

    if planned.is_empty() {
        return Err(AivyxError::Agent(
            "decompose_task: plan contains zero subtasks".into(),
        ));
    }

    Ok(planned)
}

/// Strip markdown code fences if present.
fn strip_code_fences(text: &str) -> &str {
    let text = text.strip_prefix("```json").unwrap_or(text);
    let text = text.strip_prefix("```").unwrap_or(text);
    let text = text.strip_suffix("```").unwrap_or(text);
    text.trim()
}

/// Compute parallel execution groups from a dependency graph.
///
/// Returns a list of groups where each group contains subtask indices that
/// can run concurrently. Groups are ordered by dependency depth: group 0
/// has no dependencies, group 1 depends only on group 0, etc.
///
/// Uses topological depth assignment: each subtask's depth is
/// `1 + max(depth of dependencies)`, or 0 if it has no dependencies.
fn compute_parallel_groups(subtasks: &[PlannedSubtask]) -> Vec<Vec<usize>> {
    let n = subtasks.len();
    let mut depths = vec![0usize; n];

    // Compute depth for each subtask (simple iterative approach)
    // Repeat until stable — handles any valid DAG ordering.
    for _ in 0..n {
        let mut changed = false;
        for (i, task) in subtasks.iter().enumerate() {
            for &dep in &task.depends_on {
                if dep < n {
                    let new_depth = depths[dep].saturating_add(1);
                    if new_depth > depths[i] {
                        depths[i] = new_depth;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }

    // Group by depth
    let max_depth = depths.iter().copied().max().unwrap_or(0);
    let mut groups: Vec<Vec<usize>> = Vec::with_capacity(max_depth.saturating_add(1));
    for d in 0..=max_depth {
        let group: Vec<usize> = (0..n).filter(|&i| depths[i] == d).collect();
        if !group.is_empty() {
            groups.push(group);
        }
    }

    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_subtasks() {
        let json = r#"[
            {"description": "Research the topic", "assigned_to": "researcher", "depends_on": [], "priority": "high"},
            {"description": "Write the code", "assigned_to": "coder", "depends_on": [0], "priority": "high"}
        ]"#;
        let subtasks = parse_decomposition(json).unwrap();
        assert_eq!(subtasks.len(), 2);
        assert_eq!(subtasks[0].description, "Research the topic");
        assert_eq!(subtasks[0].assigned_to, "researcher");
        assert!(subtasks[0].depends_on.is_empty());
        assert_eq!(subtasks[0].priority, "high");
        assert_eq!(subtasks[1].depends_on, vec![0]);
    }

    #[test]
    fn parse_fenced_json() {
        let json =
            "```json\n[\n  {\"description\": \"Step 1\", \"assigned_to\": \"coder\"}\n]\n```";
        let subtasks = parse_decomposition(json).unwrap();
        assert_eq!(subtasks.len(), 1);
        assert_eq!(subtasks[0].description, "Step 1");
    }

    #[test]
    fn parse_bare_fenced_json() {
        let json = "```\n[{\"description\": \"Do thing\", \"assigned_to\": \"executor\"}]\n```";
        let subtasks = parse_decomposition(json).unwrap();
        assert_eq!(subtasks.len(), 1);
    }

    #[test]
    fn parse_missing_depends_defaults_empty() {
        let json = r#"[{"description": "Think about it", "assigned_to": "analyst"}]"#;
        let subtasks = parse_decomposition(json).unwrap();
        assert!(subtasks[0].depends_on.is_empty());
    }

    #[test]
    fn parse_missing_priority_defaults_medium() {
        let json = r#"[{"description": "Think about it", "assigned_to": "analyst"}]"#;
        let subtasks = parse_decomposition(json).unwrap();
        assert_eq!(subtasks[0].priority, "medium");
    }

    #[test]
    fn parse_empty_array_fails() {
        let json = "[]";
        let result = parse_decomposition(json);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("zero subtasks"));
    }

    #[test]
    fn parse_invalid_json_fails() {
        let json = "This is not valid JSON";
        let result = parse_decomposition(json);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("parse JSON"));
    }

    #[test]
    fn compute_groups_no_dependencies() {
        let subtasks = vec![
            PlannedSubtask {
                description: "A".into(),
                assigned_to: "a".into(),
                depends_on: vec![],
                priority: "high".into(),
            },
            PlannedSubtask {
                description: "B".into(),
                assigned_to: "b".into(),
                depends_on: vec![],
                priority: "high".into(),
            },
        ];
        let groups = compute_parallel_groups(&subtasks);
        assert_eq!(groups, vec![vec![0, 1]]);
    }

    #[test]
    fn compute_groups_sequential() {
        let subtasks = vec![
            PlannedSubtask {
                description: "A".into(),
                assigned_to: "a".into(),
                depends_on: vec![],
                priority: "high".into(),
            },
            PlannedSubtask {
                description: "B".into(),
                assigned_to: "b".into(),
                depends_on: vec![0],
                priority: "medium".into(),
            },
            PlannedSubtask {
                description: "C".into(),
                assigned_to: "c".into(),
                depends_on: vec![1],
                priority: "low".into(),
            },
        ];
        let groups = compute_parallel_groups(&subtasks);
        assert_eq!(groups, vec![vec![0], vec![1], vec![2]]);
    }

    #[test]
    fn compute_groups_diamond() {
        // A (0) -> B (1) -> D (3)
        // A (0) -> C (2) -> D (3)
        let subtasks = vec![
            PlannedSubtask {
                description: "A".into(),
                assigned_to: "a".into(),
                depends_on: vec![],
                priority: "high".into(),
            },
            PlannedSubtask {
                description: "B".into(),
                assigned_to: "b".into(),
                depends_on: vec![0],
                priority: "high".into(),
            },
            PlannedSubtask {
                description: "C".into(),
                assigned_to: "c".into(),
                depends_on: vec![0],
                priority: "medium".into(),
            },
            PlannedSubtask {
                description: "D".into(),
                assigned_to: "d".into(),
                depends_on: vec![1, 2],
                priority: "high".into(),
            },
        ];
        let groups = compute_parallel_groups(&subtasks);
        assert_eq!(groups, vec![vec![0], vec![1, 2], vec![3]]);
    }

    #[test]
    fn compute_groups_ignores_out_of_bounds() {
        let subtasks = vec![PlannedSubtask {
            description: "A".into(),
            assigned_to: "a".into(),
            depends_on: vec![99], // out of bounds, ignored
            priority: "high".into(),
        }];
        let groups = compute_parallel_groups(&subtasks);
        assert_eq!(groups, vec![vec![0]]);
    }

    #[test]
    fn strip_fences_json() {
        assert_eq!(strip_code_fences("```json\n[1]\n```"), "[1]");
    }

    #[test]
    fn strip_fences_bare() {
        assert_eq!(strip_code_fences("```\n[1]\n```"), "[1]");
    }

    #[test]
    fn strip_fences_none() {
        assert_eq!(strip_code_fences("[1]"), "[1]");
    }
}

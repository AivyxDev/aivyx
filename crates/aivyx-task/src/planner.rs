//! LLM-driven mission planning.
//!
//! [`plan_mission`] sends a structured prompt to an LLM provider that instructs
//! it to decompose a goal into a flat list of sequential steps. The response is
//! parsed as a JSON array and converted into [`Step`] values.
//!
//! [`plan_mission_dag`] uses a dependency-aware prompt that allows the LLM to
//! specify `depends_on` relationships between steps, enabling parallel execution
//! of independent steps.

use aivyx_core::{AivyxError, Result};
use aivyx_llm::{ChatMessage, ChatRequest, LlmProvider};

use crate::dag;
use crate::types::{Step, StepStatus};

/// System prompt that instructs the LLM to decompose a goal into steps.
const PLANNING_SYSTEM_PROMPT: &str = r#"You are a task planner. Given a user goal, decompose it into 3-7 sequential steps that an AI agent can execute one at a time.

Output ONLY a JSON array where each element has:
- "description": a clear, actionable description of what this step accomplishes
- "tool_hints": an array of tool names likely needed (e.g., "web_search", "http_fetch", "file_write", "file_read", "memory_store", "shell")

Do NOT include any text before or after the JSON array. Do NOT wrap it in markdown code fences.

Example output:
[
  {"description": "Search for information about the topic", "tool_hints": ["web_search"]},
  {"description": "Read detailed documentation pages", "tool_hints": ["http_fetch"]},
  {"description": "Synthesize findings into a coherent summary", "tool_hints": []},
  {"description": "Write the summary to the specified file", "tool_hints": ["file_write"]}
]"#;

/// Intermediate struct for deserializing the LLM's JSON output.
#[derive(serde::Deserialize)]
struct PlannedStep {
    description: String,
    #[serde(default)]
    tool_hints: Vec<String>,
    #[serde(default)]
    depends_on: Vec<usize>,
    #[serde(default)]
    kind: Option<crate::types::StepKind>,
}

/// Ask the LLM to decompose a goal into sequential steps.
///
/// Sends a planning prompt to the provider, parses the JSON response, and
/// returns the steps with [`StepStatus::Pending`] status.
pub async fn plan_mission(
    provider: &dyn LlmProvider,
    goal: &str,
    max_tokens: u32,
) -> Result<Vec<Step>> {
    let request = ChatRequest {
        system_prompt: Some(PLANNING_SYSTEM_PROMPT.to_string()),
        messages: vec![ChatMessage::user(goal)],
        tools: vec![],
        model: None,
        max_tokens,
    };

    let response = provider.chat(&request).await?;

    let text = &response.message.content;
    if text.is_empty() {
        return Err(AivyxError::Task("planner returned empty response".into()));
    }

    parse_plan_response(text)
}

/// System prompt for DAG-aware planning with dependency relationships.
const DAG_PLANNING_SYSTEM_PROMPT: &str = r#"You are a task planner. Given a user goal, decompose it into 3-10 steps with dependency relationships. Steps that don't depend on each other will run in parallel.

Output ONLY a JSON array where each element has:
- "description": a clear, actionable description of what this step accomplishes
- "tool_hints": an array of tool names likely needed (e.g., "web_search", "http_fetch", "file_write", "file_read", "memory_store", "shell")
- "depends_on": an array of step indices (0-based) that must complete before this step can start. Use [] for steps with no dependencies.

Optionally, you may include review steps:
- "kind": {"kind": "Reflect", "target_step": N, "max_depth": 2, "current_depth": 0} — re-evaluates step N's output
- "kind": {"kind": "Approval", "context": "description of what needs approval", "timeout_secs": 300, "auto_approve_on_timeout": false} — pauses for human approval

Steps with no dependencies will start immediately. Maximize parallelism where steps are truly independent.

Do NOT include any text before or after the JSON array. Do NOT wrap it in markdown code fences.

Example output:
[
  {"description": "Search for information about topic A", "tool_hints": ["web_search"], "depends_on": []},
  {"description": "Search for information about topic B", "tool_hints": ["web_search"], "depends_on": []},
  {"description": "Synthesize findings from both searches", "tool_hints": [], "depends_on": [0, 1]},
  {"description": "Write the final report", "tool_hints": ["file_write"], "depends_on": [2]}
]"#;

/// Ask the LLM to decompose a goal into steps with dependency relationships.
///
/// Unlike [`plan_mission`], this produces steps with `depends_on` fields,
/// enabling parallel execution of independent steps.
pub async fn plan_mission_dag(
    provider: &dyn LlmProvider,
    goal: &str,
    max_tokens: u32,
) -> Result<Vec<Step>> {
    let request = ChatRequest {
        system_prompt: Some(DAG_PLANNING_SYSTEM_PROMPT.to_string()),
        messages: vec![ChatMessage::user(goal)],
        tools: vec![],
        model: None,
        max_tokens,
    };

    let response = provider.chat(&request).await?;

    let text = &response.message.content;
    if text.is_empty() {
        return Err(AivyxError::Task("planner returned empty response".into()));
    }

    let steps = parse_plan_response(text)?;

    // Validate the DAG before returning
    dag::validate_dag(&steps)?;

    Ok(steps)
}

/// Parse the LLM response into a list of steps.
///
/// Handles both raw JSON arrays and markdown-fenced JSON (```json ... ```).
pub fn parse_plan_response(text: &str) -> Result<Vec<Step>> {
    let trimmed = strip_code_fences(text.trim());

    let planned: Vec<PlannedStep> = serde_json::from_str(trimmed)
        .map_err(|e| AivyxError::Task(format!("failed to parse plan JSON: {e}")))?;

    if planned.is_empty() {
        return Err(AivyxError::Task("planner returned zero steps".into()));
    }

    let steps = planned
        .into_iter()
        .enumerate()
        .map(|(i, p)| Step {
            index: i,
            description: p.description,
            tool_hints: p.tool_hints,
            status: StepStatus::Pending,
            prompt: None,
            result: None,
            retries: 0,
            started_at: None,
            completed_at: None,
            depends_on: p.depends_on,
            kind: p.kind.unwrap_or_default(),
        })
        .collect();

    Ok(steps)
}

/// Strip markdown code fences if present.
fn strip_code_fences(text: &str) -> &str {
    let text = text.strip_prefix("```json").unwrap_or(text);
    let text = text.strip_prefix("```").unwrap_or(text);
    let text = text.strip_suffix("```").unwrap_or(text);
    text.trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_json() {
        let json = r#"[
            {"description": "Search for info", "tool_hints": ["web_search"]},
            {"description": "Write summary", "tool_hints": ["file_write"]}
        ]"#;
        let steps = parse_plan_response(json).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].index, 0);
        assert_eq!(steps[0].description, "Search for info");
        assert_eq!(steps[0].tool_hints, vec!["web_search"]);
        assert_eq!(steps[1].index, 1);
        assert!(matches!(steps[0].status, StepStatus::Pending));
    }

    #[test]
    fn parse_markdown_fenced_json() {
        let json = "```json\n[\n  {\"description\": \"Step 1\", \"tool_hints\": []}\n]\n```";
        let steps = parse_plan_response(json).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].description, "Step 1");
    }

    #[test]
    fn parse_bare_fenced_json() {
        let json = "```\n[{\"description\": \"Do thing\", \"tool_hints\": [\"shell\"]}]\n```";
        let steps = parse_plan_response(json).unwrap();
        assert_eq!(steps.len(), 1);
    }

    #[test]
    fn parse_empty_array_fails() {
        let json = "[]";
        let result = parse_plan_response(json);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("zero steps"));
    }

    #[test]
    fn parse_invalid_json_fails() {
        let json = "This is not valid JSON";
        let result = parse_plan_response(json);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("parse plan JSON"));
    }

    #[test]
    fn missing_tool_hints_defaults_to_empty() {
        let json = r#"[{"description": "Think about it"}]"#;
        let steps = parse_plan_response(json).unwrap();
        assert!(steps[0].tool_hints.is_empty());
    }

    #[test]
    fn parse_dag_with_dependencies() {
        let json = r#"[
            {"description": "Search A", "tool_hints": ["web_search"], "depends_on": []},
            {"description": "Search B", "tool_hints": ["web_search"], "depends_on": []},
            {"description": "Combine results", "tool_hints": [], "depends_on": [0, 1]}
        ]"#;
        let steps = parse_plan_response(json).unwrap();
        assert_eq!(steps.len(), 3);
        assert!(steps[0].depends_on.is_empty());
        assert!(steps[1].depends_on.is_empty());
        assert_eq!(steps[2].depends_on, vec![0, 1]);
    }

    #[test]
    fn parse_dag_missing_depends_on_defaults_to_empty() {
        let json = r#"[
            {"description": "Step 1", "tool_hints": []},
            {"description": "Step 2", "tool_hints": [], "depends_on": [0]}
        ]"#;
        let steps = parse_plan_response(json).unwrap();
        assert!(steps[0].depends_on.is_empty());
        assert_eq!(steps[1].depends_on, vec![0]);
    }

    #[test]
    fn parse_dag_with_step_kind() {
        let json = r#"[
            {"description": "Do work", "tool_hints": ["shell"], "depends_on": []},
            {"description": "Review work", "tool_hints": [], "depends_on": [0],
             "kind": {"kind": "Reflect", "target_step": 0, "max_depth": 2, "current_depth": 0}}
        ]"#;
        let steps = parse_plan_response(json).unwrap();
        assert_eq!(steps.len(), 2);
        assert!(matches!(steps[0].kind, crate::types::StepKind::Execute));
        match &steps[1].kind {
            crate::types::StepKind::Reflect {
                target_step,
                max_depth,
                current_depth,
            } => {
                assert_eq!(*target_step, 0);
                assert_eq!(*max_depth, 2);
                assert_eq!(*current_depth, 0);
            }
            _ => panic!("expected Reflect step kind"),
        }
    }

    #[test]
    fn parse_dag_with_approval_kind() {
        let json = r#"[
            {"description": "Prepare deployment", "tool_hints": ["shell"], "depends_on": []},
            {"description": "Approve deployment", "tool_hints": [], "depends_on": [0],
             "kind": {"kind": "Approval", "context": "Deploy to production?", "timeout_secs": 300, "auto_approve_on_timeout": false}}
        ]"#;
        let steps = parse_plan_response(json).unwrap();
        match &steps[1].kind {
            crate::types::StepKind::Approval {
                context,
                timeout_secs,
                auto_approve_on_timeout,
            } => {
                assert_eq!(context, "Deploy to production?");
                assert_eq!(*timeout_secs, Some(300));
                assert!(!auto_approve_on_timeout);
            }
            _ => panic!("expected Approval step kind"),
        }
    }
}

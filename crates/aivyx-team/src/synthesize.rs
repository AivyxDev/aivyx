//! LLM-driven result synthesis for team coordination.
//!
//! [`SynthesizeResultsTool`] aggregates specialist outputs into a coherent
//! summary, reducing context bloat by distilling raw results into key findings.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::info;

use aivyx_agent::AgentSession;
use aivyx_core::{AivyxError, CapabilityScope, Result, Tool, ToolId};
use aivyx_llm::{ChatMessage, ChatRequest};

use crate::delegation::DelegationTracker;

/// Maximum characters per specialist result in the synthesis prompt.
const MAX_RESULT_CHARS: usize = 2000;

/// System prompt instructing the LLM to synthesize specialist results.
const SYNTHESIZE_SYSTEM_PROMPT: &str = r#"You are a team result synthesizer. Given an original goal and specialist results, produce a coherent, structured synthesis. Merge overlapping findings, resolve contradictions, and highlight key insights.

Your output should include:
- A concise executive summary (2-3 sentences)
- Key findings organized by topic
- Any unresolved issues or contradictions between specialist outputs
- Recommended next steps (if applicable)

Do NOT simply repeat raw specialist output — synthesize and distill it.
Be concise but thorough. Focus on what matters for the original goal."#;

/// Tool that synthesizes all collected specialist results into a coherent summary.
///
/// Registered only on the coordinator agent. Uses the LLM to merge, deduplicate,
/// and summarize outputs from multiple specialists, reducing the coordinator's
/// need to re-tokenize every raw specialist response.
pub struct SynthesizeResultsTool {
    id: ToolId,
    session: Arc<AgentSession>,
    tracker: DelegationTracker,
}

impl SynthesizeResultsTool {
    /// Create a new synthesis tool.
    ///
    /// `tracker` is the shared delegation tracker that collects results from
    /// `delegate_task` and `query_agent` calls.
    pub fn new(session: Arc<AgentSession>, tracker: DelegationTracker) -> Self {
        Self {
            id: ToolId::new(),
            session,
            tracker,
        }
    }
}

#[async_trait]
impl Tool for SynthesizeResultsTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "synthesize_results"
    }

    fn description(&self) -> &str {
        "Synthesize all collected specialist results into a coherent summary. \
         Merges overlapping findings, resolves contradictions, and distills key insights."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "goal": {
                    "type": "string",
                    "description": "The original goal that specialists were working toward"
                },
                "include_errors": {
                    "type": "boolean",
                    "description": "Include failed specialist results in the synthesis. Default: false."
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
            .ok_or_else(|| AivyxError::Agent("synthesize_results: missing 'goal'".into()))?;
        let include_errors = input["include_errors"].as_bool().unwrap_or(false);

        let results = self.tracker.lock().await.clone();

        if results.is_empty() {
            return Ok(serde_json::json!({
                "synthesis": "No specialist results to synthesize.",
                "source_count": 0,
                "error_count": 0,
            }));
        }

        let completed: Vec<_> = results.iter().filter(|r| r.status == "completed").collect();
        let errors: Vec<_> = results.iter().filter(|r| r.status != "completed").collect();

        if completed.is_empty() && !include_errors {
            return Ok(serde_json::json!({
                "synthesis": "All specialist results were errors. Set include_errors=true to include them.",
                "source_count": 0,
                "error_count": errors.len(),
            }));
        }

        info!(
            "Synthesizing {} completed + {} error results for goal",
            completed.len(),
            errors.len()
        );

        // Build the user message with specialist results
        let mut user_msg = format!("Original goal: {goal}\n\nSpecialist results:\n\n");
        let mut source_count = 0;

        for (i, r) in completed.iter().enumerate() {
            let truncated = truncate_result(&r.response, MAX_RESULT_CHARS);
            user_msg.push_str(&format!(
                "{}. [{}] (task: {})\n{}\n\n",
                i + 1,
                r.agent,
                r.task,
                truncated
            ));
            source_count += 1;
        }

        if include_errors && !errors.is_empty() {
            user_msg.push_str("Failed specialist attempts:\n\n");
            for r in &errors {
                let truncated = truncate_result(&r.response, MAX_RESULT_CHARS);
                user_msg.push_str(&format!(
                    "- [{}] (task: {}) ERROR: {}\n",
                    r.agent, r.task, truncated
                ));
            }
        }

        // Call LLM for synthesis
        let provider = self.session.create_llm_provider()?;
        let request = ChatRequest {
            system_prompt: Some(SYNTHESIZE_SYSTEM_PROMPT.to_string()),
            messages: vec![ChatMessage::user(&user_msg)],
            tools: vec![],
            model: None,
            max_tokens: 8192,
        };

        let response = provider.chat(&request).await?;
        let synthesis = &response.message.content;

        if synthesis.is_empty() {
            return Err(AivyxError::Agent(
                "synthesize_results: LLM returned empty synthesis".into(),
            ));
        }

        Ok(serde_json::json!({
            "synthesis": synthesis,
            "source_count": source_count,
            "error_count": errors.len(),
        }))
    }
}

/// Truncate a result string to `max_chars`, respecting UTF-8 boundaries.
fn truncate_result(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        text.to_string()
    } else {
        let boundary = text.floor_char_boundary(max_chars);
        format!("{}...", &text[..boundary])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthesize_schema_valid() {
        let tracker = crate::delegation::new_tracker();
        let dir = std::env::temp_dir().join(format!("aivyx-synth-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let dirs = aivyx_config::AivyxDirs::new(&dir);
        let config = aivyx_config::AivyxConfig::default();
        let key = aivyx_crypto::MasterKey::generate();
        let session = Arc::new(AgentSession::new(dirs, config, key));

        let tool = SynthesizeResultsTool::new(session, tracker);
        assert_eq!(tool.name(), "synthesize_results");
        let schema = tool.input_schema();
        assert!(schema["properties"]["goal"].is_object());
        assert!(schema["properties"]["include_errors"].is_object());
        assert_eq!(schema["required"], serde_json::json!(["goal"]));
    }

    #[test]
    fn synthesize_required_scope() {
        let tracker = crate::delegation::new_tracker();
        let dir = std::env::temp_dir().join(format!("aivyx-synth-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let dirs = aivyx_config::AivyxDirs::new(&dir);
        let config = aivyx_config::AivyxConfig::default();
        let key = aivyx_crypto::MasterKey::generate();
        let session = Arc::new(AgentSession::new(dirs, config, key));

        let tool = SynthesizeResultsTool::new(session, tracker);
        assert_eq!(
            tool.required_scope(),
            Some(CapabilityScope::Custom("coordination".into()))
        );
    }

    #[tokio::test]
    async fn synthesize_empty_tracker() {
        let tracker = crate::delegation::new_tracker();
        let dir = std::env::temp_dir().join(format!("aivyx-synth-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let dirs = aivyx_config::AivyxDirs::new(&dir);
        let config = aivyx_config::AivyxConfig::default();
        let key = aivyx_crypto::MasterKey::generate();
        let session = Arc::new(AgentSession::new(dirs, config, key));

        let tool = SynthesizeResultsTool::new(session, tracker);
        let result = tool
            .execute(serde_json::json!({"goal": "test"}))
            .await
            .unwrap();
        assert_eq!(result["source_count"], 0);
        assert_eq!(result["error_count"], 0);
    }

    #[test]
    fn truncate_short_text() {
        assert_eq!(truncate_result("hello", 100), "hello");
    }

    #[test]
    fn truncate_long_text() {
        let long = "x".repeat(300);
        let truncated = truncate_result(&long, 200);
        assert!(truncated.ends_with("..."));
        assert!(truncated.len() <= 204); // 200 + "..."
    }
}

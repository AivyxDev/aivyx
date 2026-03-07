//! Structured quality-gate tool for team coordination.
//!
//! [`VerifyOutputTool`] routes specialist output to a reviewer agent for
//! structured evaluation. Returns a machine-parseable verdict (approved/rejected
//! with score and issues) rather than free-form prose, enabling the coordinator
//! to make automated redelegate decisions.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::info;

use aivyx_core::{AivyxError, CapabilityScope, Result, Tool, ToolId};

use crate::delegation::SpecialistPool;

/// System prompt instructing the reviewer to produce structured JSON output.
const REVIEW_SYSTEM_PROMPT: &str = r#"You are a quality reviewer for a multi-agent team. You will be given a specialist's output and criteria to evaluate it against.

Evaluate the result thoroughly and respond with ONLY a JSON object in this exact format:
{"approved": true/false, "score": 1-10, "issues": ["issue1", "issue2"], "summary": "one sentence summary"}

Rules:
- "approved": true if the output meets the criteria, false otherwise
- "score": 1 = completely wrong, 5 = acceptable, 10 = excellent
- "issues": list specific problems (empty array if none)
- "summary": one-sentence verdict

Do NOT include any text outside the JSON object."#;

/// Structured verdict from a quality review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewVerdict {
    /// Whether the output passed review.
    pub approved: bool,
    /// Quality score from 1 (worst) to 10 (best).
    pub score: u8,
    /// Specific issues identified by the reviewer.
    pub issues: Vec<String>,
    /// One-sentence summary of the verdict.
    pub summary: String,
}

/// Tool that routes specialist output to a reviewer for structured evaluation.
///
/// Registered on the coordinator agent. Uses a reviewer specialist (from the
/// `SpecialistPool`) to evaluate another specialist's output against given
/// criteria. Returns a structured `ReviewVerdict` rather than free-form text.
pub struct VerifyOutputTool {
    id: ToolId,
    pool: SpecialistPool,
    /// Optional token channel for streaming reviewer output.
    token_tx: Option<tokio::sync::mpsc::Sender<String>>,
}

impl VerifyOutputTool {
    /// Create a new verification tool.
    ///
    /// `pool` is the shared specialist pool — the reviewer agent is obtained
    /// from this pool, preserving its conversation context across reviews.
    pub fn new(pool: SpecialistPool) -> Self {
        Self {
            id: ToolId::new(),
            pool,
            token_tx: None,
        }
    }

    /// Set the token sender for streaming reviewer output.
    pub fn with_token_tx(mut self, tx: tokio::sync::mpsc::Sender<String>) -> Self {
        self.token_tx = Some(tx);
        self
    }
}

#[async_trait]
impl Tool for VerifyOutputTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "verify_output"
    }

    fn description(&self) -> &str {
        "Verify specialist output against criteria using a reviewer agent. \
         Returns a structured verdict with approved/rejected, score, and issues."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "result": {
                    "type": "string",
                    "description": "The specialist output to verify"
                },
                "criteria": {
                    "type": "string",
                    "description": "What to verify against (requirements, quality standards)"
                },
                "reviewer_agent": {
                    "type": "string",
                    "description": "Agent to use for review. Default: 'reviewer'"
                }
            },
            "required": ["result", "criteria"]
        })
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        Some(CapabilityScope::Custom("coordination".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let specialist_result = input["result"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("verify_output: missing 'result'".into()))?;
        let criteria = input["criteria"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("verify_output: missing 'criteria'".into()))?;
        let reviewer_name = input["reviewer_agent"].as_str().unwrap_or("reviewer");

        info!("Routing output to reviewer '{reviewer_name}' for verification");

        // Get or create the reviewer from the pool
        let reviewer_arc = self.pool.get_or_create(reviewer_name).await.map_err(|e| {
            AivyxError::Agent(format!("verify_output: failed to create reviewer: {e}"))
        })?;

        // Build the review prompt
        let prompt = format!(
            "{REVIEW_SYSTEM_PROMPT}\n\n\
             RESULT:\n{specialist_result}\n\n\
             CRITERIA:\n{criteria}"
        );

        // Run the reviewer
        let response = {
            let mut reviewer = reviewer_arc.lock().await;
            if let Some(ref tx) = self.token_tx {
                let _ = tx.send(format!("\n--- [{reviewer_name}] ---\n")).await;
                let r = reviewer.turn_stream(&prompt, None, tx.clone(), None).await;
                let _ = tx.send(format!("\n--- [/{reviewer_name}] ---\n")).await;
                r
            } else {
                reviewer.turn(&prompt, None).await
            }
        }
        .map_err(|e| AivyxError::Agent(format!("verify_output: reviewer failed: {e}")))?;

        // Parse the structured verdict
        match parse_review_response(&response) {
            Ok(verdict) => Ok(serde_json::json!({
                "approved": verdict.approved,
                "score": verdict.score,
                "issues": verdict.issues,
                "summary": verdict.summary,
                "reviewer": reviewer_name,
            })),
            Err(_) => {
                // Graceful fallback: return raw text with parse_error flag
                Ok(serde_json::json!({
                    "raw_response": response,
                    "parse_error": true,
                    "reviewer": reviewer_name,
                }))
            }
        }
    }
}

/// Parse the reviewer's response into a structured verdict.
///
/// Handles both raw JSON and markdown-fenced JSON. Validates that score
/// is within the 1-10 range.
fn parse_review_response(text: &str) -> std::result::Result<ReviewVerdict, String> {
    let trimmed = strip_code_fences(text.trim());
    let verdict: ReviewVerdict =
        serde_json::from_str(trimmed).map_err(|e| format!("invalid JSON: {e}"))?;

    if verdict.score > 10 {
        return Err(format!("score {} out of range (1-10)", verdict.score));
    }

    Ok(verdict)
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
    use std::sync::Arc;

    use super::*;

    #[test]
    fn verify_schema_valid() {
        let dir = std::env::temp_dir().join(format!("aivyx-verify-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let dirs = aivyx_config::AivyxDirs::new(&dir);
        let config = aivyx_config::AivyxConfig::default();
        let key = aivyx_crypto::MasterKey::generate();
        let session = Arc::new(aivyx_agent::AgentSession::new(dirs, config, key));
        let pool = SpecialistPool::new(
            session,
            None,
            aivyx_capability::CapabilitySet::new(),
            None,
            crate::config::DialogueConfig::default(),
        );

        let tool = VerifyOutputTool::new(pool);
        assert_eq!(tool.name(), "verify_output");
        let schema = tool.input_schema();
        assert!(schema["properties"]["result"].is_object());
        assert!(schema["properties"]["criteria"].is_object());
        assert!(schema["properties"]["reviewer_agent"].is_object());
        assert_eq!(
            schema["required"],
            serde_json::json!(["result", "criteria"])
        );
    }

    #[test]
    fn verify_required_scope() {
        let dir = std::env::temp_dir().join(format!("aivyx-verify-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let dirs = aivyx_config::AivyxDirs::new(&dir);
        let config = aivyx_config::AivyxConfig::default();
        let key = aivyx_crypto::MasterKey::generate();
        let session = Arc::new(aivyx_agent::AgentSession::new(dirs, config, key));
        let pool = SpecialistPool::new(
            session,
            None,
            aivyx_capability::CapabilitySet::new(),
            None,
            crate::config::DialogueConfig::default(),
        );

        let tool = VerifyOutputTool::new(pool);
        assert_eq!(
            tool.required_scope(),
            Some(CapabilityScope::Custom("coordination".into()))
        );
    }

    #[test]
    fn parse_reviewer_response_valid_json() {
        let json = r#"{"approved": true, "score": 8, "issues": [], "summary": "Looks good"}"#;
        let verdict = parse_review_response(json).unwrap();
        assert!(verdict.approved);
        assert_eq!(verdict.score, 8);
        assert!(verdict.issues.is_empty());
        assert_eq!(verdict.summary, "Looks good");
    }

    #[test]
    fn parse_reviewer_response_with_issues() {
        let json = r#"{"approved": false, "score": 3, "issues": ["Missing error handling", "No tests"], "summary": "Needs work"}"#;
        let verdict = parse_review_response(json).unwrap();
        assert!(!verdict.approved);
        assert_eq!(verdict.score, 3);
        assert_eq!(verdict.issues.len(), 2);
        assert_eq!(verdict.issues[0], "Missing error handling");
    }

    #[test]
    fn parse_reviewer_response_fenced_json() {
        let fenced =
            "```json\n{\"approved\": true, \"score\": 7, \"issues\": [], \"summary\": \"OK\"}\n```";
        let verdict = parse_review_response(fenced).unwrap();
        assert!(verdict.approved);
        assert_eq!(verdict.score, 7);
    }

    #[test]
    fn parse_reviewer_response_invalid_json() {
        let bad = "This is not JSON at all";
        assert!(parse_review_response(bad).is_err());
    }

    #[test]
    fn parse_reviewer_response_score_out_of_range() {
        let json = r#"{"approved": true, "score": 15, "issues": [], "summary": "OK"}"#;
        assert!(parse_review_response(json).is_err());
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

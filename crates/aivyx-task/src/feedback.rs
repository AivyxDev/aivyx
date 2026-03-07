//! Planner feedback from outcome history.
//!
//! Analyzes past [`OutcomeRecord`]s to identify successful/failure patterns,
//! tool rankings, and role rankings. The resulting [`PlannerFeedback`] can be
//! formatted as a prompt block for injection into the planner system prompt.

use std::collections::HashMap;

use aivyx_memory::OutcomeRecord;

/// A pattern identified from outcome history.
#[derive(Debug, Clone)]
pub struct PlanPattern {
    /// Human-readable description of the pattern.
    pub description: String,
    /// How many times this pattern appeared.
    pub frequency: usize,
}

/// Aggregated feedback from outcome history for the planner.
#[derive(Debug, Clone)]
pub struct PlannerFeedback {
    /// Patterns that led to success.
    pub successful_patterns: Vec<PlanPattern>,
    /// Patterns that led to failure.
    pub failure_patterns: Vec<PlanPattern>,
    /// Tool success rates: (tool_name, success_rate).
    pub tool_rankings: Vec<(String, f64)>,
    /// Role success rates: (role, success_rate).
    pub role_rankings: Vec<(String, f64)>,
}

/// Analyze a set of outcome records to produce planner feedback.
pub fn analyze_outcomes(outcomes: &[OutcomeRecord]) -> PlannerFeedback {
    if outcomes.is_empty() {
        return PlannerFeedback {
            successful_patterns: Vec::new(),
            failure_patterns: Vec::new(),
            tool_rankings: Vec::new(),
            role_rankings: Vec::new(),
        };
    }

    // 1. Compute success rate per tool
    let mut tool_stats: HashMap<String, (usize, usize)> = HashMap::new(); // (total, successes)
    for outcome in outcomes {
        for tool in &outcome.tools_used {
            let entry = tool_stats.entry(tool.clone()).or_insert((0, 0));
            entry.0 += 1;
            if outcome.success {
                entry.1 += 1;
            }
        }
    }

    let mut tool_rankings: Vec<(String, f64)> = tool_stats
        .iter()
        .map(|(name, (total, successes))| {
            let rate = *successes as f64 / *total as f64;
            (name.clone(), rate)
        })
        .collect();
    tool_rankings.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // 2. Compute success rate per role
    let mut role_stats: HashMap<String, (usize, usize)> = HashMap::new();
    for outcome in outcomes {
        if let Some(ref role) = outcome.agent_role {
            let entry = role_stats.entry(role.clone()).or_insert((0, 0));
            entry.0 += 1;
            if outcome.success {
                entry.1 += 1;
            }
        }
    }

    let mut role_rankings: Vec<(String, f64)> = role_stats
        .iter()
        .map(|(role, (total, successes))| {
            let rate = *successes as f64 / *total as f64;
            (role.clone(), rate)
        })
        .collect();
    role_rankings.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // 3. Identify tool combination patterns
    // Group outcomes by their sorted tool combination key
    let mut combo_stats: HashMap<String, (usize, usize)> = HashMap::new();
    for outcome in outcomes {
        if outcome.tools_used.is_empty() {
            continue;
        }
        let mut tools = outcome.tools_used.clone();
        tools.sort();
        let key = if tools.len() == 1 {
            format!("{} alone", tools[0])
        } else {
            tools.join(" + ")
        };
        let entry = combo_stats.entry(key).or_insert((0, 0));
        entry.0 += 1;
        if outcome.success {
            entry.1 += 1;
        }
    }

    let mut successful_patterns = Vec::new();
    let mut failure_patterns = Vec::new();

    for (combo, (total, successes)) in &combo_stats {
        if *total < 2 {
            continue;
        }
        let rate = *successes as f64 / *total as f64;
        if rate > 0.8 {
            successful_patterns.push(PlanPattern {
                description: combo.clone(),
                frequency: *total,
            });
        } else if rate < 0.3 {
            failure_patterns.push(PlanPattern {
                description: combo.clone(),
                frequency: *total,
            });
        }
    }

    // Sort patterns by frequency descending for deterministic output
    successful_patterns.sort_by(|a, b| b.frequency.cmp(&a.frequency));
    failure_patterns.sort_by(|a, b| b.frequency.cmp(&a.frequency));

    PlannerFeedback {
        successful_patterns,
        failure_patterns,
        tool_rankings,
        role_rankings,
    }
}

/// Format planner feedback as a prompt block for injection into the planner system prompt.
pub fn format_feedback_block(feedback: &PlannerFeedback) -> String {
    let mut lines = Vec::new();
    lines.push("[PLANNER FEEDBACK]".to_string());

    if !feedback.tool_rankings.is_empty() {
        lines.push("Tool success rates:".to_string());
        for (tool, rate) in &feedback.tool_rankings {
            let pct = (rate * 100.0).round() as u32;
            lines.push(format!("- {tool}: {pct}%"));
        }
    }

    if !feedback.successful_patterns.is_empty() {
        lines.push("Successful patterns:".to_string());
        for pattern in &feedback.successful_patterns {
            lines.push(format!(
                "- \"{}\" ({} occurrences)",
                pattern.description, pattern.frequency
            ));
        }
    }

    if !feedback.failure_patterns.is_empty() {
        lines.push("Failure patterns:".to_string());
        for pattern in &feedback.failure_patterns {
            lines.push(format!(
                "- \"{}\" ({} occurrences)",
                pattern.description, pattern.frequency
            ));
        }
    }

    lines.push("[END PLANNER FEEDBACK]".to_string());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivyx_core::TaskId;
    use aivyx_memory::{OutcomeRecord, OutcomeSource};

    fn make_outcome(
        success: bool,
        tools: Vec<&str>,
        role: Option<&str>,
        duration_ms: u64,
    ) -> OutcomeRecord {
        let mut record = OutcomeRecord::new(
            OutcomeSource::MissionStep {
                task_id: TaskId::new(),
                step_index: 0,
            },
            success,
            "test".into(),
            duration_ms,
            "agent".into(),
            "goal".into(),
        )
        .with_tools(tools.into_iter().map(String::from).collect());

        if let Some(r) = role {
            record = record.with_role(r);
        }
        record
    }

    #[test]
    fn analyze_empty_outcomes() {
        let feedback = analyze_outcomes(&[]);
        assert!(feedback.successful_patterns.is_empty());
        assert!(feedback.failure_patterns.is_empty());
        assert!(feedback.tool_rankings.is_empty());
        assert!(feedback.role_rankings.is_empty());
    }

    #[test]
    fn analyze_mixed_outcomes_tool_rankings() {
        let outcomes = vec![
            make_outcome(true, vec!["shell"], None, 100),
            make_outcome(true, vec!["shell"], None, 200),
            make_outcome(false, vec!["shell"], None, 300),
            make_outcome(true, vec!["web_search"], None, 100),
            make_outcome(false, vec!["web_search"], None, 200),
        ];

        let feedback = analyze_outcomes(&outcomes);

        // shell: 2/3 = 0.667, web_search: 1/2 = 0.5
        assert_eq!(feedback.tool_rankings.len(), 2);
        assert_eq!(feedback.tool_rankings[0].0, "shell");
        assert!((feedback.tool_rankings[0].1 - 2.0 / 3.0).abs() < 0.01);
        assert_eq!(feedback.tool_rankings[1].0, "web_search");
        assert!((feedback.tool_rankings[1].1 - 0.5).abs() < 0.01);
    }

    #[test]
    fn analyze_role_rankings() {
        let outcomes = vec![
            make_outcome(true, vec!["shell"], Some("coder"), 100),
            make_outcome(true, vec!["shell"], Some("coder"), 200),
            make_outcome(false, vec!["web_search"], Some("researcher"), 100),
            make_outcome(false, vec!["web_search"], Some("researcher"), 200),
        ];

        let feedback = analyze_outcomes(&outcomes);

        assert_eq!(feedback.role_rankings.len(), 2);
        // coder: 2/2 = 1.0, researcher: 0/2 = 0.0
        let coder = feedback
            .role_rankings
            .iter()
            .find(|(r, _)| r == "coder")
            .unwrap();
        assert!((coder.1 - 1.0).abs() < 0.01);
        let researcher = feedback
            .role_rankings
            .iter()
            .find(|(r, _)| r == "researcher")
            .unwrap();
        assert!((researcher.1 - 0.0).abs() < 0.01);
    }

    #[test]
    fn format_feedback_block_output() {
        let feedback = PlannerFeedback {
            successful_patterns: vec![PlanPattern {
                description: "shell + file_write".into(),
                frequency: 5,
            }],
            failure_patterns: vec![PlanPattern {
                description: "web_search alone".into(),
                frequency: 3,
            }],
            tool_rankings: vec![("shell".into(), 0.92), ("web_search".into(), 0.50)],
            role_rankings: vec![],
        };

        let block = format_feedback_block(&feedback);
        assert!(block.starts_with("[PLANNER FEEDBACK]"));
        assert!(block.ends_with("[END PLANNER FEEDBACK]"));
        assert!(block.contains("shell: 92%"));
        assert!(block.contains("web_search: 50%"));
        assert!(block.contains("\"shell + file_write\" (5 occurrences)"));
        assert!(block.contains("\"web_search alone\" (3 occurrences)"));
    }
}

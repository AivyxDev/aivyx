//! Skill effectiveness scoring from outcome history.
//!
//! Computes per-tool effectiveness scores (success rate, average duration,
//! activation count) from [`OutcomeRecord`]s. Used by the `/skills/effectiveness`
//! endpoint and the planner feedback loop.

use std::collections::HashMap;

use aivyx_memory::OutcomeRecord;
use chrono::{DateTime, Utc};

/// Effectiveness score for a skill/tool.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SkillScore {
    /// Tool/skill name.
    pub skill_name: String,
    /// Total number of activations.
    pub activations: usize,
    /// Fraction of activations that succeeded (0.0..=1.0).
    pub success_rate: f64,
    /// Average execution duration in milliseconds.
    pub avg_duration_ms: f64,
    /// Most recent usage timestamp.
    pub last_used: DateTime<Utc>,
}

/// Compute skill effectiveness scores from outcome records.
///
/// Groups outcomes by each tool in `tools_used`, computes per-tool statistics,
/// and returns results sorted by `success_rate` descending.
pub fn score_skills(outcomes: &[OutcomeRecord]) -> Vec<SkillScore> {
    if outcomes.is_empty() {
        return Vec::new();
    }

    // Per-tool accumulators: (total, successes, duration_sum, max_created_at)
    let mut stats: HashMap<String, (usize, usize, u64, DateTime<Utc>)> = HashMap::new();

    for outcome in outcomes {
        for tool in &outcome.tools_used {
            let entry = stats
                .entry(tool.clone())
                .or_insert((0, 0, 0, DateTime::<Utc>::MIN_UTC));
            entry.0 += 1;
            if outcome.success {
                entry.1 += 1;
            }
            entry.2 += outcome.duration_ms;
            if outcome.created_at > entry.3 {
                entry.3 = outcome.created_at;
            }
        }
    }

    let mut scores: Vec<SkillScore> = stats
        .into_iter()
        .map(
            |(name, (total, successes, duration_sum, last))| SkillScore {
                skill_name: name,
                activations: total,
                success_rate: successes as f64 / total as f64,
                avg_duration_ms: duration_sum as f64 / total as f64,
                last_used: last,
            },
        )
        .collect();

    // Sort by success_rate descending, then by name for stability
    scores.sort_by(|a, b| {
        b.success_rate
            .partial_cmp(&a.success_rate)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.skill_name.cmp(&b.skill_name))
    });

    scores
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivyx_core::TaskId;
    use aivyx_memory::{OutcomeRecord, OutcomeSource};

    fn make_outcome(success: bool, tools: Vec<&str>, duration_ms: u64) -> OutcomeRecord {
        OutcomeRecord::new(
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
        .with_tools(tools.into_iter().map(String::from).collect())
    }

    #[test]
    fn score_skills_empty() {
        let scores = score_skills(&[]);
        assert!(scores.is_empty());
    }

    #[test]
    fn score_skills_computes_rates_and_durations() {
        let outcomes = vec![
            make_outcome(true, vec!["shell"], 100),
            make_outcome(true, vec!["shell"], 200),
            make_outcome(false, vec!["shell"], 300),
            make_outcome(true, vec!["web_search"], 400),
            make_outcome(false, vec!["web_search"], 600),
        ];

        let scores = score_skills(&outcomes);

        // shell: 2/3 success, avg 200ms
        let shell = scores.iter().find(|s| s.skill_name == "shell").unwrap();
        assert_eq!(shell.activations, 3);
        assert!((shell.success_rate - 2.0 / 3.0).abs() < 0.01);
        assert!((shell.avg_duration_ms - 200.0).abs() < 0.01);

        // web_search: 1/2 success, avg 500ms
        let ws = scores
            .iter()
            .find(|s| s.skill_name == "web_search")
            .unwrap();
        assert_eq!(ws.activations, 2);
        assert!((ws.success_rate - 0.5).abs() < 0.01);
        assert!((ws.avg_duration_ms - 500.0).abs() < 0.01);
    }

    #[test]
    fn score_skills_sorted_by_success_rate() {
        let outcomes = vec![
            make_outcome(true, vec!["file_write"], 100),
            make_outcome(true, vec!["file_write"], 100),
            make_outcome(true, vec!["shell"], 100),
            make_outcome(false, vec!["shell"], 100),
            make_outcome(false, vec!["web_search"], 100),
            make_outcome(false, vec!["web_search"], 100),
        ];

        let scores = score_skills(&outcomes);

        // file_write: 100%, shell: 50%, web_search: 0%
        assert_eq!(scores[0].skill_name, "file_write");
        assert!((scores[0].success_rate - 1.0).abs() < 0.01);
        assert_eq!(scores[1].skill_name, "shell");
        assert!((scores[1].success_rate - 0.5).abs() < 0.01);
        assert_eq!(scores[2].skill_name, "web_search");
        assert!((scores[2].success_rate - 0.0).abs() < 0.01);
    }
}

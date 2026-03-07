//! Metrics endpoints for observability.
//!
//! `GET /metrics` — Prometheus exposition format (public, no auth).
//! `GET /metrics/summary` — aggregated metrics over the last 7 days.
//! `GET /metrics/timeline` — hourly buckets over the last 7 days.
//! `GET /metrics/costs` — per-agent, per-provider cost breakdown.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};

use aivyx_audit::{AuditEvent, compute_summary, compute_timeline};
use aivyx_config::ModelPricing;

use crate::app_state::AppState;
use crate::error::ServerError;
use crate::extractors::AuthContextExt;
use aivyx_tenant::AivyxRole;

/// Query parameters for metrics endpoints.
#[derive(Debug, Deserialize)]
pub struct MetricsQuery {
    /// Number of days to include (default: 7, max: 30).
    #[serde(default = "default_days")]
    pub days: u64,
}

fn default_days() -> u64 {
    7
}

/// `GET /metrics` — Prometheus exposition format.
///
/// Public endpoint (no auth) for Prometheus scraping.
pub async fn prometheus_metrics(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match &state.prometheus_handle {
        Some(handle) => (
            [(
                axum::http::header::CONTENT_TYPE,
                "text/plain; version=0.0.4",
            )],
            handle.render(),
        )
            .into_response(),
        None => (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Prometheus metrics not initialized",
        )
            .into_response(),
    }
}

/// `GET /metrics/summary` — aggregated metrics over the specified period.
pub async fn metrics_summary(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Query(query): Query<MetricsQuery>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Billing)?;
    let days = query.days.clamp(1, 30);
    let to = Utc::now();
    let from = to - Duration::days(days as i64);

    let entries = state.audit_log.read_all_entries()?;

    let cost_fn = |input_tokens: u32, output_tokens: u32, provider: &str| -> f64 {
        let pricing = ModelPricing::default_for_model(provider);
        (input_tokens as f64 * pricing.input_cost_per_token)
            + (output_tokens as f64 * pricing.output_cost_per_token)
    };

    let summary = compute_summary(&entries, from, to, &cost_fn);
    Ok(axum::Json(summary))
}

/// `GET /metrics/timeline` — hourly buckets over the specified period.
pub async fn metrics_timeline(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Query(query): Query<MetricsQuery>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Billing)?;
    let days = query.days.clamp(1, 30);
    let to = Utc::now();
    let from = to - Duration::days(days as i64);

    let entries = state.audit_log.read_all_entries()?;

    let cost_fn = |input_tokens: u32, output_tokens: u32, provider: &str| -> f64 {
        let pricing = ModelPricing::default_for_model(provider);
        (input_tokens as f64 * pricing.input_cost_per_token)
            + (output_tokens as f64 * pricing.output_cost_per_token)
    };

    let timeline = compute_timeline(&entries, from, to, &cost_fn);
    Ok(axum::Json(timeline))
}

/// Per-agent cost breakdown entry.
#[derive(Debug, Clone, Serialize)]
struct AgentCost {
    agent_id: String,
    provider: String,
    input_tokens: u64,
    output_tokens: u64,
    requests: u64,
    cost_usd: f64,
}

/// Response for `GET /metrics/costs`.
#[derive(Debug, Clone, Serialize)]
struct CostRollup {
    from: chrono::DateTime<Utc>,
    to: chrono::DateTime<Utc>,
    total_cost_usd: f64,
    breakdown: Vec<AgentCost>,
}

/// `GET /metrics/costs` — per-agent, per-provider cost breakdown.
///
/// Returns JSON with cost data grouped by (agent_id, provider) over the
/// specified number of days.
pub async fn cost_rollup(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Query(query): Query<MetricsQuery>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Billing)?;
    let days = query.days.clamp(1, 30);
    let to = Utc::now();
    let from = to - Duration::days(days as i64);

    let entries = state.audit_log.read_all_entries()?;

    // Group by (agent_id, provider)
    let mut groups: HashMap<(String, String), AgentCost> = HashMap::new();
    let mut total_cost = 0.0;

    for entry in &entries {
        let ts = chrono::DateTime::parse_from_rfc3339(&entry.timestamp)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or(from);
        if ts < from || ts > to {
            continue;
        }

        if let AuditEvent::LlmResponseReceived {
            agent_id,
            provider,
            input_tokens,
            output_tokens,
            ..
        } = &entry.event
        {
            let pricing = ModelPricing::default_for_model(provider);
            let cost = (*input_tokens as f64 * pricing.input_cost_per_token)
                + (*output_tokens as f64 * pricing.output_cost_per_token);
            total_cost += cost;

            let key = (agent_id.to_string(), provider.clone());
            let entry = groups.entry(key).or_insert_with(|| AgentCost {
                agent_id: agent_id.to_string(),
                provider: provider.clone(),
                input_tokens: 0,
                output_tokens: 0,
                requests: 0,
                cost_usd: 0.0,
            });
            entry.input_tokens += *input_tokens as u64;
            entry.output_tokens += *output_tokens as u64;
            entry.requests += 1;
            entry.cost_usd += cost;
        }
    }

    let mut breakdown: Vec<AgentCost> = groups.into_values().collect();
    breakdown.sort_by(|a, b| {
        b.cost_usd
            .partial_cmp(&a.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(axum::Json(CostRollup {
        from,
        to,
        total_cost_usd: total_cost,
        breakdown,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_query_default() {
        let q: MetricsQuery = serde_json::from_str("{}").unwrap();
        assert_eq!(q.days, 7);
    }

    #[test]
    fn metrics_query_custom() {
        let q: MetricsQuery = serde_json::from_str(r#"{"days":14}"#).unwrap();
        assert_eq!(q.days, 14);
    }
}

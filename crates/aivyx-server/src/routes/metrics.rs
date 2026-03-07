//! Metrics endpoints for observability.
//!
//! `GET /metrics/summary` — aggregated metrics over the last 7 days.
//! `GET /metrics/timeline` — hourly buckets over the last 7 days.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use chrono::{Duration, Utc};
use serde::Deserialize;

use aivyx_audit::{compute_summary, compute_timeline};
use aivyx_config::ModelPricing;

use crate::app_state::AppState;
use crate::error::ServerError;

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

/// `GET /metrics/summary` — aggregated metrics over the specified period.
pub async fn metrics_summary(
    State(state): State<Arc<AppState>>,
    Query(query): Query<MetricsQuery>,
) -> Result<impl IntoResponse, ServerError> {
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
    Query(query): Query<MetricsQuery>,
) -> Result<impl IntoResponse, ServerError> {
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

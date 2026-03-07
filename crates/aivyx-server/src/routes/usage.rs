//! Usage and billing endpoints.
//!
//! `GET /usage` -- usage summary (today + this month).
//! `GET /usage/daily` -- daily time series over the last N days.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use chrono::{Datelike, Duration, Utc};
use serde::{Deserialize, Serialize};

use aivyx_billing::CostFilter;
use aivyx_tenant::AivyxRole;

use crate::app_state::AppState;
use crate::error::ServerError;
use crate::extractors::AuthContextExt;

/// Response for `GET /usage`.
#[derive(Debug, Serialize)]
struct UsageSummary {
    today_usd: f64,
    month_usd: f64,
}

/// Query parameters for `GET /usage/daily`.
#[derive(Debug, Deserialize)]
pub struct DailyQuery {
    /// Number of days to include (default: 30, max: 90).
    #[serde(default = "default_days")]
    pub days: u64,
}

fn default_days() -> u64 {
    30
}

/// A single day in the daily time series.
#[derive(Debug, Serialize)]
struct DailyEntry {
    date: String,
    cost_usd: f64,
}

/// `GET /usage` -- usage summary for today and this month.
pub async fn usage_summary(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Billing)?;

    let key = auth.effective_master_key(&state.master_key);
    let now = Utc::now();
    let today = now.date_naive();

    let today_usd = state
        .cost_ledger
        .daily_total(auth.tenant_id(), today, &key)?;
    let month_usd = state
        .cost_ledger
        .monthly_total(auth.tenant_id(), now.year(), now.month(), &key)?;

    Ok(axum::Json(UsageSummary {
        today_usd,
        month_usd,
    }))
}

/// `GET /usage/daily` -- daily cost time series.
pub async fn usage_daily(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Query(query): Query<DailyQuery>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Billing)?;

    let key = auth.effective_master_key(&state.master_key);
    let days = query.days.clamp(1, 90);
    let now = Utc::now();
    let today = now.date_naive();
    let from_date = today - Duration::days(days as i64 - 1);

    let filter = CostFilter {
        tenant_id: auth.tenant_id().copied(),
        from_date: Some(from_date),
        to_date: Some(today),
        ..Default::default()
    };
    let entries = state.cost_ledger.query(&filter, &key)?;

    // Bucket by date
    let mut buckets = std::collections::BTreeMap::new();
    let mut d = from_date;
    while d <= today {
        buckets.insert(d, 0.0_f64);
        d = d.succ_opt().unwrap_or(d);
        if d == from_date {
            break; // safety guard
        }
    }
    for entry in &entries {
        if let Some(total) = buckets.get_mut(&entry.date) {
            *total += entry.cost_usd;
        }
    }

    let series: Vec<DailyEntry> = buckets
        .into_iter()
        .map(|(date, cost_usd)| DailyEntry {
            date: date.to_string(),
            cost_usd,
        })
        .collect();

    Ok(axum::Json(series))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daily_query_default() {
        let q: DailyQuery = serde_json::from_str("{}").unwrap();
        assert_eq!(q.days, 30);
    }

    #[test]
    fn daily_query_custom() {
        let q: DailyQuery = serde_json::from_str(r#"{"days":14}"#).unwrap();
        assert_eq!(q.days, 14);
    }
}

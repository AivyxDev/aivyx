//! Cost ledger backed by `EncryptedStore`.
//!
//! Each entry is stored as `"cost:{date}:{uuid}"` -> serialized [`LedgerEntry`].

use std::collections::HashMap;
use std::path::Path;

use aivyx_core::{AgentId, AivyxError, Result, TenantId};
use aivyx_crypto::{EncryptedStore, MasterKey};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Key prefix for cost entries in `EncryptedStore`.
const COST_PREFIX: &str = "cost:";

/// A single cost record for an LLM invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerEntry {
    /// Tenant that owns this cost (if multi-tenant).
    pub tenant_id: Option<TenantId>,
    /// Agent that incurred the cost.
    pub agent_id: AgentId,
    /// Team name (if the agent belongs to a team).
    pub team_name: Option<String>,
    /// Model name used for the invocation.
    pub model: String,
    /// Provider name (e.g. "openai", "anthropic").
    pub provider: String,
    /// Number of input tokens consumed.
    pub input_tokens: u64,
    /// Number of output tokens produced.
    pub output_tokens: u64,
    /// Cost in USD for this invocation.
    pub cost_usd: f64,
    /// Arbitrary tags for cost attribution.
    pub tags: HashMap<String, String>,
    /// When the invocation occurred.
    pub timestamp: DateTime<Utc>,
    /// Date for daily bucketing.
    pub date: NaiveDate,
}

/// Filter criteria for querying ledger entries.
#[derive(Debug, Clone, Default)]
pub struct CostFilter {
    /// Filter by tenant.
    pub tenant_id: Option<TenantId>,
    /// Filter by agent.
    pub agent_id: Option<AgentId>,
    /// Filter by team name.
    pub team_name: Option<String>,
    /// Filter by model name.
    pub model: Option<String>,
    /// Include entries on or after this date.
    pub from_date: Option<NaiveDate>,
    /// Include entries on or before this date.
    pub to_date: Option<NaiveDate>,
}

/// Persistent cost ledger backed by `EncryptedStore`.
pub struct CostLedger {
    store: EncryptedStore,
}

impl CostLedger {
    /// Open (or create) a cost ledger at the given path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            store: EncryptedStore::open(path)?,
        })
    }

    /// Record a cost entry in the ledger.
    pub fn record(&self, entry: &LedgerEntry, key: &MasterKey) -> Result<()> {
        let store_key = format!("{}{}:{}", COST_PREFIX, entry.date, Uuid::new_v4());
        let json = serde_json::to_vec(entry)
            .map_err(|e| AivyxError::Storage(format!("failed to serialize ledger entry: {e}")))?;
        self.store.put(&store_key, &json, key)?;
        Ok(())
    }

    /// Query ledger entries matching the given filter.
    pub fn query(&self, filter: &CostFilter, key: &MasterKey) -> Result<Vec<LedgerEntry>> {
        let all_keys = self.store.list_keys()?;
        let mut entries = Vec::new();

        for k in all_keys {
            if !k.starts_with(COST_PREFIX) {
                continue;
            }

            // Extract the date portion from the key "cost:{date}:{uuid}"
            let rest = &k[COST_PREFIX.len()..];
            if let Some(date_str) = rest.split(':').next() {
                // Pre-filter by date range using the key before decrypting
                if let Ok(entry_date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
                    if let Some(from) = filter.from_date {
                        if entry_date < from {
                            continue;
                        }
                    }
                    if let Some(to) = filter.to_date {
                        if entry_date > to {
                            continue;
                        }
                    }
                }
            }

            if let Some(bytes) = self.store.get(&k, key)? {
                if let Ok(entry) = serde_json::from_slice::<LedgerEntry>(&bytes) {
                    if matches_filter(&entry, filter) {
                        entries.push(entry);
                    }
                }
            }
        }

        Ok(entries)
    }

    /// Compute the total USD cost for a given date, optionally filtered by tenant.
    pub fn daily_total(
        &self,
        tenant_id: Option<&TenantId>,
        date: NaiveDate,
        key: &MasterKey,
    ) -> Result<f64> {
        let filter = CostFilter {
            tenant_id: tenant_id.copied(),
            from_date: Some(date),
            to_date: Some(date),
            ..Default::default()
        };
        let entries = self.query(&filter, key)?;
        Ok(entries.iter().map(|e| e.cost_usd).sum())
    }

    /// Compute the total USD cost for a given month, optionally filtered by tenant.
    pub fn monthly_total(
        &self,
        tenant_id: Option<&TenantId>,
        year: i32,
        month: u32,
        key: &MasterKey,
    ) -> Result<f64> {
        let from = NaiveDate::from_ymd_opt(year, month, 1)
            .ok_or_else(|| AivyxError::Config(format!("invalid date: {year}-{month}")))?;
        let to = if month == 12 {
            NaiveDate::from_ymd_opt(year + 1, 1, 1)
        } else {
            NaiveDate::from_ymd_opt(year, month + 1, 1)
        }
        .ok_or_else(|| AivyxError::Config(format!("invalid date range for {year}-{month}")))?
        .pred_opt()
        .ok_or_else(|| AivyxError::Config("date underflow".into()))?;

        let filter = CostFilter {
            tenant_id: tenant_id.copied(),
            from_date: Some(from),
            to_date: Some(to),
            ..Default::default()
        };
        let entries = self.query(&filter, key)?;
        Ok(entries.iter().map(|e| e.cost_usd).sum())
    }
}

/// Check whether a ledger entry matches the given filter criteria.
fn matches_filter(entry: &LedgerEntry, filter: &CostFilter) -> bool {
    if let Some(ref tid) = filter.tenant_id {
        if entry.tenant_id.as_ref() != Some(tid) {
            return false;
        }
    }
    if let Some(ref aid) = filter.agent_id {
        if &entry.agent_id != aid {
            return false;
        }
    }
    if let Some(ref team) = filter.team_name {
        if entry.team_name.as_ref() != Some(team) {
            return false;
        }
    }
    if let Some(ref model) = filter.model {
        if &entry.model != model {
            return false;
        }
    }
    if let Some(from) = filter.from_date {
        if entry.date < from {
            return false;
        }
    }
    if let Some(to) = filter.to_date {
        if entry.date > to {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_ledger() -> (CostLedger, MasterKey) {
        let dir = tempfile::tempdir().unwrap();
        let ledger = CostLedger::open(dir.path().join("costs.db")).unwrap();
        let key = MasterKey::from_bytes([42u8; 32]);
        (ledger, key)
    }

    fn sample_entry(date: NaiveDate, cost: f64, agent_id: AgentId) -> LedgerEntry {
        LedgerEntry {
            tenant_id: None,
            agent_id,
            team_name: None,
            model: "gpt-4".to_string(),
            provider: "openai".to_string(),
            input_tokens: 1000,
            output_tokens: 500,
            cost_usd: cost,
            tags: HashMap::new(),
            timestamp: date.and_hms_opt(12, 0, 0).unwrap().and_utc(),
            date,
        }
    }

    #[test]
    fn record_and_query_entry() {
        let (ledger, key) = temp_ledger();
        let date = NaiveDate::from_ymd_opt(2026, 3, 7).unwrap();
        let agent = AgentId::new();
        let entry = sample_entry(date, 0.05, agent);

        ledger.record(&entry, &key).unwrap();

        let results = ledger.query(&CostFilter::default(), &key).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].cost_usd, 0.05);
        assert_eq!(results[0].model, "gpt-4");
        assert_eq!(results[0].agent_id, agent);
    }

    #[test]
    fn query_filters_by_agent() {
        let (ledger, key) = temp_ledger();
        let date = NaiveDate::from_ymd_opt(2026, 3, 7).unwrap();
        let agent_a = AgentId::new();
        let agent_b = AgentId::new();

        ledger.record(&sample_entry(date, 0.10, agent_a), &key).unwrap();
        ledger.record(&sample_entry(date, 0.20, agent_b), &key).unwrap();

        let filter = CostFilter {
            agent_id: Some(agent_a),
            ..Default::default()
        };
        let results = ledger.query(&filter, &key).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].agent_id, agent_a);
    }

    #[test]
    fn query_filters_by_date_range() {
        let (ledger, key) = temp_ledger();
        let agent = AgentId::new();

        for day in 1..=5 {
            let date = NaiveDate::from_ymd_opt(2026, 3, day).unwrap();
            ledger.record(&sample_entry(date, 0.10, agent), &key).unwrap();
        }

        let filter = CostFilter {
            from_date: Some(NaiveDate::from_ymd_opt(2026, 3, 2).unwrap()),
            to_date: Some(NaiveDate::from_ymd_opt(2026, 3, 4).unwrap()),
            ..Default::default()
        };
        let results = ledger.query(&filter, &key).unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn query_filters_by_model() {
        let (ledger, key) = temp_ledger();
        let date = NaiveDate::from_ymd_opt(2026, 3, 7).unwrap();
        let agent = AgentId::new();

        let mut entry_gpt = sample_entry(date, 0.10, agent);
        entry_gpt.model = "gpt-4".to_string();
        ledger.record(&entry_gpt, &key).unwrap();

        let mut entry_claude = sample_entry(date, 0.15, agent);
        entry_claude.model = "claude-3".to_string();
        ledger.record(&entry_claude, &key).unwrap();

        let filter = CostFilter {
            model: Some("claude-3".to_string()),
            ..Default::default()
        };
        let results = ledger.query(&filter, &key).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].model, "claude-3");
    }

    #[test]
    fn query_filters_by_tenant() {
        let (ledger, key) = temp_ledger();
        let date = NaiveDate::from_ymd_opt(2026, 3, 7).unwrap();
        let agent = AgentId::new();
        let tenant = TenantId::new();

        let mut entry_with_tenant = sample_entry(date, 0.10, agent);
        entry_with_tenant.tenant_id = Some(tenant);
        ledger.record(&entry_with_tenant, &key).unwrap();

        let entry_no_tenant = sample_entry(date, 0.20, agent);
        ledger.record(&entry_no_tenant, &key).unwrap();

        let filter = CostFilter {
            tenant_id: Some(tenant),
            ..Default::default()
        };
        let results = ledger.query(&filter, &key).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tenant_id, Some(tenant));
    }

    #[test]
    fn daily_total_sums_costs() {
        let (ledger, key) = temp_ledger();
        let date = NaiveDate::from_ymd_opt(2026, 3, 7).unwrap();
        let agent = AgentId::new();

        ledger.record(&sample_entry(date, 0.10, agent), &key).unwrap();
        ledger.record(&sample_entry(date, 0.25, agent), &key).unwrap();

        let total = ledger.daily_total(None, date, &key).unwrap();
        assert!((total - 0.35).abs() < 1e-10);
    }

    #[test]
    fn daily_total_excludes_other_dates() {
        let (ledger, key) = temp_ledger();
        let agent = AgentId::new();
        let day1 = NaiveDate::from_ymd_opt(2026, 3, 7).unwrap();
        let day2 = NaiveDate::from_ymd_opt(2026, 3, 8).unwrap();

        ledger.record(&sample_entry(day1, 0.10, agent), &key).unwrap();
        ledger.record(&sample_entry(day2, 0.50, agent), &key).unwrap();

        let total = ledger.daily_total(None, day1, &key).unwrap();
        assert!((total - 0.10).abs() < 1e-10);
    }

    #[test]
    fn monthly_total_sums_across_days() {
        let (ledger, key) = temp_ledger();
        let agent = AgentId::new();

        for day in [1, 10, 20, 28] {
            let date = NaiveDate::from_ymd_opt(2026, 3, day).unwrap();
            ledger.record(&sample_entry(date, 1.00, agent), &key).unwrap();
        }

        // Add an entry in a different month
        let other = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
        ledger.record(&sample_entry(other, 99.0, agent), &key).unwrap();

        let total = ledger.monthly_total(None, 2026, 3, &key).unwrap();
        assert!((total - 4.00).abs() < 1e-10);
    }

    #[test]
    fn monthly_total_december_boundary() {
        let (ledger, key) = temp_ledger();
        let agent = AgentId::new();

        let dec = NaiveDate::from_ymd_opt(2026, 12, 15).unwrap();
        ledger.record(&sample_entry(dec, 2.50, agent), &key).unwrap();

        let jan = NaiveDate::from_ymd_opt(2027, 1, 5).unwrap();
        ledger.record(&sample_entry(jan, 10.0, agent), &key).unwrap();

        let total = ledger.monthly_total(None, 2026, 12, &key).unwrap();
        assert!((total - 2.50).abs() < 1e-10);
    }

    #[test]
    fn ledger_entry_serde_roundtrip() {
        let entry = LedgerEntry {
            tenant_id: Some(TenantId::new()),
            agent_id: AgentId::new(),
            team_name: Some("alpha-team".into()),
            model: "gpt-4".into(),
            provider: "openai".into(),
            input_tokens: 1000,
            output_tokens: 500,
            cost_usd: 0.05,
            tags: HashMap::from([("task".into(), "summarize".into())]),
            timestamp: Utc::now(),
            date: NaiveDate::from_ymd_opt(2026, 3, 7).unwrap(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: LedgerEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.model, "gpt-4");
        assert_eq!(parsed.cost_usd, 0.05);
        assert_eq!(parsed.tags.get("task").unwrap(), "summarize");
    }

    #[test]
    fn empty_ledger_returns_zero_totals() {
        let (ledger, key) = temp_ledger();
        let date = NaiveDate::from_ymd_opt(2026, 3, 7).unwrap();

        let daily = ledger.daily_total(None, date, &key).unwrap();
        assert!((daily - 0.0).abs() < 1e-10);

        let monthly = ledger.monthly_total(None, 2026, 3, &key).unwrap();
        assert!((monthly - 0.0).abs() < 1e-10);
    }
}

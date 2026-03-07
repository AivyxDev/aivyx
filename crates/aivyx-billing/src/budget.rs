//! Budget enforcement for agent and tenant spending limits.
//!
//! [`BudgetEnforcer`] checks current spending against configured limits and
//! returns `Err(AivyxError::RateLimit(...))` when a budget with [`BudgetAction::Pause`]
//! is exceeded.

use std::sync::Arc;

use aivyx_core::{AivyxError, Result, TenantId};
use aivyx_crypto::MasterKey;
use chrono::{Datelike, Utc};
use serde::{Deserialize, Serialize};

use crate::ledger::CostLedger;

/// What happens when a budget limit is exceeded.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum BudgetAction {
    /// Pause all LLM requests until the budget period resets.
    Pause,
    /// Log an alert but allow requests to continue.
    Alert,
}

/// Budget configuration for spending limits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetConfig {
    /// Maximum USD spend per agent per day.
    pub agent_daily_usd: Option<f64>,
    /// Maximum USD spend per agent per month.
    pub agent_monthly_usd: Option<f64>,
    /// Maximum USD spend per tenant per day.
    pub tenant_daily_usd: Option<f64>,
    /// Maximum USD spend per tenant per month.
    pub tenant_monthly_usd: Option<f64>,
    /// Action to take when budget is exceeded.
    pub on_exceeded: BudgetAction,
    /// Alert threshold as a fraction (e.g. 0.8 = alert at 80% of budget).
    pub alert_threshold: Option<f64>,
    /// Webhook URL to call when alert threshold is reached.
    pub alert_webhook: Option<String>,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            agent_daily_usd: None,
            agent_monthly_usd: None,
            tenant_daily_usd: Some(100.0),
            tenant_monthly_usd: Some(2000.0),
            on_exceeded: BudgetAction::Pause,
            alert_threshold: Some(0.8),
            alert_webhook: None,
        }
    }
}

/// Enforces budget limits by checking the cost ledger against configured thresholds.
pub struct BudgetEnforcer {
    ledger: Arc<CostLedger>,
    config: BudgetConfig,
}

impl BudgetEnforcer {
    /// Create a new budget enforcer with the given ledger and configuration.
    pub fn new(ledger: Arc<CostLedger>, config: BudgetConfig) -> Self {
        Self { ledger, config }
    }

    /// Check whether the tenant (or global) budget has been exceeded.
    ///
    /// Returns `Ok(())` if spending is within limits, or
    /// `Err(AivyxError::RateLimit(...))` if a [`BudgetAction::Pause`] limit is exceeded.
    pub fn check_budget(&self, tenant_id: Option<&TenantId>, key: &MasterKey) -> Result<()> {
        let now = Utc::now();
        let today = now.date_naive();
        let year = now.year();
        let month = now.month();

        // Check tenant daily limit
        if let Some(daily_limit) = self.config.tenant_daily_usd {
            let daily_total = self.ledger.daily_total(tenant_id, today, key)?;
            if daily_total >= daily_limit && self.config.on_exceeded == BudgetAction::Pause {
                return Err(AivyxError::RateLimit(format!(
                    "daily budget exceeded: ${:.2} of ${:.2} limit",
                    daily_total, daily_limit
                )));
            }
        }

        // Check tenant monthly limit
        if let Some(monthly_limit) = self.config.tenant_monthly_usd {
            let monthly_total = self.ledger.monthly_total(tenant_id, year, month, key)?;
            if monthly_total >= monthly_limit && self.config.on_exceeded == BudgetAction::Pause {
                return Err(AivyxError::RateLimit(format!(
                    "monthly budget exceeded: ${:.2} of ${:.2} limit",
                    monthly_total, monthly_limit
                )));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::LedgerEntry;
    use aivyx_core::AgentId;
    use chrono::NaiveDate;
    use std::collections::HashMap;

    fn temp_enforcer(config: BudgetConfig) -> (BudgetEnforcer, MasterKey) {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(CostLedger::open(dir.path().join("costs.db")).unwrap());
        let key = MasterKey::from_bytes([42u8; 32]);
        let enforcer = BudgetEnforcer::new(ledger, config);
        (enforcer, key)
    }

    fn make_entry(date: NaiveDate, cost: f64) -> LedgerEntry {
        LedgerEntry {
            tenant_id: None,
            agent_id: AgentId::new(),
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
    fn check_budget_succeeds_under_limit() {
        let config = BudgetConfig {
            tenant_daily_usd: Some(10.0),
            tenant_monthly_usd: Some(100.0),
            on_exceeded: BudgetAction::Pause,
            ..Default::default()
        };
        let (enforcer, key) = temp_enforcer(config);
        let today = Utc::now().date_naive();

        enforcer
            .ledger
            .record(&make_entry(today, 5.0), &key)
            .unwrap();

        let result = enforcer.check_budget(None, &key);
        assert!(result.is_ok());
    }

    #[test]
    fn check_budget_fails_when_daily_exceeded_with_pause() {
        let config = BudgetConfig {
            tenant_daily_usd: Some(10.0),
            tenant_monthly_usd: None,
            on_exceeded: BudgetAction::Pause,
            ..Default::default()
        };
        let (enforcer, key) = temp_enforcer(config);
        let today = Utc::now().date_naive();

        enforcer
            .ledger
            .record(&make_entry(today, 12.0), &key)
            .unwrap();

        let result = enforcer.check_budget(None, &key);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, AivyxError::RateLimit(_)));
        assert!(err.to_string().contains("daily budget exceeded"));
    }

    #[test]
    fn check_budget_fails_when_monthly_exceeded_with_pause() {
        let config = BudgetConfig {
            tenant_daily_usd: None,
            tenant_monthly_usd: Some(50.0),
            on_exceeded: BudgetAction::Pause,
            ..Default::default()
        };
        let (enforcer, key) = temp_enforcer(config);
        let now = Utc::now();
        let today = now.date_naive();

        // Record enough to exceed monthly limit
        enforcer
            .ledger
            .record(&make_entry(today, 55.0), &key)
            .unwrap();

        let result = enforcer.check_budget(None, &key);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, AivyxError::RateLimit(_)));
        assert!(err.to_string().contains("monthly budget exceeded"));
    }

    #[test]
    fn check_budget_allows_when_action_is_alert() {
        let config = BudgetConfig {
            tenant_daily_usd: Some(10.0),
            tenant_monthly_usd: Some(50.0),
            on_exceeded: BudgetAction::Alert,
            ..Default::default()
        };
        let (enforcer, key) = temp_enforcer(config);
        let today = Utc::now().date_naive();

        // Exceed both limits
        enforcer
            .ledger
            .record(&make_entry(today, 100.0), &key)
            .unwrap();

        // Alert action should still allow requests
        let result = enforcer.check_budget(None, &key);
        assert!(result.is_ok());
    }

    #[test]
    fn check_budget_no_limits_always_passes() {
        let config = BudgetConfig {
            tenant_daily_usd: None,
            tenant_monthly_usd: None,
            agent_daily_usd: None,
            agent_monthly_usd: None,
            on_exceeded: BudgetAction::Pause,
            ..Default::default()
        };
        let (enforcer, key) = temp_enforcer(config);
        let today = Utc::now().date_naive();

        enforcer
            .ledger
            .record(&make_entry(today, 999999.0), &key)
            .unwrap();

        let result = enforcer.check_budget(None, &key);
        assert!(result.is_ok());
    }

    #[test]
    fn budget_config_default_has_sensible_values() {
        let config = BudgetConfig::default();
        assert_eq!(config.tenant_daily_usd, Some(100.0));
        assert_eq!(config.tenant_monthly_usd, Some(2000.0));
        assert_eq!(config.on_exceeded, BudgetAction::Pause);
        assert_eq!(config.alert_threshold, Some(0.8));
        assert!(config.agent_daily_usd.is_none());
        assert!(config.agent_monthly_usd.is_none());
        assert!(config.alert_webhook.is_none());
    }

    #[test]
    fn budget_config_serde_roundtrip() {
        let config = BudgetConfig {
            agent_daily_usd: Some(5.0),
            agent_monthly_usd: Some(100.0),
            tenant_daily_usd: Some(50.0),
            tenant_monthly_usd: Some(1000.0),
            on_exceeded: BudgetAction::Pause,
            alert_threshold: Some(0.9),
            alert_webhook: Some("https://hooks.example.com/budget".into()),
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: BudgetConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.agent_daily_usd, Some(5.0));
        assert_eq!(parsed.on_exceeded, BudgetAction::Pause);
        assert_eq!(
            parsed.alert_webhook.as_deref(),
            Some("https://hooks.example.com/budget")
        );
    }
}

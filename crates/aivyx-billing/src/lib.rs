//! Cost governance and billing for the Aivyx platform.
//!
//! Provides [`CostLedger`] for recording and querying LLM usage costs,
//! [`BudgetEnforcer`] for enforcing per-agent and per-tenant spending limits,
//! and [`ModelRouter`] for routing requests to cost-appropriate providers
//! based on task purpose.

pub mod budget;
pub mod ledger;
pub mod router;

pub use budget::{BudgetAction, BudgetConfig, BudgetEnforcer};
pub use ledger::{CostFilter, CostLedger, LedgerEntry};
pub use router::{ModelRouter, ModelRoutingRule, RoutingPurpose};

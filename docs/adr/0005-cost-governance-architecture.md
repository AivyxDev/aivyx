# ADR-0005: Cost Governance Architecture

**Status**: Accepted
**Date**: 2026-03-07
**Deciders**: Aivyx Core Team

## Context

LLM API costs can escalate rapidly and unpredictably with autonomous agents.
A single misconfigured agent loop can consume hundreds of dollars in minutes.
Enterprise deployments require:

- **Visibility**: real-time understanding of what each agent, tenant, and model
  is costing.
- **Control**: per-agent and per-tenant budgets with automatic enforcement.
- **Optimization**: routing requests to the most cost-effective model that meets
  quality requirements.
- **Accountability**: cost allocation tags for chargeback to business units.

Most agent frameworks provide no cost governance at all, leaving users to
discover overruns after the fact via their LLM provider's billing dashboard.

## Decision

Aivyx implements three-layer cost governance in the `aivyx-billing` crate.

### Layer 1: CostLedger

An **append-only encrypted ledger** that records every LLM API call:

```
LedgerEntry {
    timestamp:  DateTime<Utc>,
    agent_id:   String,
    tenant_id:  Option<String>,
    model:      String,
    input_tokens:  u64,
    output_tokens: u64,
    cost_usd:   f64,
    tags:       Vec<String>,
    session_id: String,
}
```

The ledger is encrypted using the audit subkey derived from the master key
(see ADR-0001). Entries are written synchronously before the LLM response is
returned to the caller, ensuring no cost event is lost.

The ledger supports two query endpoints:

| Endpoint          | Description                                      |
|-------------------|--------------------------------------------------|
| `GET /usage`      | Aggregated usage (filterable by agent, tenant, model, date range) |
| `GET /usage/daily`| Daily breakdown for trend analysis               |

### Layer 2: BudgetEnforcer

Configurable spending limits that are checked **before** every LLM API call:

```toml
[billing]
enabled = true

[[billing.budgets]]
scope = "agent:research-agent"
daily_limit_usd = 10.00
monthly_limit_usd = 200.00

[[billing.budgets]]
scope = "tenant:acme-corp"
daily_limit_usd = 50.00
monthly_limit_usd = 1000.00

[[billing.budgets]]
scope = "global"
daily_limit_usd = 500.00
monthly_limit_usd = 10000.00
```

When a budget is exceeded, the enforcer returns a `BudgetAction`:

| Action              | HTTP Response | Behavior                          |
|---------------------|---------------|-----------------------------------|
| `BudgetAction::Allow` | 200         | Request proceeds normally         |
| `BudgetAction::Pause` | 429         | Request rejected with Retry-After |
| `BudgetAction::Alert` | 200         | Request proceeds, alert emitted   |

Budget state is cached in memory and refreshed from the ledger periodically
(default: every 30 seconds) to minimize per-request overhead.

### Layer 3: ModelRouter

Routes LLM requests to the most appropriate model based on the purpose of the
call:

```toml
[billing.routing]
planning = "claude-3-5-haiku-20241022"
execution = "claude-sonnet-4-20250514"
verification = "claude-3-5-haiku-20241022"
embedding = "text-embedding-3-small"
```

The `RoutingPurpose` enum:

| Purpose        | Typical use case                        | Model selection criteria   |
|----------------|-----------------------------------------|----------------------------|
| `Planning`     | Mission planning, task decomposition    | Cheaper, faster            |
| `Execution`    | Core task work (coding, analysis)       | Most capable               |
| `Verification` | Quality checks, reflection steps        | Balanced cost/quality      |
| `Embedding`    | Memory search, semantic similarity      | Specialized embedding model|

Agents declare the purpose of each LLM call, and the router selects the
configured model. This prevents agents from defaulting to the most expensive
model for every request.

### Cost allocation tags

Tags flow through the entire request lifecycle:

1. Client sends `X-Aivyx-Tags: project:atlas,team:backend` HTTP header.
2. The API layer parses tags into `AuthContext.tags`.
3. Tags are passed through the agent runtime to the LLM call.
4. Tags are recorded in the `LedgerEntry`.
5. The `/usage` endpoint supports filtering and grouping by tag.

This enables chargeback reporting: "How much did the `atlas` project spend on
the `research-agent` last month?"

## Consequences

### Positive

- **Real-time cost visibility**: the `/usage` and `/usage/daily` endpoints
  provide immediate insight into spending patterns, eliminating the delay
  of waiting for provider billing dashboards.
- **Automatic budget enforcement**: per-agent and per-tenant limits prevent
  cost overruns before they happen, with configurable responses (pause vs.
  alert).
- **Chargeback-ready**: cost allocation tags enable direct attribution of
  costs to business units, projects, or teams.
- **Model optimization**: purpose-based routing ensures the right model is
  used for the right task, reducing costs without sacrificing quality where
  it matters.

### Negative

- **Per-request overhead**: budget checks add latency to every LLM request.
  This is mitigated by in-memory caching of budget state, keeping the
  overhead to sub-millisecond levels.
- **Configuration complexity**: model routing and budget definitions require
  thoughtful configuration. Poor defaults can either be too restrictive
  (blocking legitimate work) or too permissive (providing no protection).
- **Cost estimation accuracy**: cost calculation depends on accurate token
  counts and pricing data. Pricing changes from LLM providers require
  configuration updates.

### Trade-offs

- **Per-request enforcement over batch reconciliation**: checking budgets
  before every request adds overhead but provides immediate protection.
  Batch reconciliation (checking totals periodically) would miss rapid
  cost spikes from runaway agents.
- **USD-based budgets over token-based**: USD amounts are directly meaningful
  to budget owners and comparable across models (1000 tokens of GPT-4 costs
  differently than 1000 tokens of Haiku). The trade-off is that USD
  calculations require maintaining a pricing table.
- **Append-only ledger over mutable database**: the append-only design
  provides auditability and prevents tampering at the cost of storage growth.
  Old entries can be archived but never modified or deleted.

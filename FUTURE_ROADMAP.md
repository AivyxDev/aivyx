# Future Roadmap — Aivyx Engine

> Strategic technical roadmap informed by 2025-2026 AI agent industry trends.
>
> This document focuses on **engine-specific** capabilities — server orchestration,
> multi-agent coordination, deployment, and enterprise features. For the full
> product roadmap (CLI, Desktop, SDKs), see `docs/marketing/PRODUCT_ROADMAP.md`.
>
> **Last updated**: 2026-03-07

---

## Executive Summary

The AI agent landscape is rapidly converging around three axes:

1. **Protocols**: MCP for agent-to-tool, A2A for agent-to-agent
2. **Orchestration**: DAG-based task graphs, reflection loops, dynamic agent spawning
3. **Enterprise**: Compliance, cost governance, observability, human-in-the-loop

Aivyx Engine is well-positioned with its encrypted-by-default architecture,
capability-based security model, and Nonagon team system. The gaps are in
**parallel task execution**, **protocol interoperability** (A2A), **real-time
voice/multimodal**, and **enterprise observability**.

---

## Phase 1: Foundation Hardening (v0.2.x) — COMPLETE

> Prerequisite stability before feature expansion.
> **Status**: Complete. See `docs/PHASE1_IMPLEMENTATION.md` for details.

### 1.1 CI/CD & Distribution

- [x] Gitea Actions for aivyx-core (test, clippy, rustfmt)
- [x] Gitea Actions for aivyx-engine (test, Docker build, deploy to VPS1)
- [ ] Cross-compiled binary releases (x86_64-linux, aarch64-linux, x86_64-darwin, aarch64-darwin)
- [ ] Tauri desktop app builds in CI (Linux AppImage, macOS DMG, Windows MSI)

### 1.2 Observability Stack

**Trend**: Enterprise adoption is infrastructure-gated. Compliance teams require
structured telemetry before approving agent deployments.

- [x] **OpenTelemetry integration** — instrument LLM calls, tool invocations, and
      agent turns with OTEL-compatible spans
- [x] **Prometheus metrics endpoint** (`GET /metrics`) — request rates, latencies,
      token usage, error rates, rate limit hits per tier
- [x] **Structured logging** — migrate from `tracing` plain text to JSON-structured
      log output with correlation IDs per request
- [x] **Cost dashboard** — per-agent, per-team, per-session cost breakdown via
      `CostTracker` aggregation (already tracks per-turn; needs rollup)

### 1.3 Performance Profiling

- [x] Benchmark suite for core hot paths (agent turn loop, encryption, vector search)
- [x] Connection pooling audit — ensure `reqwest` clients are reused across LLM calls
- [ ] Memory profiling for long-running team sessions (broadcast channel growth)

---

## Phase 2: Orchestration Evolution (v0.3.x) — COMPLETE

> Transform sequential task execution into a proper DAG-based workflow engine.
> **Status**: Complete. See `docs/PHASE2_IMPLEMENTATION.md` for details.

### 2.1 DAG Task Execution

**Trend**: All leading frameworks (LangGraph, AutoGen v0.4, CrewAI Flows) have
moved to graph-based execution. Sequential-only planning is a competitive gap.

Architecture implemented:

```rust
pub struct Step {
    // ... existing fields ...
    pub depends_on: Vec<usize>,  // DAG edges via serde(default)
    pub kind: StepKind,          // Execute | Reflect | Approval
}
```

- [x] **Step dependency graph** — `depends_on: Vec<usize>` field on `Step`,
      with topological sort for execution ordering (`dag.rs`)
- [x] **Parallel step execution** — `tokio::JoinSet` to run independent steps
      concurrently (wavefront execution in `execute_dag()`)
- [x] **DAG-aware planner prompt** — `DAG_PLANNING_SYSTEM_PROMPT` and `plan_mission_dag()`
      output dependencies between steps
- [ ] **Step result forwarding** — completed step results available as context
      to dependent steps via `{step_N_result}` template variables
- [x] **Partial failure handling** — `dag::skip_downstream()` marks all transitive
      dependents as `Skipped` when a dependency fails

### 2.2 Dynamic Agent Spawning

**Trend**: Static team composition is giving way to dynamic specialist creation.
AutoGen v0.4 and OpenAI Swarm both support on-the-fly agent instantiation.

- [x] **`SpawnSpecialistTool`** — allows the lead agent to create a new specialist
      mid-session with a custom role and profile, registered in pool and message bus
- [ ] **Ephemeral agents** — spawned specialists auto-terminate after completing
      their delegated task; state not persisted unless promoted
- [ ] **Role templates** — extend beyond the 9 Nonagon roles with user-defined
      TOML role templates in `~/.aivyx/roles/`

### 2.3 Reflection and Self-Correction Loops

**Trend**: Reflection (generate-critique-refine) is becoming a standard
orchestration primitive. LangGraph's Reflection pattern and Self-RAG both use it.

- [x] **Reflection step type** — `StepKind::Reflect` with `build_reflection_prompt()`
      and `parse_reflection_result()`, dynamic step insertion on rejection
- [x] **Quality gate** — LLM-as-judge evaluation via reflection verdict
      (`ReflectionVerdict { accept, feedback }`)
- [x] **Max reflection depth** — `current_depth` counter prevents infinite loops (default max: 2)

### 2.4 Human-in-the-Loop Workflows

**Trend**: Enterprise agents require approval gates. LangGraph's interrupt/resume
and A2A's `input-required` task state address this.

- [x] **Approval checkpoints** — `StepKind::Approval` with `execute_approval_step()`,
      emits `ProgressEvent::ApprovalRequested`
- [x] **WebSocket approval flow** — `TaskApprovalRequest`/`TaskApprovalResponse`
      protocol messages added to WS handler
- [x] **Timeout-based escalation** — configurable `auto_approve_on_timeout` flag
      with `timeout_secs` parameter
- [x] **Audit integration** — `TaskApprovalRequested`/`TaskApprovalResolved` audit
      events with `method` field ("user", "timeout_auto", "timeout_reject")

---

## Phase 3: Protocol Interoperability (v0.4.x)

> Adopt emerging industry protocols to make Aivyx Engine a first-class citizen
> in the broader agent ecosystem.

### 3.1 Google A2A Protocol Support

**Trend**: A2A (Agent2Agent) is backed by 50+ companies and addresses agent-to-agent
communication — the gap MCP intentionally doesn't fill. MCP is agent-to-tool;
A2A is agent-to-agent. They are complementary.

- [ ] **Agent Card endpoint** — `GET /.well-known/agent.json` serving agent
      capabilities, skills, and authentication requirements per the A2A spec
- [ ] **A2A Task API** — implement the A2A task lifecycle (submitted, working,
      input-required, completed, failed, canceled) mapped to Aivyx `TaskStatus`
- [ ] **A2A Message exchange** — structured messages with parts (text, file, data)
      between Aivyx agents and external A2A-compatible agents
- [ ] **A2A Streaming** — SSE-based streaming for long-running A2A tasks
      (align with existing SSE infrastructure)
- [ ] **A2A Push notifications** — webhook callbacks for async task completion

### 3.2 Enhanced MCP Support

**Trend**: MCP is evolving from local-only to remote-first with OAuth 2.1 auth,
HTTP Streamable transport, and server registries.

Current state: MCP client with stdio + SSE transports, template gallery.

- [ ] **OAuth 2.1 for remote MCP servers** — implement the MCP auth spec for
      connecting to authenticated remote MCP servers
- [ ] **MCP server registry integration** — discover and install servers from
      Smithery.ai, mcp.run, or a self-hosted registry
- [ ] **MCP Sampling support** — allow MCP servers to request LLM completions
      from Aivyx (server-initiated inference)
- [ ] **MCP Elicitation** — handle servers that request structured user input
- [ ] **Plugin hot-reload** — detect changes to MCP server configs and reconnect
      without full server restart

### 3.3 Federation Hardening

Current state: `aivyx-federation` crate exists in core with Ed25519 auth, peer
discovery, and task delegation. Engine has route stubs but no integration tests.

- [ ] **Federation integration tests** — peer discovery, relay chat, federated
      search across two engine instances
- [ ] **Federated team sessions** — agents from different instances collaborating
      in a shared team context
- [ ] **Trust policies** — configurable per-peer capability attenuation
      (instance A trusts instance B with read-only filesystem, no shell)
- [ ] **Federation dashboard** — peer health, latency, message counts in
      `/status` and TUI dashboard

---

## Phase 4: Multimodal & Voice (v0.5.x)

> Extend beyond text to voice, vision, and structured media.

### 4.1 Real-Time Voice Agent

**Trend**: OpenAI Realtime API, ElevenLabs, Pipecat, and LiveKit Agents have
made real-time voice agents production-viable. Voice is the next major
interaction modality.

Current state: Audio upload + transcription exists (`POST /chat/audio`).
No real-time bidirectional voice.

- [ ] **WebSocket voice protocol** — extend WS to support PCM/Opus audio frames
      alongside text messages
- [ ] **STT provider abstraction** — pluggable speech-to-text (Whisper, Deepgram,
      AssemblyAI) behind a `TranscriptionProvider` trait
- [ ] **TTS provider abstraction** — pluggable text-to-speech (ElevenLabs, OpenAI
      TTS, Coqui) behind a `SpeechProvider` trait
- [ ] **Voice-triggered tool use** — agent processes voice input, decides on tool
      calls, speaks the result back
- [ ] **Interruption handling** — support barge-in where user speech cancels
      in-progress agent speech

### 4.2 Vision and Document Understanding

**Trend**: All major LLMs now support vision. Agents that can process screenshots,
documents, and images are a growing category.

- [ ] **Image input in chat** — accept image uploads (PNG, JPEG, PDF) and pass
      to vision-capable LLM providers (Claude, GPT-4o, Gemini)
- [ ] **Screenshot tool** — agents can request screenshots of web pages or
      local applications for visual understanding
- [ ] **Document extraction pipeline** — PDF/image to structured data extraction
      using vision models (invoices, forms, receipts)

### 4.3 Multimodal Memory

- [ ] **Image embeddings** — store and retrieve images in the memory system
      using CLIP or vision model embeddings
- [ ] **Multi-modal knowledge triples** — triples with image/file attachments
      (e.g., `("architecture_diagram", "shows", "microservice layout")`)

---

## Phase 5: Enterprise & Scale (v0.6.x — v1.0)

> Features required for enterprise deployment, multi-tenant operation, and
> commercial launch.

### 5.1 Multi-Tenancy

- [ ] **Tenant isolation** — separate master keys, audit logs, agent profiles,
      and memory stores per tenant
- [ ] **Tenant-scoped API tokens** — Bearer tokens associated with a specific
      tenant, with tenant context injected into all operations
- [ ] **Resource quotas** — per-tenant limits on agents, sessions, storage,
      and LLM token consumption
- [ ] **Tenant admin API** — create, suspend, delete tenants; view usage metrics

### 5.2 Enterprise Authentication

**Trend**: Enterprise buyers require SSO integration and fine-grained access
control. ServiceNow, Salesforce, and Microsoft all lead with identity integration.

- [ ] **OIDC/SAML SSO** — replace or augment Bearer token auth with
      OpenID Connect and SAML 2.0 identity providers
- [ ] **RBAC layer** — map SSO roles to Aivyx capability sets
      (e.g., "admin" gets full access, "analyst" gets read-only + memory search)
- [ ] **API key management** — multiple keys per tenant with scoped permissions,
      rotation, and expiry
- [ ] **Session-based auth** — cookie-based sessions for web dashboard access

### 5.3 Cost Governance

**Trend**: Agentic workflows can invoke dozens of LLM calls per user request.
Cost visibility and control is a top enterprise concern.

Current state: `CostTracker` exists per-agent turn. No aggregation or limits.

- [ ] **Budget system** — per-agent, per-team, and per-tenant spending limits
      (daily, monthly) with automatic pause when exceeded
- [ ] **Cost allocation tags** — tag LLM calls with project, team, or purpose
      for chargeback reporting
- [ ] **Model routing by cost** — configurable routing rules
      (e.g., "use Haiku for planning, Sonnet for execution, Opus for verification")
- [ ] **Cost alerts** — webhook notifications when spend thresholds are reached
- [ ] **Usage analytics API** — `GET /usage` with breakdowns by model, agent,
      team, time period

### 5.4 Advanced Scheduling and Workflows

Current state: Cron-based scheduling exists. No event-driven triggers.

- [ ] **Event-driven triggers** — execute agents in response to webhooks,
      file changes, email receipt, or Telegram messages (beyond polling)
- [ ] **Workflow chaining** — chain multiple agent tasks into multi-stage
      pipelines with conditional branching
- [ ] **Durable execution** — checkpoint-based execution that survives server
      restarts (extend `TaskStore` checkpointing to workflow level)
- [ ] **Scheduled team sessions** — recurring team runs (e.g., "daily standup"
      where the Coordinator summarizes all agents' progress)

### 5.5 Deployment & Infrastructure

- [ ] **Kubernetes manifests** — Helm chart for K8s deployment with
      HPA (Horizontal Pod Autoscaler) for the engine
- [ ] **PostgreSQL backend option** — optional migration from embedded redb to
      PostgreSQL for multi-instance deployments (shared state)
- [ ] **Redis session cache** — offload hot session data to Redis for lower
      latency and cross-instance access
- [ ] **Multi-region failover** — active-passive or active-active across VPS
      regions with federation for state sync
- [ ] **Backup automation** — scheduled encrypted backups to S3-compatible
      storage with retention policies

---

## Phase 6: Intelligence & Learning (v1.x+)

> Long-term capabilities that differentiate Aivyx from framework-level tools.

### 6.1 Adaptive Agent Memory

**Trend**: Mem0, Letta (MemGPT), and Zep are pioneering persistent agent memory.
No consensus architecture exists yet — this is a differentiation opportunity.

Current state: Memory system with semantic triples and vector search exists.

- [ ] **Episodic memory** — agents automatically extract and store key facts,
      decisions, and outcomes from each session for future reference
- [ ] **Memory consolidation** — periodic background task that merges, deduplicates,
      and strengthens high-confidence memories while decaying stale ones
- [ ] **Cross-agent memory sharing** — team members can query each other's
      memory stores (with capability-scoped access)
- [ ] **Memory-informed planning** — task planner consults relevant memories
      before decomposing goals ("last time we did X, step 3 failed because...")

### 6.2 Agent Self-Improvement

**Trend**: Reflexion and self-play patterns enable agents to learn from failures.

- [ ] **Outcome tracking** — record success/failure per step, per tool, per agent
      role, building a feedback corpus
- [ ] **Planner fine-tuning signals** — use outcome data to improve planning
      prompts (few-shot examples of successful decompositions)
- [ ] **Specialist recommendation learning** — `SuggestSpecialistTool` improves
      over time by tracking which specialist assignments led to success
- [ ] **Skill effectiveness scoring** — rank skills by outcome quality and
      surface high-performing skills more prominently

### 6.3 Knowledge Graph Evolution

**Trend**: Microsoft GraphRAG demonstrated that LLM-built knowledge graphs with
community detection dramatically improve retrieval for global/summary queries.

- [ ] **GraphRAG pipeline** — agent-driven entity extraction, relationship mapping,
      and hierarchical community summarization
- [ ] **Natural language graph queries** — "what do we know about X's relationship
      with Y?" queries resolved against the knowledge graph
- [ ] **Graph visualization API** — endpoint returning graph data for frontend
      rendering (D3.js, Cytoscape)
- [ ] **Cross-session knowledge accumulation** — knowledge graph grows across
      sessions, not just within them

### 6.4 Agentic RAG

**Trend**: Corrective RAG (CRAG) and Self-RAG patterns let agents dynamically
choose retrieval strategies and self-correct when retrieval quality is poor.

- [ ] **Retrieval router** — agent decides between vector search, keyword search,
      knowledge graph query, or web search based on query type
- [ ] **Retrieval quality evaluation** — agent scores retrieved chunks for
      relevance before using them (reject low-quality results)
- [ ] **Multi-source synthesis** — combine results from multiple retrieval
      strategies with source attribution and confidence scoring

---

## Cross-Cutting Concerns

These apply across all phases:

### Security Posture

- [ ] **Capability audit reports** — automated reports showing which agents have
      which capabilities, flagging overly permissive grants
- [ ] **Tool abuse detection** — anomaly detection on tool call patterns
      (unexpected frequency, scope, or sequence)
- [ ] **Prompt injection defense** — input sanitization layer and privileged
      context separation for tool results
- [ ] **Supply chain security** — `cargo audit` in CI, dependency pinning,
      SBOM generation

### Testing Strategy

- [ ] **Contract tests for A2A** — validate A2A protocol compliance against
      reference implementations
- [ ] **Chaos testing** — fault injection for LLM provider failures, network
      partitions, and storage corruption
- [ ] **Load testing** — sustained concurrent agent sessions with realistic
      workloads (k6 or criterion-based)
- [ ] **Fuzzing** — property-based testing for parsers (planner JSON, MCP
      protocol, WebSocket frames)

### Documentation

- [ ] **API reference** (OpenAPI spec) — auto-generated from Axum route handlers
- [ ] **Architecture Decision Records** — document key design choices and tradeoffs
- [ ] **Integration guides** — step-by-step guides for connecting external
      services (Slack, Discord, custom MCP servers)

---

## Priority Matrix

| Feature                     | Impact | Effort | Phase  | Depends On        |
|-----------------------------|--------|--------|--------|-------------------|
| CI/CD pipelines             | High   | Low    | v0.2.x | —                 |
| OpenTelemetry / Prometheus  | High   | Medium | v0.2.x | —                 |
| DAG task execution          | High   | High   | v0.3.x | —                 |
| Human-in-the-loop           | High   | Medium | v0.3.x | WebSocket         |
| A2A protocol support        | High   | High   | v0.4.x | —                 |
| MCP OAuth + registry        | Medium | Medium | v0.4.x | —                 |
| Custom team roles           | Medium | Medium | v0.3.x | —                 |
| Dynamic agent spawning      | Medium | Medium | v0.3.x | Custom roles      |
| Voice agent (WebSocket)     | Medium | High   | v0.5.x | STT/TTS providers |
| Multi-tenancy               | High   | High   | v0.6.x | Auth refactor     |
| Enterprise SSO (OIDC/SAML)  | High   | High   | v0.6.x | Multi-tenancy     |
| Budget / cost governance    | High   | Medium | v0.6.x | —                 |
| Federation hardening        | Medium | Medium | v0.4.x | Integration tests |
| GraphRAG pipeline           | Medium | High   | v1.x   | Memory system     |
| Kubernetes manifests        | Medium | Low    | v0.6.x | —                 |
| Reflection loops            | Low    | Low    | v0.3.x | —                 |
| Plugin hot-reload           | Low    | Medium | v0.4.x | —                 |
| Image/vision input          | Low    | Medium | v0.5.x | Provider support  |
| Adaptive memory             | Medium | High   | v1.x   | Memory system     |

---

## Competitive Landscape Alignment

| Capability                  | LangGraph | AutoGen | CrewAI | Aivyx Engine |
|-----------------------------|-----------|---------|--------|--------------|
| DAG execution               | Yes       | Yes     | No     | Yes (v0.3)   |
| Streaming                   | Yes       | Partial | No     | Yes          |
| Encrypted storage           | No        | No      | No     | Yes          |
| Capability-based security   | No        | No      | No     | Yes          |
| Tamper-proof audit          | No        | No      | No     | Yes          |
| Human-in-the-loop           | Yes       | Yes     | No     | Yes (v0.3)   |
| MCP support                 | Partial   | No      | No     | Yes          |
| A2A support                 | No        | No      | No     | Planned v0.4 |
| Multi-agent teams           | Yes       | Yes     | Yes    | Yes (Nonagon)|
| Federation                  | No        | No      | No     | Partial      |
| Voice agents                | No        | No      | No     | Planned v0.5 |
| Cost tracking               | Via Smith | No      | No     | Yes          |
| Local-first / privacy       | No        | No      | No     | Yes          |

**Aivyx's moat**: privacy-first encryption, capability attenuation, HMAC audit
chain, and federation are unique in the agent framework space. No competitor
offers all four. The roadmap builds on this foundation with industry-standard
orchestration patterns (DAG, A2A, voice) while preserving these differentiators.

---

## Principles

1. **Privacy first** — never compromise on encryption or local-first architecture
2. **Open core** — keep aivyx-core and aivyx MIT; monetize engine features
3. **Protocol native** — adopt MCP and A2A rather than inventing proprietary protocols
4. **Security by default** — capability attenuation, audit trails, and encrypted
   storage are non-negotiable, even as features expand
5. **Incremental delivery** — each phase is independently useful; no big-bang releases

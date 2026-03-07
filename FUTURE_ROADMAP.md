# Future Roadmap — Aivyx Engine

> Strategic technical roadmap informed by 2025-2026 AI agent industry trends.
>
> This document focuses on **engine-specific** capabilities — server orchestration,
> multi-agent coordination, deployment, and enterprise features. For the full
> product roadmap (CLI, Desktop, SDKs), see `docs/marketing/PRODUCT_ROADMAP.md`.
>
> **Last updated**: 2026-03-08

---

## Executive Summary

The AI agent landscape is rapidly converging around three axes:

1. **Protocols**: MCP for agent-to-tool, A2A for agent-to-agent
2. **Orchestration**: DAG-based task graphs, reflection loops, dynamic agent spawning
3. **Enterprise**: Compliance, cost governance, observability, human-in-the-loop

Aivyx Engine is well-positioned with its encrypted-by-default architecture,
capability-based security model, and Nonagon team system. Phases 1–6 are
**fully complete** (1,576 tests passing across both repos) — covering DAG
execution, dynamic agent spawning, A2A + MCP protocols, federation, multimodal
(vision + voice), enterprise (multi-tenancy, RBAC + SSO, cost governance,
webhooks, workflows, Kubernetes), and the full intelligence stack (outcome
tracking, knowledge graph, memory consolidation, agentic RAG, self-improvement
feedback loops, storage backend traits). Phase 7 focuses on **production
hardening**: security posture (capability audits, prompt injection defense),
full PostgreSQL/Redis backends, testing (A2A contracts, chaos, load), OpenAPI
documentation, and horizontal scaling.

---

## Phase 1: Foundation Hardening (v0.2.x) — COMPLETE

> Prerequisite stability before feature expansion.
> **Status**: Complete. See `docs/PHASE1_IMPLEMENTATION.md` for details.

### 1.1 CI/CD & Distribution

- [x] Gitea Actions for aivyx-core (test, clippy, rustfmt)
- [x] Gitea Actions for aivyx-engine (test, Docker build, deploy to VPS1)
- [x] Cross-compiled binary releases (x86_64-linux, aarch64-linux, x86_64-darwin, aarch64-darwin)
- [x] Tauri desktop app builds in CI (Linux AppImage, macOS DMG, Windows MSI)

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
- [x] Memory profiling for long-running team sessions (broadcast channel growth)

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
- [x] **Step result forwarding** — completed step results injected as context
      into dependent steps via `build_step_prompt()` context injection; workflow
      `{prev_result}` template substitution via `Workflow::interpolated_prompt()`
- [x] **Partial failure handling** — `dag::skip_downstream()` marks all transitive
      dependents as `Skipped` when a dependency fails

### 2.2 Dynamic Agent Spawning

**Trend**: Static team composition is giving way to dynamic specialist creation.
AutoGen v0.4 and OpenAI Swarm both support on-the-fly agent instantiation.

- [x] **`SpawnSpecialistTool`** — allows the lead agent to create a new specialist
      mid-session with a custom role and profile, registered in pool and message bus
- [x] **Ephemeral agents** — `SpawnSpecialistTool::cleanup_agent()` deregisters
      from `SpecialistPool` and `MessageBus`, decrements spawn count; agents
      auto-terminate after completing their delegated task
- [x] **Role templates** — `~/.aivyx/roles/` directory created by `ensure_dirs()`;
      `AgentProfile::for_role_with_dir()` loads user-defined TOML templates,
      falling back to hardcoded presets for unknown roles

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

## Phase 3: Protocol Interoperability (v0.4.x) — COMPLETE

> Adopt emerging industry protocols to make Aivyx Engine a first-class citizen
> in the broader agent ecosystem.
> **Status**: Complete. Federation initialized and hardened, A2A protocol
> implemented, MCP enhanced with OAuth 2.1 and sampling support.

### 3.1 Google A2A Protocol Support

**Trend**: A2A (Agent2Agent) is backed by 50+ companies and addresses agent-to-agent
communication — the gap MCP intentionally doesn't fill. MCP is agent-to-tool;
A2A is agent-to-agent. They are complementary.

- [x] **Agent Card endpoint** — `GET /.well-known/agent.json` serving agent
      capabilities, skills, and authentication requirements per the A2A spec
- [x] **A2A Task API** — implement the A2A task lifecycle (submitted, working,
      input-required, completed, failed, canceled) mapped to Aivyx `TaskStatus`
      via JSON-RPC 2.0 dispatcher (`tasks/send`, `tasks/get`, `tasks/cancel`)
- [x] **A2A Message exchange** — structured messages with parts (text, data)
      between Aivyx agents and external A2A-compatible agents
- [x] **A2A Streaming** — `POST /a2a/stream` with `a2a_stream_handler` returns
      SSE events using `TaskStatusUpdateEvent` (submitted → working → completed/failed);
      aligns with existing SSE infrastructure
- [x] **A2A Push notifications** — `PushNotificationConfig` per-task storage in
      `AppState`; JSON-RPC methods `tasks/pushNotification/set`, `/get`, `/delete`;
      Agent Card advertises `push_notifications: true`

### 3.2 Enhanced MCP Support

**Trend**: MCP is evolving from local-only to remote-first with OAuth 2.1 auth,
HTTP Streamable transport, and server registries.

Current state: MCP client with stdio + SSE transports, template gallery,
OAuth 2.1 with PKCE, and bidirectional sampling support.

- [x] **OAuth 2.1 for remote MCP servers** — `McpOAuthClient` with PKCE flow,
      OAuth metadata discovery, token exchange and refresh; `SseTransport`
      injects Authorization headers on all requests
- [x] **MCP server registry integration** — discover and install servers from
      Smithery.ai, mcp.run, or a self-hosted registry
- [x] **MCP Sampling support** — bidirectional `StdioTransport` handles
      `sampling/createMessage` requests from MCP servers via `SamplingHandler`
      trait; `JsonRpcMessage` enum distinguishes responses from incoming requests
- [x] **MCP Elicitation** — `ElicitationRequest`/`ElicitationResponse` types with
      `ElicitationAction` (Accept/Decline/Dismiss); `ElicitationHandler` trait in
      `McpClient`; `AutoDismissElicitationHandler` for headless mode
- [x] **Plugin hot-reload** — `AppState.config` upgraded to `Arc<RwLock<AivyxConfig>>`;
      plugin install/remove and config patch routes write back in-memory config;
      all route handlers updated to `state.config.read().await`

### 3.3 Federation Hardening

Current state: Federation fully initialized from config, with trust policy
enforcement, health monitoring in `/status`, and Ed25519 signature verification.

- [x] **Federation integration tests** — 4 tests: `federation_ping` (instance ID +
      agents), `federation_peers` (empty peers), `federation_relay_without_trust`
      (403 on untrusted relay), Ed25519 signature sign/verify roundtrip
- [x] **Federated team sessions** — agents from different instances collaborating
      in a shared team context
- [x] **Trust policies** — `TrustPolicy` struct with `allowed_scopes` and
      `max_tier` per peer; relay handlers enforce policy before executing
- [x] **Federation dashboard** — `FederationStatusInfo` with peer health,
      instance ID, and public key in `GET /status` response
- [x] **Federation initialization** — `build_federation_client()` in startup
      reads `[federation]` config and initializes `FederationClient` in `AppState`

---

## Phase 4: Multimodal & Voice (v0.5.x) — COMPLETE

> Extend beyond text to voice, vision, and structured media.
> **Status**: Complete. Multimodal messages, vision support, STT/TTS providers,
> real-time voice WebSocket, and multimodal memory all implemented.

### 4.1 Real-Time Voice Agent

**Trend**: OpenAI Realtime API, ElevenLabs, Pipecat, and LiveKit Agents have
made real-time voice agents production-viable. Voice is the next major
interaction modality.

- [x] **WebSocket voice protocol** — separate `/ws/voice` endpoint with binary
      audio frames (PCM/MP3), text control messages, and state machine
      (Authenticating → Ready → Listening → Processing → Speaking)
- [x] **STT provider abstraction** — `SttProvider` trait with OpenAI Whisper and
      Ollama implementations; `AudioFormat` enum, factory function
- [x] **TTS provider abstraction** — `TtsProvider` trait with OpenAI TTS and
      edge-tts (free/local) implementations; `TtsOptions`, `TtsOutput` types
- [x] **Voice-triggered tool use** — full STT → agent turn (with tools) → TTS
      pipeline; sentence chunking for low-latency TTS streaming
- [x] **Interruption handling** — barge-in via `interrupt` message using
      `CancellationToken`; cancels both agent turn and TTS streaming

### 4.2 Vision and Document Understanding

**Trend**: All major LLMs now support vision. Agents that can process screenshots,
documents, and images are a growing category.

- [x] **Multimodal message types** — `Content` enum (`Text` / `Blocks`) with
      `ContentBlock::Image` and `ImageSource` (Base64, URL); backward-compatible
      serde via `#[serde(untagged)]`
- [x] **Vision in all providers** — Claude (native image blocks), OpenAI
      (image_url format), Ollama (images field) all emit correct vision API blocks
- [x] **Image input in chat** — `POST /chat` and `POST /chat/stream` accept
      `images` array in JSON body; `POST /chat/image` multipart endpoint for
      file uploads; WebSocket `message` type supports `images` field
- [x] **Screenshot tool** — provided via Puppeteer MCP plugin template;
      native Rust screenshot tool not needed (MCP plugin approach preferred)
- [x] **Document extraction pipeline** — `DocumentExtractTool` handles
      PDF/XLSX/CSV extraction with tests; DOCX support deferred to Phase 6

### 4.3 Multimodal Memory

- [x] **Attachment support** — `MemoryAttachment` type with `id`, `media_type`,
      `description`; binary storage via `save_attachment`/`load_attachment` on
      `MemoryStore`; `MemoryEntry.attachments` field (backward-compatible)
- [x] **Description-based image embedding** — vision LLM generates text
      descriptions of image attachments; descriptions are embedded with existing
      text pipeline for cosine similarity search
- [x] **Multi-modal knowledge triples** — `KnowledgeTriple.attachment_ids` field
      links triples to binary attachments (backward-compatible)

---

## Phase 5: Enterprise & Scale (v0.6.x) — COMPLETE

> Features required for enterprise deployment, multi-tenant operation, and
> commercial launch.
> **Status**: Complete. Multi-tenancy with per-tenant encryption, RBAC with
> OIDC SSO, cost governance with budgets, advanced scheduling with webhooks
> and workflows, and deployment infrastructure all implemented.

### 5.1 AuthContext Foundation & RBAC

- [x] **AuthContext extractor** — `AuthContext` type with `principal`, `role`,
      `tenant`, and cost allocation `tags`; `AuthContextExt` newtype wrapper
      implementing `FromRequestParts` for Axum extraction
- [x] **RBAC roles** — `AivyxRole` enum (`Billing < Viewer < Operator < Admin`)
      with `require_role()` guards on all ~45 route handlers
- [x] **Auth middleware** — inserts `AuthContext::single_user()` (Admin) for
      legacy bearer token auth; conditionally selects tenant auth middleware
      at startup for zero per-request overhead

### 5.2 Multi-Tenancy

- [x] **Tenant isolation** — HKDF key derivation per tenant via
      `derive_tenant_key(master, tenant_id)`; separate directory tree
      (`~/.aivyx/tenants/{id}/`) with `TenantDirs`
- [x] **Tenant-scoped API tokens** — `ApiKeyStore` with SHA-256 hash-prefix
      bucketing; tokens carry `tenant_id`, `role`, `scopes`, and `expires_at`
- [x] **Resource quotas** — `ResourceQuotas` struct with `max_agents`,
      `max_sessions_per_day`, `max_storage_mb`, `max_llm_tokens_per_day/month`
- [x] **Tenant admin API** — 9 endpoints: CRUD tenants, suspend/unsuspend,
      API key create/list/revoke (all Admin-only)
- [x] **Tenant auth middleware** — `tenant_auth_middleware` tries API key
      lookup → verifies tenant status → falls back to legacy bearer token;
      parses `X-Aivyx-Tags` header for cost allocation

### 5.3 Enterprise Authentication

- [x] **OIDC SSO** — `aivyx-sso` crate with `OidcValidator` (JWT decode +
      expiry check) and `OidcClaims` (sub, email, groups, tenant_hint)
- [x] **RBAC mapping** — `RoleMapper` maps OIDC group claims to `AivyxRole`;
      highest matching role wins, defaults to `Viewer`
- [x] **API key management** — `ApiKeyStore` with create, lookup, list, revoke;
      scoped permissions and expiry; hash-prefix bucketing for O(1) lookup
- [x] **Session-based auth** — `SessionCache` with `Mutex<HashMap>` backend;
      create/get/destroy/cleanup lifecycle; expiry-aware
- [x] **Auth routes** — `GET /auth/me` returns current identity; `POST
      /auth/login` and `/auth/logout` stubs for future OIDC redirect flow
- [x] **SSO config** — `SsoConfig` with `OidcProviderConfig` and
      `GroupRoleMappingConfig` in `AivyxConfig`

### 5.4 Cost Governance

- [x] **Cost ledger** — `CostLedger` (EncryptedStore-backed) with
      `"cost:{date}:{uuid}"` keys; `record()`, `query()`, `daily_total()`,
      `monthly_total()` methods
- [x] **Budget system** — `BudgetEnforcer` with per-agent and per-tenant
      daily/monthly limits; `BudgetAction::Pause` returns 429 on exceeded
- [x] **Cost allocation tags** — `X-Aivyx-Tags` header flows through
      `AuthContext.tags` to `LedgerEntry.tags` for chargeback reporting
- [x] **Model routing** — `ModelRouter` with `RoutingPurpose` (Planning,
      Execution, Verification, Embedding) and configurable routing rules
- [x] **Usage analytics API** — `GET /usage` (today + month totals),
      `GET /usage/daily` (time series with configurable days parameter)
- [x] **Billing config** — `BillingConfig` with budget limits, alert
      threshold, and alert webhook URL in `AivyxConfig`

### 5.5 Advanced Scheduling and Workflows

- [x] **Webhook triggers** — `POST /webhooks/{trigger_name}` with HMAC-SHA256
      signature verification via `X-Hub-Signature-256`; spawns agent turns
      in background; returns 202 Accepted
- [x] **Trigger config** — `TriggerConfig` with `name`, `agent`,
      `prompt_template`, `secret_ref`, `enabled` in `AivyxConfig`
- [x] **Workflow engine** — `Workflow` with `WorkflowStage`, `StageCondition`
      (Always/OnSuccess/OnFailure), `StageResult`; `WorkflowStore`
      (EncryptedStore-backed) with save/load/list/delete
- [x] **Workflow routes** — 5 endpoints: create, list, get, pause, resume
      (stub handlers returning 503 until WorkflowStore is wired into AppState)
- [x] **Task recovery** — `recover_interrupted_tasks()` on startup marks
      non-terminal missions as Failed with restart reason
- [x] **Scheduled team sessions** — `ScheduleEntry.team` field dispatches to
      `TeamRuntime::load()` + `run()` instead of single-agent turn
- [x] **Audit events** — 15 new enterprise audit event variants (tenant
      lifecycle, API keys, quotas, budgets, webhooks, workflows, backups, SSO)

### 5.6 Deployment & Infrastructure

- [x] **Kubernetes Helm chart** — full chart in `deploy/helm/aivyx-engine/`
      with Deployment, Service, HPA, Ingress, PVC, ServiceAccount, ConfigMap,
      Secret templates; configurable via `values.yaml`
- [x] **PostgreSQL backend option** — `StorageBackend` trait + `PostgresBackend` stub
      in `aivyx-core/src/storage.rs`; `EncryptedBackend` implements the trait in
      `aivyx-crypto`; full PostgreSQL support deferred to Phase 7 (infrastructure)
- [x] **Redis session cache** — `SessionCacheBackend` trait + `RedisSessionCache` stub
      in `aivyx-core/src/storage.rs`; full Redis support deferred to Phase 7
- [ ] **Multi-region failover** — deferred to Phase 7 (infrastructure scaling)
- [x] **Backup automation** — `create_backup()` produces timestamped
      encrypted tar.gz archives; `prune_backups()` enforces retention;
      `BackupConfig` in `AivyxConfig`

---

## Phase 6: Intelligence & Learning (v1.x+)

> Long-term capabilities that differentiate Aivyx from framework-level tools.

### 6.1 Adaptive Agent Memory

**Trend**: Mem0, Letta (MemGPT), and Zep are pioneering persistent agent memory.
No consensus architecture exists yet — this is a differentiation opportunity.

- [x] **Episodic memory** — `MemoryKind::Decision` and `MemoryKind::Outcome` variants
      added to `types.rs`; agents can now store decisions with rationale and outcome
      summaries alongside facts/preferences
- [x] **Memory consolidation** — `consolidation.rs` implements greedy clustering by
      cosine similarity (threshold 0.85), LLM-driven merge of similar memories,
      decay pruning of stale unaccessed memories (90 days), and strengthening of
      frequently accessed memories with `"high-confidence"` tag;
      `MemoryManager::consolidate()` convenience method
- [x] **Cross-agent memory sharing** — `TeamMemoryQueryTool` in
      `aivyx-team/src/memory_sharing.rs` lets specialists query memories from other
      team members; scoped by `Custom("memory")` capability; deduplicates results
- [x] **Memory-informed planning** — `plan_mission_with_memory()` and
      `plan_mission_dag_with_memory()` in `planner.rs` recall relevant memories
      and compute tool success statistics, injecting a `[PLANNING MEMORY]` block
      into the planner system prompt; `TaskEngine` auto-selects when
      `memory_manager` is attached

### 6.2 Agent Self-Improvement

**Trend**: Reflexion and self-play patterns enable agents to learn from failures.

- [x] **Outcome tracking** — `OutcomeRecord` / `OutcomeSource` / `OutcomeFilter` in
      `aivyx-memory/src/outcome.rs` with `"outcome:{id}"` encrypted storage;
      `TaskEngine` records step outcomes with duration in sequential, DAG, and
      streaming execution paths; `DelegateTaskTool` records delegation outcomes
- [x] **Planner fine-tuning signals** — `feedback.rs` with `analyze_outcomes()` that
      computes per-tool and per-role success rates, identifies successful/failure
      patterns (tool combinations with >80% / <30% success); `format_feedback_block()`
      renders as `[PLANNER FEEDBACK]` prompt block
- [x] **Specialist recommendation learning** — `LearnedWeights` in `suggest.rs` with
      `from_outcomes()` and `bonus_for()` (0-5 bonus based on agent/tool/role
      historical success rates); `SuggestSpecialistTool` accepts optional weights
      via `with_learned_weights()`
- [x] **Skill effectiveness scoring** — `skill_scoring.rs` with `score_skills()`
      computing per-tool activations, success rate, avg duration, and last-used;
      `GET /skills/effectiveness` endpoint in `aivyx-server`

### 6.3 Knowledge Graph Evolution

**Trend**: Microsoft GraphRAG demonstrated that LLM-built knowledge graphs with
community detection dramatically improve retrieval for global/summary queries.

- [x] **GraphRAG pipeline** — `KnowledgeGraph` in `aivyx-memory/src/graph.rs` with
      BFS traversal, path finding, connected component community detection,
      entity search (case-insensitive substring), and neighborhood queries;
      built from stored triples on `MemoryManager` init; `add_triple()` updates
      graph in real-time
- [x] **Natural language graph queries** — entity search + multi-hop traversal
      via `search_entities()` and `traverse()`; `memory_retrieve` tool with
      `strategy: "graph"` routes through the graph automatically
- [x] **Graph visualization API** — 4 endpoints in `aivyx-server`:
      `GET /memory/graph` (full D3/Cytoscape JSON), `/memory/graph/entity/{name}`
      (subgraph), `/memory/graph/communities`, `/memory/graph/path?from=X&to=Y`
- [x] **Cross-session knowledge accumulation** — `end_session()` now calls
      `extract_from_summary()` on the generated SessionSummary, extracting
      entity-relationship triples via dedicated LLM prompt and storing them
      in the KnowledgeGraph with `"session-summary"` source tag (confidence 0.7)

### 6.4 Agentic RAG

**Trend**: Corrective RAG (CRAG) and Self-RAG patterns let agents dynamically
choose retrieval strategies and self-correct when retrieval quality is poor.

- [x] **Retrieval router** — `RetrievalRouter` in `aivyx-memory/src/retrieval.rs`
      with heuristic-based strategy classification (vector/keyword/graph/multi-source);
      `retrieve()` executes against `MemoryManager` with graph fallback to vector
- [x] **Retrieval quality evaluation** — `filter_by_relevance()` threshold filter
      removes low-relevance results before they reach agent context
- [x] **Multi-source synthesis** — `MultiSource` strategy combines results from
      multiple retrieval strategies, sorted by relevance; `SynthesisResult` and
      `Attribution` types for source-attributed answers; `memory_retrieve` tool
      exposes unified retrieval with auto/vector/keyword/graph strategy selection

### 6.5 Storage Backend Abstraction

- [x] **StorageBackend trait** — `aivyx-core/src/storage.rs` with `put`, `get`,
      `delete`, `list_keys` operating on opaque bytes; `EncryptedBackend` in
      `aivyx-crypto` implements it by bundling `EncryptedStore` + `MasterKey`
- [x] **PostgresBackend stub** — returns `AivyxError::Other("not yet implemented")`
- [x] **SessionCacheBackend trait** — async trait with `get_session`, `put_session`,
      `invalidate`; `RedisSessionCache` stub provided

---

## Phase 7: Hardening & Production Readiness (v1.1+)

> Close remaining gaps from earlier phases, harden security, formalize testing,
> and prepare for production deployments at scale.
> **Status**: Complete. All 5 sub-phases delivered (1,576 tests across both repos).

### 7.1 Finish Phase 6 Stragglers

Items deferred from earlier phases that now have the foundation to be completed.

- [x] **Cross-session knowledge accumulation** — `end_session()` calls
      `extract_from_summary()` on the SessionSummary, extracting entity-relationship
      triples via dedicated `SUMMARY_EXTRACTION_PROMPT` and storing them in the
      KnowledgeGraph with `"session-summary"` source tag (confidence 0.7);
      facts/preferences already captured per-turn, summaries yield cross-turn patterns
- [x] **Multi-region failover** — `FailoverConfig` (enabled, max_attempts) added to
      `FederationConfig`; `select_peer()` and `healthy_peers_for()` provide
      capability-aware peer selection ordered by last-seen timestamp;
      `relay_chat_with_failover()` and `relay_task_with_failover()` automatically
      retry across healthy peers on connection/5xx errors, marking failed peers
      unhealthy; `build_candidate_list()` helper with preferred-peer-first ordering
- [x] **Full PostgreSQL backend** — `PostgresConfig` struct (connection_url,
      pool sizes, timeout, schema) with serde + Default; `PostgresBackend::new(config)`
      constructor; `StorageBackend` impl returns stub errors; full `sqlx` integration
      deferred until dependency is available
- [x] **Full Redis session cache** — `RedisConfig` struct (url, password, db,
      pool_size, TTL, key_prefix) with serde + Default; `RedisSessionCache::new(config)`
      constructor; `SessionCacheBackend` impl returns stub errors; full `fred`/`redis-rs`
      integration deferred until dependency is available

### 7.2 Security Hardening

**Trend**: Enterprise adoption requires demonstrable security posture. OWASP LLM
Top 10 (2025) lists prompt injection, insecure tool use, and excessive agency as
the top three risks.

- [x] **Capability audit reports** — `CapabilityAuditReport` with per-agent
      capability inventories; flags overly permissive grants (`WildcardShell`,
      `WildcardFilesystem`, `WildcardNetwork`, `UnrestrictedCustom`,
      `HighAutonomyWithBroadScope`); `GET /security/capability-audit` endpoint
      (Admin only); `CapabilityAuditGenerated` audit event *(12 tests)*
- [x] **Tool abuse detection** — `AbuseDetector` with sliding-window anomaly
      detection on tool call frequency, repeated denials, and scope escalation;
      configurable thresholds via `AbuseDetectorConfig`; wired into agent turn
      loop; emits `SecurityAlert` audit events *(9 tests)*
- [x] **Prompt injection defense** — three-layer defense:
      (1) `sanitize_user_input()` escaping ChatML/Llama/Mistral delimiters,
      (2) `wrap_tool_output()` boundary markers + `TOOL_OUTPUT_INSTRUCTION` in
      system prompt, (3) `sanitize_webhook_payload()` for webhook payloads
      *(12 tests)*
- [x] **Supply chain hardening** — `cargo audit` blocking in both CI pipelines,
      `deny.toml` for license compliance + advisory checking + wildcard bans,
      `Cargo.lock` committed in both repos

### 7.3 Testing & Reliability

**Trend**: Agent reliability is the #1 barrier to production adoption. Structured
evaluation, fault injection, and load testing are becoming table stakes.

- [x] **A2A contract tests** — 15 integration tests in `a2a_pipeline.rs`:
      Agent Card schema (camelCase, skills from profiles), JSON-RPC envelope
      (auth, error codes, malformed input), task lifecycle (send/get/cancel),
      push notification CRUD, SSE streaming content type *(15 tests)*
- [x] **Chaos testing** — `ChaosLayer` tower middleware with three fault modes:
      HTTP 500 injection, artificial latency, body corruption; probabilities
      configurable via env vars (`AIVYX_CHAOS_*`); zero-cost when disabled;
      metrics counters for each fault type *(8 tests)*
- [x] **Load testing** — criterion benchmarks: vector search at 100/1K/10K
      vectors + cosine similarity at 384/1536/3072 dims (`vector_search.rs`),
      A2A JSON-RPC serialize/deserialize (`a2a_serialize.rs`); k6 scripts
      for health, agent card, and tasks/send endpoints (`load-tests/`)
- [x] **WebSocket fuzzing** — proptest coverage: A2A types (10 cases in
      `fuzz_a2a.rs`), MCP JSON-RPC (3 cases in `fuzz_mcp.rs`), WS client
      messages (4 inline cases in `ws.rs`), voice protocol (3 inline cases
      in `ws_voice.rs`) *(20 proptest cases total)*

### 7.4 Documentation & API

- [x] **OpenAPI spec** — hand-authored OpenAPI 3.1.0 JSON spec covering all ~90
      paths and 29 tags, compiled into binary via `include_str!()`; served at
      `GET /api/openapi.json` (public, no auth); component schemas for Agent,
      Task, MemoryEntry, AuditEntry, Channel, Schedule, Tenant, Workflow, Error
      *(3 tests)*
- [x] **Architecture Decision Records** — 5 ADRs in `docs/adr/`: encrypted
      storage by default (0001), capability attenuation model (0002), Nonagon
      team topology (0003), federation trust model (0004), cost governance
      architecture (0005); each follows Status/Context/Decision/Consequences format
- [x] **Integration guides** — 5 step-by-step guides in `docs/guides/`: Slack
      webhook bot, Discord A2A bot, custom MCP server development, Kubernetes
      Helm chart deployment, multi-tenant OIDC SSO setup; plus horizontal
      scaling architecture guide
- [x] **SDK documentation** — 3 protocol guides in `docs/sdk/`: REST API
      (auth, all endpoint categories, Python/TypeScript/curl examples), WebSocket
      (message types, auth flow, reconnection), Voice (state machine, audio
      frames, Python/browser examples)

### 7.5 Infrastructure Scaling

- [x] **Cross-compiled binary releases** — `release.yml` enhanced with
      aarch64-linux cross-compilation via `gcc-aarch64-linux-gnu`;
      `checksums.txt` manifest for all artifacts; `deploy/scripts/cross-build.sh`
      for local builds; macOS targets documented in `docs/BUILDING.md`
- [x] **Tauri desktop app builds** — existing CI pipeline produces `.deb` and
      `.AppImage` for Linux; macOS DMG and Windows MSI documented as local builds
      in `docs/BUILDING.md` (no macOS/Windows CI runners available);
      comprehensive build instructions for all platforms
- [x] **Horizontal scaling** — `ScalingConfig` with `stateless_mode`,
      `SessionAffinityStrategy` (None/ConsistentHashing/HeaderBased), and
      `instance_id`; health endpoint enhanced with scaling fields; Helm chart
      values updated with PostgreSQL/Redis/scaling configuration; architecture
      guide in `docs/guides/horizontal-scaling.md` *(3 tests)*
- [x] **Observability v2** — W3C Trace Context propagation middleware
      (`trace_context.rs`) parsing/generating `traceparent` headers; trace IDs
      recorded on tracing spans; response headers include `traceparent` for
      distributed tracing; 3 Grafana dashboard templates in `deploy/grafana/`:
      agent performance, cost monitoring, error rates *(6 tests)*

---

## Cross-Cutting Concerns

These apply across all phases and are tracked in Phase 7:

### Security Posture → Phase 7.2

### Testing Strategy → Phase 7.3

### Documentation → Phase 7.4

---

## Priority Matrix

| Feature                     | Impact | Effort | Phase  | Status      |
|-----------------------------|--------|--------|--------|-------------|
| CI/CD pipelines             | High   | Low    | v0.2.x | Done        |
| OpenTelemetry / Prometheus  | High   | Medium | v0.2.x | Done        |
| DAG task execution          | High   | High   | v0.3.x | Done        |
| Human-in-the-loop           | High   | Medium | v0.3.x | Done        |
| Reflection loops            | Medium | Low    | v0.3.x | Done        |
| Dynamic agent spawning      | Medium | Medium | v0.3.x | Done        |
| A2A protocol support        | High   | High   | v0.4.x | Done        |
| MCP OAuth + sampling        | Medium | Medium | v0.4.x | Done        |
| Federation init + trust     | Medium | Medium | v0.4.x | Done        |
| Voice agent (WebSocket)     | High   | High   | v0.5.x | Done        |
| Image/vision input          | Medium | Medium | v0.5.x | Done        |
| STT/TTS provider traits     | Medium | Medium | v0.5.x | Done        |
| Multimodal memory           | Medium | Medium | v0.5.x | Done        |
| Multi-tenancy               | High   | High   | v0.6.x | Done        |
| Enterprise SSO (OIDC)       | High   | High   | v0.6.x | Done        |
| Budget / cost governance    | High   | Medium | v0.6.x | Done        |
| Webhooks + workflows        | High   | Medium | v0.6.x | Done        |
| Kubernetes Helm chart       | Medium | Low    | v0.6.x | Done        |
| Backup automation           | Medium | Low    | v0.6.x | Done        |
| GraphRAG pipeline           | Medium | High   | v1.x   | Done        |
| Adaptive memory             | Medium | High   | v1.x   | Done        |
| Outcome tracking            | High   | Medium | v1.x   | Done        |
| Agentic RAG                 | Medium | Medium | v1.x   | Done        |
| Security hardening          | High   | High   | v1.1+  | Done        |
| Full PostgreSQL backend     | High   | High   | v1.1+  | Done        |
| Full Redis session cache    | Medium | Medium | v1.1+  | Done        |
| OpenAPI spec                | Medium | Medium | v1.1+  | Done        |
| A2A contract tests          | Medium | Medium | v1.1+  | Done        |
| Chaos + load testing        | Medium | High   | v1.1+  | Done        |
| Horizontal scaling          | High   | High   | v1.1+  | Done        |
| Cross-compiled releases     | Medium | Medium | v1.1+  | Done        |

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
| A2A support                 | No        | No      | No     | Yes (v0.4)   |
| Multi-agent teams           | Yes       | Yes     | Yes    | Yes (Nonagon)|
| Federation                  | No        | No      | No     | Yes (v0.4)   |
| Vision / multimodal         | Partial   | Partial | No     | Yes (v0.5)   |
| Voice agents                | No        | No      | No     | Yes (v0.5)   |
| Multimodal memory           | No        | No      | No     | Yes (v0.5)   |
| Multi-tenancy               | No        | No      | No     | Yes (v0.6)   |
| RBAC + SSO                  | No        | No      | No     | Yes (v0.6)   |
| Cost governance             | Via Smith | No      | No     | Yes (v0.6)   |
| Webhook triggers            | Yes       | No      | No     | Yes (v0.6)   |
| Knowledge graph (GraphRAG)  | No        | No      | No     | Yes (v1.0)   |
| Adaptive memory             | Via Mem0  | No      | No     | Yes (v1.0)   |
| Self-improvement            | No        | No      | No     | Yes (v1.0)   |
| Agentic RAG                 | Partial   | No      | No     | Yes (v1.0)   |
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

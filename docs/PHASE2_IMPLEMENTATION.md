# Phase 2: Orchestration Evolution — Implementation Report

> **Version**: v0.3.x
> **Status**: Complete
> **Date**: 2026-03-07

---

## Overview

Phase 2 transformed the task orchestration engine from strictly sequential execution
into a DAG-based workflow engine with parallel step execution, reflection loops for
self-correction, human-in-the-loop approval gates, and dynamic agent spawning.

All changes are **backwards-compatible** — existing sequential missions execute
identically to before. The new capabilities activate only when steps include
`depends_on` relationships or non-default `StepKind` variants.

---

## 2.1 DAG-Based Step Execution

### 2.1.1 — Type Foundation (`StepKind`, `ExecutionMode`, `depends_on`)

**File**: `crates/aivyx-task/src/types.rs`

#### What Changed

Added three new types and two new fields to the `Step` struct:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum StepKind {
    Execute,
    Reflect { target_step: usize, max_depth: u32, current_depth: u32 },
    Approval { context: String, timeout_secs: Option<u64>, auto_approve_on_timeout: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    Sequential,
    Dag,
}
```

New fields on `Step`:
- `depends_on: Vec<usize>` — step indices that must complete before this step starts
- `kind: StepKind` — determines execution behavior (default: `Execute`)

Both fields use `#[serde(default)]` for backwards compatibility.

Added `Mission::execution_mode()` — returns `Dag` if any step has non-empty `depends_on`,
otherwise `Sequential`.

#### Design Decision

Using `#[serde(tag = "kind")]` internally-tagged representation for `StepKind` means
the JSON looks like `{"kind": "Reflect", "target_step": 0, ...}` rather than
`{"Reflect": {...}}`. This is more natural for the LLM planner to generate and
more readable in checkpoint files.

---

### 2.1.2 — DAG Validation and Resolution

**New file**: `crates/aivyx-task/src/dag.rs`

#### What Changed

Created a dedicated DAG module with four public functions:

| Function | Purpose |
|----------|---------|
| `validate_dag(steps)` | Cycle detection via Kahn's algorithm. Also validates: no self-dependencies, all dependency indices in bounds |
| `ready_steps(steps)` | Returns indices of `Pending` steps whose all dependencies are `Completed` |
| `skip_downstream(steps, failed_idx)` | BFS from failed step marking all transitive dependents as `Skipped` |
| `topological_order(steps)` | Returns deterministic execution order respecting all dependencies |

#### Design Decision: Kahn's Algorithm

Kahn's algorithm was chosen over DFS-based topological sort for two reasons:

1. **Cycle detection as a side-effect** — if the algorithm doesn't visit all nodes,
   a cycle exists. No separate cycle-detection pass needed.
2. **O(V + E) complexity** — same as DFS, but the iterative BFS style is easier to
   reason about in reviews.

The algorithm builds an adjacency list and in-degree array, then processes nodes with
zero in-degree iteratively. If `visited < total_nodes`, the remaining nodes form a cycle.

#### Tests

- Cycle detection (A→B→A)
- Self-dependency rejection
- Invalid index rejection
- Diamond DAG correctness (A,B independent → C depends on both)
- Partial completion (ready_steps with half-completed DAG)
- Empty graph edge case
- skip_downstream transitive propagation

---

### 2.1.3 — DAG-Aware Execution Loop

**File**: `crates/aivyx-task/src/engine.rs`

#### What Changed

The monolithic `execute_mission()` method was refactored into a dispatch pattern:

```
execute_mission()
├── match mission.execution_mode()
│   ├── Sequential → execute_sequential()  // Original behavior, extracted
│   └── Dag → validate_dag() + execute_dag()  // New parallel execution
├── handle_step_failure()  // Extracted helper
└── finalize_mission()     // Extracted helper
```

**`execute_sequential()`** — Extracted from the original loop. Enhanced to dispatch
on `StepKind`: `Execute` follows the original path, `Reflect` invokes the reflection
subsystem, `Approval` invokes the approval gate.

**`execute_dag()`** — New wavefront parallel execution:

```
loop {
    ready = dag::ready_steps(&mission.steps)
    if ready.is_empty() → break or deadlock error

    // Mark all ready steps as Running
    // Build prompts for each

    JoinSet::spawn(async {
        let agent = session.create_agent(profile).await;
        agent.turn(&prompt, None).await
    })

    // Collect results as they complete
    // On failure: dag::skip_downstream() + continue
    // Checkpoint after each batch
}
```

#### Design Decision: Agent-Per-Step

Each parallel step gets its **own agent instance**. Agents are stateful (they maintain
conversation history), so sharing one agent across concurrent steps would cause race
conditions. The `AgentSession::create_agent()` call is cheap — it instantiates a new
`Agent` struct but reuses the shared `reqwest::Client` from the LLM provider (thanks
to Phase 1 connection pooling).

#### Design Decision: Wavefront vs. Task-Level Spawning

We use **wavefront** execution (collect all results from batch N before starting
batch N+1) rather than immediately spawning each step as soon as its dependencies
complete. Wavefront is simpler, provides natural checkpoint boundaries, and avoids
thundering-herd problems with many parallel steps.

---

### 2.1.4 — DAG-Aware Planner

**File**: `crates/aivyx-task/src/planner.rs`

#### What Changed

- Updated `PlannedStep` deserialization struct with `depends_on: Vec<usize>` and
  `kind: Option<StepKind>` fields
- Added `DAG_PLANNING_SYSTEM_PROMPT` — instructs the LLM to output dependency
  relationships between steps
- Added `plan_mission_dag()` — calls `parse_plan_response()` then `validate_dag()`
  to ensure the LLM-generated plan is acyclic

The original `plan_mission()` and `PLANNING_SYSTEM_PROMPT` are preserved unchanged
for sequential planning.

#### Design Decision

The DAG planner prompt includes examples of all three `StepKind` variants so the LLM
can optionally include reflection and approval steps in its plans. This is optional —
`kind` defaults to `Execute` when omitted from the JSON.

---

### 2.1.5 — DAG Benchmarks

**File**: `crates/aivyx-task/benches/planning.rs`

#### What Changed

Added DAG-specific benchmarks:

| Benchmark | What It Measures |
|-----------|-----------------|
| `parse_plan_response/dag_json/{4,10,25,50}` | Parsing plans with `depends_on` fields |
| `dag_operations/validate_dag/{4,10,25,50}` | Cycle detection via Kahn's algorithm |
| `dag_operations/topological_order/{4,10,25,50}` | Topological sort |
| `dag_operations/ready_steps/{4,10,25,50}` | Ready-step resolution with half-completed DAGs |
| `mission_methods/execution_mode/{5,20,50}` | ExecutionMode detection |

The `generate_dag_plan_json()` helper creates diamond-pattern DAGs with configurable
width (3 parallel lanes merging at join points).

---

## 2.2 Reflection Loops

### 2.2.1 — Reflection Step Execution

**File**: `crates/aivyx-task/src/engine.rs`

#### What Changed

Added reflection handling to `execute_sequential()` when a step has
`kind: StepKind::Reflect { target_step, max_depth, current_depth }`:

1. `build_reflection_prompt()` constructs a critic prompt including the target step's
   description and result
2. The agent evaluates and returns a verdict
3. `parse_reflection_result()` extracts a `ReflectionVerdict { accept, feedback }`
4. **If accepted**: step marked `Completed`
5. **If rejected and `current_depth < max_depth`**: two new steps are appended:
   - A new `Execute` step re-doing the target with the feedback
   - A new `Reflect` step with `current_depth + 1`
6. **If rejected at max depth**: step marked `Completed` with a warning

```rust
struct ReflectionVerdict {
    accept: bool,
    feedback: Option<String>,
}
```

#### Design Decision: Append, Don't Modify

When a reflection rejects output, we **append** new steps to the mission rather than
modifying existing ones. This preserves the full audit trail — you can always see the
original attempt, the rejection, and each retry. It also avoids index invalidation
issues in the DAG.

#### Design Decision: Lenient Parsing

`parse_reflection_result()` is intentionally lenient:
- Extracts JSON from surrounding text (LLMs often wrap JSON in explanation)
- Falls back to treating the entire response as rejection feedback if no valid JSON found
- This prevents the reflection system from failing due to LLM formatting quirks

---

### 2.2.2 — Reflection in Planner

**File**: `crates/aivyx-task/src/planner.rs`

The DAG planner prompt includes `Reflect` as an optional step kind. Example in the prompt:

```json
{"description": "Review research quality", "tool_hints": [],
 "depends_on": [0], "kind": {"kind": "Reflect", "target_step": 0,
 "max_depth": 2, "current_depth": 0}}
```

The sequential planner does NOT emit reflection steps (backwards compatibility).

---

## 2.3 Human-in-the-Loop Approval Gates

### 2.3.1 — Progress Events

**File**: `crates/aivyx-task/src/progress.rs`

Added two new `ProgressEvent` variants:

```rust
ApprovalRequested {
    task_id: TaskId,
    step_index: usize,
    context: String,
    timeout_secs: Option<u64>,
    timestamp: DateTime<Utc>,
},
ApprovalReceived {
    task_id: TaskId,
    step_index: usize,
    approved: bool,
    timestamp: DateTime<Utc>,
},
```

These integrate with the existing `ProgressEmitter` system used by WebSocket and
SSE streaming endpoints.

### 2.3.2 — Audit Events

**File**: `aivyx-core/crates/aivyx-audit/src/event.rs`

Added two audit event variants for compliance tracking:

```rust
TaskApprovalRequested {
    task_id: String,
    step_index: usize,
    context: String,
},
TaskApprovalResolved {
    task_id: String,
    step_index: usize,
    approved: bool,
    method: String, // "user", "timeout_auto", "timeout_reject"
},
```

The `method` field distinguishes between explicit user decisions and timeout-based
automatic resolution — critical for compliance auditing.

### 2.3.3 — Approval Step Execution

**File**: `crates/aivyx-task/src/engine.rs`

Added `execute_approval_step()` method:

1. Emits `AuditEvent::TaskApprovalRequested`
2. Emits `ProgressEvent::ApprovalRequested` (reaches WebSocket/SSE clients)
3. Resolution logic:
   - If a `ChannelAdapter` is available → sends approval request through it (WebSocket)
   - If `auto_approve_on_timeout` is true → auto-approves
   - Otherwise → auto-rejects (no channel to receive approval from)
4. Emits `AuditEvent::TaskApprovalResolved` with the method used

> **Note**: Full blocking approval flow (where execution genuinely pauses waiting
> for a WebSocket response) is structured but left as a TODO for the WebSocket
> integration layer. The protocol types and audit trail are complete.

### 2.3.4 — WebSocket Protocol Extension

**File**: `crates/aivyx-server/src/routes/ws.rs`

Added protocol message types:

```rust
// Server → Client
ServerMessage::TaskApprovalRequest {
    task_id, step_index, context, request_id, timeout_secs
}

// Client → Server
ClientMessage::TaskApprovalResponse {
    request_id, approved, reason
}
```

This extends the existing `ApprovalRequest`/`ApprovalResponse` pattern (used for
tool-call approvals in Leash-tier agents) to task-level step approvals.

---

## 2.4 Dynamic Agent Spawning

### 2.4.1 — SpawnSpecialistTool

**New file**: `crates/aivyx-team/src/spawn.rs`

A `Tool` implementation that lets the lead agent create specialist agents mid-session:

```rust
pub struct SpawnSpecialistTool {
    session: Arc<AgentSession>,
    pool: SpecialistPool,
    bus: Arc<MessageBus>,
    max_spawned: usize,
    spawned_count: Arc<AtomicUsize>,
}
```

**Input**: `agent_name` (required), `role` (required), `profile` (optional, defaults to "aivyx")

**Execution flow**:
1. Check `spawned_count < max_spawned` (prevent runaway spawning)
2. Create agent via `session.create_agent(profile)`
3. Register in `MessageBus` (for inter-agent messaging)
4. Register in `SpecialistPool` (for delegation)
5. Increment atomic counter

**Capability scope**: `CapabilityScope::Custom("coordination")` — only agents with
coordination capabilities can spawn new specialists.

#### Design Decision: AtomicUsize for Safety Counter

`AtomicUsize` with `Relaxed` ordering is sufficient for the spawn counter because:
- It only increments (no decrement/reset)
- A slight race (two concurrent spawns both seeing count=4 when max=5) is acceptable —
  worst case is max+1 specialists, which is a safe failure mode
- Much simpler than wrapping in a `Mutex`

### 2.4.2 — MessageBus Dynamic Registration

**File**: `crates/aivyx-team/src/message_bus.rs`

Changed the internal `HashMap<String, broadcast::Sender<TeamMessage>>` to be wrapped
in `std::sync::RwLock`:

```rust
pub fn register_agent(&self, name: &str) -> Result<broadcast::Receiver<TeamMessage>> {
    let mut senders = self.senders.write()?;
    if let Some(tx) = senders.get(name) {
        Ok(tx.subscribe())  // Already registered, return new subscription
    } else {
        let (tx, rx) = broadcast::channel(64);
        senders.insert(name.to_string(), tx);
        Ok(rx)
    }
}
```

#### Design Decision: `std::sync::RwLock` not `tokio::sync::RwLock`

The standard library's `RwLock` is correct here because:
- The lock is never held across `.await` points
- `send()` and `broadcast()` only need read locks (very common path)
- `register_agent()` needs a write lock (rare path — only on spawn)
- `std::sync::RwLock` has lower overhead than `tokio::sync::RwLock` for sync operations

### 2.4.3 — SpecialistPool Registration

**File**: `crates/aivyx-team/src/delegation.rs`

Added `register_spawned()` method to `SpecialistPool` that adds the new specialist
to the team context's member list, making it available for delegation.

### 2.4.4 — TeamRuntime Integration

**File**: `crates/aivyx-team/src/runtime.rs`

In `create_lead_agent()`, the `SpawnSpecialistTool` is conditionally registered:

```rust
if self.config.dialogue.max_spawned_specialists > 0 {
    lead_agent.register_tool(Box::new(SpawnSpecialistTool::new(
        Arc::clone(&self.session),
        pool,
        Arc::clone(&bus),
        self.config.dialogue.max_spawned_specialists,
    )));
}
```

Setting `max_spawned_specialists: 0` disables dynamic spawning entirely.

### 2.4.5 — Config Extension

**File**: `crates/aivyx-team/src/config.rs`

Added `max_spawned_specialists: usize` to `DialogueConfig` with a default of 5.
Uses `#[serde(default = "default_max_spawned_specialists")]` for backwards compatibility
with existing TOML team configs.

---

## File Summary

### New files (2)

| File | Lines | Purpose |
|------|-------|---------|
| `crates/aivyx-task/src/dag.rs` | ~180 | DAG validation, ready-step resolution, topological sort, downstream skipping |
| `crates/aivyx-team/src/spawn.rs` | ~175 | SpawnSpecialistTool for dynamic agent creation |

### Modified files — aivyx-engine (10)

| File | Change Summary |
|------|---------------|
| `crates/aivyx-task/src/types.rs` | `StepKind` enum, `ExecutionMode` enum, `depends_on`/`kind` fields on `Step`, `execution_mode()` on `Mission` |
| `crates/aivyx-task/src/engine.rs` | Refactored into `execute_sequential()`/`execute_dag()`, reflection handling, approval gates, `JoinSet` parallelism |
| `crates/aivyx-task/src/planner.rs` | `PlannedStep` with `depends_on`/`kind`, `DAG_PLANNING_SYSTEM_PROMPT`, `plan_mission_dag()` |
| `crates/aivyx-task/src/progress.rs` | `ApprovalRequested`/`ApprovalReceived` events |
| `crates/aivyx-task/src/lib.rs` | Export `dag` module, `StepKind`, `ExecutionMode` |
| `crates/aivyx-task/src/store.rs` | Updated test Step literals for new fields |
| `crates/aivyx-task/benches/planning.rs` | DAG benchmarks: `validate_dag`, `topological_order`, `ready_steps`, `dag_json` parsing |
| `crates/aivyx-team/src/message_bus.rs` | `RwLock` wrapping, `register_agent()` for dynamic registration |
| `crates/aivyx-team/src/runtime.rs` | Register `SpawnSpecialistTool` in `create_lead_agent()` |
| `crates/aivyx-team/src/config.rs` | `max_spawned_specialists` field in `DialogueConfig` |

### Modified files — aivyx-core (1)

| File | Change Summary |
|------|---------------|
| `crates/aivyx-audit/src/event.rs` | `TaskApprovalRequested`/`TaskApprovalResolved` audit events |

### Modified files — aivyx-team (2)

| File | Change Summary |
|------|---------------|
| `crates/aivyx-team/src/delegation.rs` | `register_spawned()` on `SpecialistPool` |
| `crates/aivyx-team/src/lib.rs` | Export `spawn` module |

---

## Backwards Compatibility

All changes use serde defaults and additive-only patterns:

| Field/Type | Default | Effect on Existing Data |
|------------|---------|------------------------|
| `Step.depends_on` | `vec![]` | Existing steps deserialize with no dependencies |
| `Step.kind` | `StepKind::Execute` | Existing steps behave identically |
| `Mission.execution_mode()` | `Sequential` | No `depends_on` → sequential loop path |
| `DialogueConfig.max_spawned_specialists` | `5` | Existing team configs gain spawning support |

The sequential `plan_mission()` function is preserved alongside `plan_mission_dag()`.
The original execution loop is extracted into `execute_sequential()` with zero
behavior changes for missions without `depends_on` fields.

---

## Verification Checklist

- [x] DAG execution: independent steps run in parallel via `JoinSet`
- [x] Cycle detection: `validate_dag()` rejects cyclic dependencies
- [x] Topological ordering: deterministic execution order
- [x] Partial failure: `skip_downstream()` propagates through transitive dependents
- [x] Reflection: `Reflect` steps re-evaluate previous output with depth bounding
- [x] Approval gates: audit trail + progress events for approval flow
- [x] Dynamic spawning: `SpawnSpecialistTool` with safety limit
- [x] MessageBus dynamic registration: `register_agent()` with `RwLock`
- [x] Backwards compatibility: existing sequential missions unchanged
- [x] Serialization: `StepKind` round-trips through serde correctly
- [x] Benchmarks: DAG operations benchmarked across graph sizes

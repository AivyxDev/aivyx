# ADR-0003: Nonagon Team Topology for Multi-Agent Collaboration

**Status**: Accepted
**Date**: 2026-03-07
**Deciders**: Aivyx Core Team

## Context

Multi-agent architectures occupy a spectrum from flat peer-to-peer topologies
(e.g., AutoGen, where agents message each other directly) to strict hierarchies
(e.g., CrewAI, where a manager assigns tasks to workers). Each has trade-offs:

- **Flat topologies** are flexible but suffer from coordination overhead, message
  storms, and difficulty attributing costs or audit responsibility.
- **Strict hierarchies** provide clear ownership but are rigid and may bottleneck
  on the manager agent.

Aivyx needed a topology that:

1. Supports structured workflows (predefined mission plans).
2. Allows dynamic specialist creation when unanticipated needs arise mid-session.
3. Maintains clear accountability and cost attribution.
4. Bounds complexity to prevent runaway agent sprawl.

## Decision

The Aivyx team topology is called **Nonagon**, reflecting its core constraint:
a lead agent coordinates up to **9 specialist agents**.

### Architecture

```
                    +-------------+
                    |  Lead Agent |
                    +------+------+
                           |
            +--------------+--------------+
            |              |              |
     +------+------+ +----+----+ +-------+-------+
     | Specialist 1| | Spec. 2 | |  ...  | Spec. 9|
     +-------------+ +---------+ +-------+--------+
```

### Core components

| Component           | Role                                                |
|---------------------|-----------------------------------------------------|
| `LeadAgent`         | Plans missions, delegates steps, aggregates results |
| `SpecialistPool`    | Manages the lifecycle of up to 9 specialist agents  |
| `MessageBus`        | Tokio broadcast channels for inter-agent messaging  |
| `MissionPlan`       | Sequential or DAG-based execution plan              |
| `StepKind`          | Enum: `Execute`, `Delegate`, `Reflect`, `Gate`      |

### Mission planning

The lead agent produces a `MissionPlan` consisting of ordered steps:

```
MissionPlan {
  steps: [
    Step { kind: Execute, agent: "lead", description: "Analyze requirements" },
    Step { kind: Delegate, agent: "coder", description: "Implement feature" },
    Step { kind: Reflect, agent: "lead", description: "Review code quality" },
    Step { kind: Gate, condition: "tests_pass", description: "Run test suite" },
    Step { kind: Delegate, agent: "documenter", description: "Write docs" },
  ]
}
```

### Delegation

- `DelegateTaskTool` sends a task to a named specialist with an attenuated
  `CapabilitySet` (see ADR-0002). The specialist processes the task and returns
  a result to the lead via the `MessageBus`.
- `SpawnSpecialistTool` creates an **ephemeral** specialist agent mid-session
  for needs that were not anticipated when the team was initially configured.
  Ephemeral specialists are destroyed at the end of the session.

### Communication

Inter-agent messaging uses tokio broadcast channels, providing:

- **Low latency**: in-process message passing, no network overhead.
- **Fan-out**: the lead can broadcast instructions to all specialists.
- **Backpressure**: bounded channel capacity prevents memory exhaustion.

Messages are typed (`AgentMessage` enum) and include sender ID, recipient ID
(or broadcast), and a payload (text, tool result, or control signal).

### Reflection loops

`StepKind::Reflect` steps allow the lead agent to evaluate the output of a
previous step against quality criteria before proceeding. This provides built-in
quality gates without requiring external review infrastructure.

### Why 9?

The limit of 9 specialists was chosen based on:

- **Cognitive load**: research on team coordination suggests 7 plus or minus 2 as
  the effective limit for coordinated groups.
- **Cost control**: each specialist consumes LLM tokens; bounding the count
  provides a predictable cost ceiling.
- **Diminishing returns**: in practice, tasks requiring more than 9 distinct
  specializations can be decomposed into sub-missions with their own teams.

## Consequences

### Positive

- **Clear responsibility hierarchy**: the lead agent owns the mission outcome,
  and each specialist owns its delegated step. This provides natural audit trails
  and cost attribution.
- **Bounded complexity**: the 9-specialist limit prevents unbounded agent sprawl
  and ensures predictable resource consumption.
- **Flexible composition**: teams can be configured statically (predefined
  specialists) or dynamically (ephemeral specialists spawned on demand).
- **Quality gates**: reflection steps provide built-in quality control without
  external infrastructure.

### Negative

- **Single point of failure**: the lead agent is critical; if it fails or
  produces a poor plan, the entire mission is compromised. Mitigation: the
  engine can restart the lead and resume from the last completed step.
- **9-specialist limit**: some workflows may genuinely require more than 9
  specialists. Mitigation: sub-missions can be delegated to nested teams.
- **Lead bottleneck**: all communication flows through the lead, which can
  become a throughput bottleneck. Mitigation: DAG-based plans allow parallel
  specialist execution.

### Trade-offs

- **Hierarchical over flat**: flat topologies offer more flexibility but at the
  cost of coordination complexity and difficulty with cost attribution. The
  hierarchical model provides natural audit trails where every action traces
  back to a delegating lead.
- **Broadcast channels over request-response**: broadcast is simpler to
  implement and allows the lead to observe all inter-specialist communication.
  The trade-off is that specialists receive messages they may not need (filtered
  by recipient ID).

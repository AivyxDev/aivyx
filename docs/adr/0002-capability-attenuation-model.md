# ADR-0002: Capability Attenuation for Tool Access Control

**Status**: Accepted
**Date**: 2026-03-07
**Deciders**: Aivyx Core Team

## Context

AI agents with unrestricted tool access pose significant security risks. The
OWASP LLM Top 10 identifies "Excessive Agency" as a critical vulnerability:
an agent that can execute arbitrary shell commands, read any file, or contact
any network host can be manipulated via prompt injection to perform unintended
actions.

Most agent frameworks address this with binary allow/deny lists (e.g., "this
agent can use the `shell` tool" or "this agent cannot use the `shell` tool").
This approach lacks granularity: an agent that needs to run `git status` should
not automatically be able to run `rm -rf /`.

Aivyx needed a permission model that is:

- **Fine-grained**: control at the level of specific commands, paths, and hosts.
- **Composable**: agents that delegate to other agents must be able to pass
  along a reduced set of permissions.
- **Non-escalating**: a delegated agent must never gain more permissions than
  its delegator.

## Decision

Aivyx implements **capability-based security with structural attenuation** in
the `aivyx-capability` crate.

### Capability types

Each agent is assigned a `CapabilitySet` containing zero or more typed
`Capability` values:

| Capability          | Parameters         | Example                           |
|---------------------|--------------------|-----------------------------------|
| `Shell(patterns)`   | Glob patterns      | `["git *", "cargo build"]`        |
| `Filesystem(paths)` | Path prefixes      | `["/home/user/project/**"]`       |
| `Network(hosts)`    | Host:port patterns  | `["api.github.com:443"]`         |
| `Custom(scope)`     | Arbitrary string   | `"database:read_only"`            |

### Attenuation

The `attenuate()` operation creates a new `CapabilitySet` that is a **subset**
of the original:

```
parent_caps = Shell(["git *", "cargo *"]) + Filesystem(["/project/**"])
child_caps  = parent_caps.attenuate(Shell(["git status", "git log"]))
            = Shell(["git status", "git log"])
```

Attenuation is enforced structurally: the `attenuate()` function intersects the
requested capabilities with the parent's capabilities and returns only the
overlap. There is no mechanism to add capabilities that the parent does not hold.

### Delegation chain

When a lead agent uses `DelegateTaskTool` or `SpawnSpecialistTool`, the
specialist receives an attenuated `CapabilitySet`. The chain is recorded for
audit:

```
Lead (full caps)
  --> Specialist A (attenuated: shell + filesystem)
        --> Sub-specialist A1 (attenuated: filesystem only)
```

### Abuse detection

The `AbuseDetector` component monitors tool usage in real time and flags
anomalous patterns:

- **High frequency**: more than N tool calls per minute.
- **Repeated denials**: an agent repeatedly attempts actions outside its
  capability set.
- **Scope escalation**: an agent attempts to use capabilities that were
  explicitly removed during attenuation.
- **Unusual patterns**: statistical deviation from the agent's baseline
  tool usage profile.

Detected abuse triggers configurable responses: logging, alerting, agent
suspension, or session termination.

## Consequences

### Positive

- **Principle of least privilege**: agents operate with the minimum permissions
  needed for their specific task, reducing the blast radius of prompt injection
  or misuse.
- **Delegation-safe**: specialist agents spawned by a lead inherit an attenuated
  subset of permissions and cannot escalate beyond what was granted.
- **Audit-friendly**: the full capability chain is recorded, making it clear
  exactly what each agent was authorized to do and what it actually did.
- **Composable**: capabilities can be combined, intersected, and narrowed without
  special-case logic.

### Negative

- **Upfront design required**: each agent profile must declare its capability
  requirements, which adds complexity to agent configuration.
- **Over-restriction risk**: overly narrow capabilities can cause legitimate
  tool calls to fail, requiring iterative tuning.
- **Runtime overhead**: every tool call is checked against the capability set,
  though this is a fast in-memory set intersection.

### Trade-offs

- **Structural attenuation over RBAC**: traditional Role-Based Access Control
  assigns permissions based on static roles. This works poorly for agent-to-agent
  delegation because the delegated agent may need a dynamic subset of the
  delegator's permissions. Structural attenuation provides non-monotonic
  permission narrowing that naturally follows the delegation chain.
- **Glob patterns over regex**: glob patterns are easier to read and write for
  common cases (file paths, command prefixes) at the cost of reduced
  expressiveness. Regex can be used in `Custom` capabilities for complex
  matching needs.

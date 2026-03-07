# ADR-0004: Federation Trust Model with Ed25519 Signatures

**Status**: Accepted
**Date**: 2026-03-07
**Deciders**: Aivyx Core Team

## Context

Aivyx instances need to collaborate across organizational boundaries. Example
scenarios include:

- A company's internal Aivyx instance delegating code review tasks to a
  partner's specialized code analysis instance.
- Multiple department-level instances sharing memory and tool outputs.
- A central orchestration instance distributing work across edge instances.

This cross-instance communication requires:

1. **Authentication**: each instance must verify the identity of the sender.
2. **Authorization**: each instance must control what remote instances are
   allowed to do.
3. **Resilience**: if one peer is unavailable, the system should failover to
   an alternative.
4. **Simplicity**: the protocol must work across firewalls and NATs without
   complex network configuration.

## Decision

Federation uses **Ed25519 digital signatures** for message authentication and
a **TrustPolicy** configuration for authorization.

### Identity and authentication

Each Aivyx instance generates an Ed25519 keypair on first startup:

```
FederationAuth::load_or_generate(key_path)
  --> Loads existing keypair from disk, or
  --> Generates a new keypair and persists it
```

- The **private key** is used to sign all outbound federation messages.
- The **public key** is shared with trusted peers via out-of-band exchange
  (e.g., configuration file, admin API, or key ceremony).

Every outbound message includes:

| Field        | Description                          |
|--------------|--------------------------------------|
| `instance_id`| Unique identifier for the sender    |
| `timestamp`  | Unix timestamp (for replay protection)|
| `payload`    | The message body                     |
| `signature`  | Ed25519 signature over the above     |

The receiving instance verifies the signature against the sender's known public
key and rejects messages with timestamps outside a configurable clock skew
window (default: 5 minutes).

### Authorization via TrustPolicy

Authentication alone is insufficient. The `TrustPolicy` defines what each
trusted peer is allowed to do:

```toml
[[federation.peers]]
instance_id = "partner-instance-01"
public_key = "base64-encoded-ed25519-public-key"
endpoint = "https://partner.example.com"

[federation.peers.trust_policy]
allowed_scopes = ["chat", "task"]
max_tier = 2
rate_limit_rpm = 60
```

| Field             | Description                                        |
|-------------------|----------------------------------------------------|
| `allowed_scopes`  | Which operations the peer may invoke (chat, task, memory) |
| `max_tier`        | Maximum model tier the peer may request (1=cheap, 3=premium) |
| `rate_limit_rpm`  | Maximum requests per minute from this peer          |

### Relay protocol

Federation uses **HTTP relay endpoints** rather than direct peer-to-peer
connections:

| Endpoint                       | Method | Description              |
|--------------------------------|--------|--------------------------|
| `/federation/relay/chat`       | POST   | Relay a chat request     |
| `/federation/relay/task`       | POST   | Relay a task request     |
| `/federation/health`           | GET    | Health check for peers   |

The relay approach was chosen over direct P2P because:

- It works behind firewalls and NATs without hole-punching.
- Standard HTTPS infrastructure (load balancers, TLS termination) applies.
- Existing monitoring and logging tools work without modification.

### Failover

`relay_chat_with_failover()` and `relay_task_with_failover()` accept an ordered
list of peer endpoints and automatically retry across healthy peers:

1. Attempt the primary peer.
2. On failure (timeout, 5xx, connection refused), mark the peer as unhealthy.
3. Retry with the next peer in the list.
4. Unhealthy peers are retried after a configurable backoff (default: 60 seconds).

Health status is tracked in memory with periodic `/federation/health` polling.

## Consequences

### Positive

- **Cryptographic authentication**: Ed25519 signatures provide strong identity
  guarantees without shared secrets. Each instance holds only its own private
  key and its peers' public keys.
- **Policy-based authorization**: `TrustPolicy` provides fine-grained control
  over what each peer can do, preventing unauthorized scope escalation.
- **Automatic failover**: multi-peer configurations provide resilience against
  individual instance failures.
- **Simple deployment**: HTTP relay works with standard web infrastructure and
  does not require special network configuration.

### Negative

- **Out-of-band key exchange**: public keys must be exchanged through a separate
  channel (configuration file, admin API). There is no automated key discovery
  protocol. This is a deliberate choice to avoid trust-on-first-use (TOFU)
  vulnerabilities.
- **No forward secrecy**: Ed25519 signatures authenticate messages but do not
  provide forward secrecy. If a private key is compromised, past messages
  (if captured) could be replayed within the clock skew window. Mitigation:
  use TLS for transport encryption with forward-secret cipher suites.
- **Clock dependency**: replay protection relies on synchronized clocks between
  instances. Significant clock skew can cause legitimate messages to be rejected.

### Trade-offs

- **Ed25519 over RSA**: Ed25519 keys are 32 bytes (vs. 256+ bytes for RSA-2048),
  signatures are faster to compute and verify, and the algorithm is resistant
  to timing side-channel attacks. The trade-off is that Ed25519 is not supported
  by some legacy systems, which is acceptable for a new protocol.
- **HTTP relay over direct P2P**: relay adds a small amount of latency (one
  extra HTTP hop) but dramatically simplifies deployment. Direct P2P would
  require NAT traversal (STUN/TURN), connection management, and custom
  protocol handling.
- **Static peer configuration over dynamic discovery**: static configuration is
  simpler and more auditable but requires manual updates when peers change.
  Dynamic discovery (e.g., DNS-SD) is a potential future enhancement.

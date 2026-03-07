# Horizontal Scaling Guide

This guide covers running multiple Aivyx Engine instances behind a load balancer
in **stateless mode**, where all persistent state is stored in external
PostgreSQL and Redis backends.

## Overview

By default, the Aivyx Engine stores data in local `redb` databases on disk.
This works well for single-instance deployments but prevents horizontal scaling
because each instance has its own isolated state.

When **stateless mode** is enabled (`AIVYX_STATELESS_MODE=1`), the engine
delegates all persistent storage to external PostgreSQL and Redis instances.
Local `redb` files are still used for ephemeral caches (session scratch data,
embedding vector indexes) but are not required to survive restarts.

This allows running multiple Aivyx Engine replicas behind a standard HTTP load
balancer with no shared filesystem.

## Prerequisites

- **PostgreSQL 15+** -- stores agents, teams, sessions, audit logs, memory,
  tenants, and billing data.
- **Redis 7+** -- used for distributed rate limiting, pub/sub event
  broadcasting, and short-lived session affinity state.
- A load balancer that supports WebSocket upgrade (e.g. NGINX Ingress, Envoy,
  AWS ALB).

## Configuration

### Environment Variables

| Variable | Description | Example |
|---|---|---|
| `AIVYX_STATELESS_MODE` | Enable stateless mode (`1` = on) | `1` |
| `AIVYX_INSTANCE_ID` | Unique ID for this instance (auto-generated if empty) | `node-1` |
| `DATABASE_URL` | PostgreSQL connection string | `postgres://aivyx:pw@pg:5432/aivyx` |
| `REDIS_URL` | Redis connection string | `redis://redis:6379/0` |

### Scaling Configuration Struct

The `ScalingConfig` struct in `crates/aivyx-server/src/scaling.rs` defines the
configuration programmatically:

```rust
ScalingConfig {
    stateless_mode: true,
    session_affinity: SessionAffinityStrategy::ConsistentHashing,
    instance_id: "node-1".into(),
}
```

Available session affinity strategies:

- `none` -- No affinity. Any instance handles any request.
- `consistent_hashing` -- The load balancer routes based on a hash of the
  session ID. Requires LB support for hash-based upstream selection.
- `header_based` -- The load balancer routes based on the `X-Aivyx-Instance-Id`
  header for sticky sessions.

## Helm Chart Configuration

### Multi-Replica Deployment

In `deploy/helm/aivyx-engine/values.yaml`:

```yaml
replicaCount: 3

autoscaling:
  enabled: true
  minReplicas: 2
  maxReplicas: 10
  targetCPUUtilizationPercentage: 80

env:
  AIVYX_STATELESS_MODE: "1"
  DATABASE_URL: "postgres://aivyx:password@postgres:5432/aivyx"
  REDIS_URL: "redis://redis:6379/0"
```

### External State Store Configuration

```yaml
scaling:
  statelessMode: true
  instanceId: ""  # Auto-generated if empty
  sessionAffinity: "consistent_hashing"

postgresql:
  url: "postgres://aivyx:password@postgres:5432/aivyx"
  maxConnections: 20
  minConnections: 2

redis:
  url: "redis://redis:6379/0"
  poolSize: 10
  ttlSeconds: 3600
```

## Session Affinity for WebSocket and Voice

WebSocket connections (`/ws`) and voice streaming connections (`/ws/voice`) are
long-lived and stateful. When a WebSocket connection is established, it is
pinned to the instance that accepted the upgrade.

If an instance restarts or is removed from the load balancer pool, all its
WebSocket connections are dropped. Clients must reconnect, and they may be
routed to a different instance.

### Consistent Hashing Strategy

With `consistent_hashing`, the load balancer hashes the session ID (from a
cookie or query parameter) and routes to a consistent upstream. This minimizes
connection disruption when scaling up or down.

Example NGINX Ingress annotation:

```yaml
nginx.ingress.kubernetes.io/upstream-hash-by: "$arg_session_id"
```

### Header-Based Strategy

With `header_based`, clients include an `X-Aivyx-Instance-Id` header and the
load balancer routes to the matching instance.

Example NGINX Ingress annotation:

```yaml
nginx.ingress.kubernetes.io/upstream-hash-by: "$http_x_aivyx_instance_id"
```

## Load Balancer Requirements

1. **WebSocket support** -- The LB must pass `Upgrade: websocket` headers and
   maintain the TCP connection.
2. **Health checks** -- Point the health check at `GET /health`. The endpoint
   returns `200 OK` with the instance ID and scaling mode in the response body.
3. **Graceful draining** -- When scaling down, drain connections before removing
   the instance. Kubernetes handles this with `preStop` hooks and
   `terminationGracePeriodSeconds`.
4. **TLS termination** -- Terminate TLS at the load balancer. The engine
   listens on plain HTTP internally.

## Monitoring with Grafana Dashboards

Three pre-built Grafana dashboards are provided in `deploy/grafana/`:

| Dashboard | File | Description |
|---|---|---|
| Agent Performance | `agent-performance.json` | Latency histograms, tool execution counts, token usage, WebSocket connections, request rates |
| Cost Monitoring | `cost-monitoring.json` | Daily cost trends, cost by agent, budget utilization, token usage by model, rate limit hits |
| Error Rates | `error-rates.json` | HTTP error rates by status code, errors by endpoint, rate limit rejections, chaos faults, federation failures |

### Importing Dashboards

1. Open Grafana and navigate to **Dashboards > Import**.
2. Upload the JSON file or paste its contents.
3. Select your Prometheus datasource when prompted.

### W3C Trace Context

All requests are tagged with a W3C `traceparent` header. If an incoming request
includes a `traceparent`, the engine propagates it. Otherwise, a new trace ID
is generated. The response always includes the `traceparent` header, enabling
distributed tracing across federated Aivyx instances.

The trace ID is recorded in the tracing span and can be correlated with logs
and Prometheus metrics for end-to-end request tracing.

## Architecture Diagram

```
                        +-------------------+
                        |   Load Balancer   |
                        | (NGINX / Envoy)   |
                        +--------+----------+
                                 |
              +------------------+------------------+
              |                  |                  |
     +--------v-------+ +-------v--------+ +-------v--------+
     | Aivyx Engine   | | Aivyx Engine   | | Aivyx Engine   |
     | instance-1     | | instance-2     | | instance-3     |
     | (stateless)    | | (stateless)    | | (stateless)    |
     +-------+--------+ +-------+--------+ +-------+--------+
             |                   |                  |
             +-------------------+------------------+
             |                                      |
     +-------v--------+                  +----------v-------+
     |  PostgreSQL    |                  |     Redis        |
     |  (persistent   |                  |  (rate limits,   |
     |   state)       |                  |   pub/sub)       |
     +----------------+                  +------------------+
```

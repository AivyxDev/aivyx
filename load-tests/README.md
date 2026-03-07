# Load Tests

k6 scripts for measuring throughput of hot HTTP endpoints.

## Prerequisites

Install k6: https://k6.io/docs/get-started/installation/

## Running

```bash
# Baseline — health endpoint
BASE_URL=http://localhost:3000 k6 run load-tests/health.js

# A2A Agent Card discovery (unauthenticated)
BASE_URL=http://localhost:3000 k6 run load-tests/agent_card.js

# A2A tasks/send (authenticated, ramp-up pattern)
BASE_URL=http://localhost:3000 BEARER_TOKEN=your-token k6 run load-tests/a2a_task_send.js
```

## CI

k6 is not included in CI (requires external binary). These scripts are
intended for local pre-release performance validation.

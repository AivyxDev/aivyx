# Changelog

All notable changes to aivyx-engine will be documented in this file.

## [0.1.0] — 2026-03-07

### 🎉 Initial Release

Private engine extracted from monorepo:

- `aivyx-engine-cli` — engine binary with server start, genesis, config
- `aivyx-server` — HTTP API (Axum), WebSocket, GCRA rate limiting
- `aivyx-task` — mission orchestration, 3-tier complexity classifier
- `aivyx-team` — multi-agent Nonagon team sessions
- `aivyx-integration-tests` — 121 integration tests
- `deploy/` — Dockerfile, docker-compose, CI scripts

### Infrastructure
- Depends on aivyx-core via Git SSH
- Proprietary license
- 121 tests passing

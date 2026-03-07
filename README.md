# Aivyx Engine

Private server, orchestration, and deployment infrastructure for the [Aivyx](https://aivyx-studio.com) ecosystem.

## Architecture

The engine extends the [aivyx-core](https://aivyx-gitea.cloud/AivyxDev/aivyx-core) shared crates with server-side functionality:

| Crate | Description |
|-------|-------------|
| `aivyx-engine-cli` | Engine binary — `aivyx server start`, genesis, config |
| `aivyx-server` | HTTP API (Axum), WebSocket, file uploads, rate limiting |
| `aivyx-task` | Mission orchestration, complexity classifier |
| `aivyx-team` | Multi-agent collaboration, team sessions |
| `aivyx-integration-tests` | Server-level test suite |

## Deployment

```bash
# Build Docker image
docker build -f deploy/engine/Dockerfile -t aivyx-engine:latest .

# Deploy to VPS1
docker compose -f deploy/engine/docker-compose.yml up -d
```

See [deploy/](deploy/) for Docker Compose, Traefik, and CI/CD scripts.

## Development

```bash
cargo build --workspace
cargo test --workspace
```

Depends on `aivyx-core` crates via git.

## License

Proprietary — internal use only.

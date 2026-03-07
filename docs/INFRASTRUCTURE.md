# Aivyx Engine — Infrastructure & Deployment

> Internal documentation for the private Aivyx Engine server.

## Architecture

The Engine extends [aivyx-core](https://aivyx-gitea.cloud/AivyxDev/aivyx-core) with server-side:

```
aivyx-core (shared crates via git)
    ↓
aivyx-engine
├── aivyx-engine-cli   — Binary: aivyx server start, genesis, config
├── aivyx-server       — HTTP API (Axum), WebSocket, rate limiting
├── aivyx-task         — Mission orchestration, complexity classifier
├── aivyx-team         — Multi-agent collaboration, Nonagon teams
└── aivyx-integration-tests — Server test suite
```

## VPS Infrastructure

| Host | IP | Role | SSH |
|------|-----|------|-----|
| VPS1 (engine.aivyx.ai) | 72.60.208.87 | **Production** | `ssh vps1` |
| VPS5 (gitea.aivyx.ai) | 148.230.102.223 | **Gitea + CI** | `ssh vps5` |

### SSH Access

```bash
# Production Engine
ssh vps1

# Gitea / CI Runner
ssh vps5
```

Uses unified deploy key (`~/.ssh/id_ed25519_aivyx_deploy`).

## Deployment

### Build & Deploy

```bash
# Build release binary
cargo build --release --bin aivyx

# Build Docker image
docker build -f deploy/engine/Dockerfile -t aivyx-engine:latest .

# Deploy to VPS1
ssh vps1 "cd /home/aivyx && docker compose pull && docker compose up -d"
```

### Docker Compose Stack (VPS1)

```yaml
services:
  engine:
    image: aivyx-engine:latest
    restart: unless-stopped
    volumes:
      - aivyx-data:/home/aivyx/.aivyx
    environment:
      - RUST_LOG=info

  traefik:
    image: traefik:v3
    ports:
      - "80:80"
      - "443:443"
```

No Postgres. No Redis. Engine uses embedded `redb` for all storage.

### CI Pipeline (Gitea Actions)

```
push to main → cargo test → docker build → push to registry → deploy to VPS1
```

Self-hosted runner on VPS5.

## Backup

```bash
# Create encrypted backup of aivyx data
ssh vps1 "aivyx backup /tmp/backup-$(date +%Y%m%d).tar.gz"
scp vps1:/tmp/backup-*.tar.gz ./backups/
```

## Monitoring

- Engine health: `curl https://api.aivyx.ai/health`
- Gitea health: `curl https://aivyx-gitea.cloud/api/v1/settings/api`

# Deployment Guide

> Step-by-step deployment of the Aivyx Engine to production.

## Prerequisites

- VPS with Docker and Docker Compose installed
- SSH access via `ssh vps1`
- Aivyx Engine Docker image built

## Build

### Local Binary

```bash
cargo build --release --bin aivyx
```

### Docker Image

```bash
docker build -f deploy/engine/Dockerfile -t aivyx-engine:latest .
```

## Deploy to VPS1

### 1. Push Docker Image

```bash
# Tag and push to Gitea container registry
docker tag aivyx-engine:latest aivyx-gitea.cloud/aivyxdev/aivyx-engine:latest
docker push aivyx-gitea.cloud/aivyxdev/aivyx-engine:latest
```

### 2. Pull and Start on VPS

```bash
ssh vps1 << 'EOF'
cd /home/aivyx
docker compose pull
docker compose up -d
docker compose logs -f engine --tail 20
EOF
```

### 3. Verify

```bash
# Health check
curl https://api.aivyx.ai/health

# Check logs
ssh vps1 "docker compose logs engine --tail 50"
```

## Docker Compose

```yaml
services:
  engine:
    image: aivyx-gitea.cloud/aivyxdev/aivyx-engine:latest
    restart: unless-stopped
    volumes:
      - aivyx-data:/home/aivyx/.aivyx
    environment:
      - RUST_LOG=info
    labels:
      - "traefik.enable=true"
      - "traefik.http.routers.engine.rule=Host(`api.aivyx.ai`)"
      - "traefik.http.routers.engine.tls.certresolver=letsencrypt"

  traefik:
    image: traefik:v3
    restart: unless-stopped
    ports:
      - "80:80"
      - "443:443"
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock:ro
      - traefik-certs:/letsencrypt

volumes:
  aivyx-data:
  traefik-certs:
```

## CI/CD Pipeline

The Gitea Actions workflow (`ci.yml`) automates:

```
push to main
    → cargo test --workspace
    → docker build
    → docker push to Gitea registry
    → SSH deploy to VPS1
```

## Rollback

```bash
ssh vps1 << 'EOF'
docker compose down engine
docker tag aivyx-engine:latest aivyx-engine:rollback
docker pull aivyx-gitea.cloud/aivyxdev/aivyx-engine:previous
docker tag aivyx-engine:previous aivyx-engine:latest
docker compose up -d engine
EOF
```

## Backup

```bash
# Create encrypted backup
ssh vps1 "aivyx backup /tmp/backup-$(date +%Y%m%d).tar.gz"

# Download locally  
scp vps1:/tmp/backup-*.tar.gz ./backups/
```

Schedule daily backups via cron on VPS1:

```bash
# /etc/cron.d/aivyx-backup
0 3 * * * aivyx /usr/local/bin/aivyx backup /home/aivyx/backups/backup-$(date +\%Y\%m\%d).tar.gz
```

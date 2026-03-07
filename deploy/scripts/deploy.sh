#!/bin/bash
set -euo pipefail

# ─── Deploy to Target VPS ───────────────────────────────────────────
# SSHes to a target server, pulls latest images, and restarts services.
#
# Usage: deploy.sh <ssh-alias> <service-name>
# Example: deploy.sh aivyx-engine engine
#          deploy.sh aivyx-studio website

SSH_HOST="${1:?Usage: deploy.sh <ssh-alias> <service-name>}"
SERVICE="${2:?Usage: deploy.sh <ssh-alias> <service-name>}"

echo "[deploy] Deploying $SERVICE to $SSH_HOST..."

ssh -o ConnectTimeout=10 -o StrictHostKeyChecking=no "$SSH_HOST" "
  cd /home/aivyx
  docker compose -f docker-compose.yml --env-file .env pull $SERVICE
  docker compose -f docker-compose.yml --env-file .env up -d $SERVICE
  echo 'Waiting for container...'
  sleep 10
  docker ps --format 'table {{.Names}}\t{{.Status}}' | head -10
"

echo "[deploy] Done ✅"

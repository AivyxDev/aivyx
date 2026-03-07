#!/bin/sh
# ─── Dashboard Entrypoint ──────────────────────────────────────────
# Reads the Bearer token from /run/secrets/bearer_token or env var
# and injects it into the Nginx config via envsubst.

set -eu

# Read bearer token from file (Docker secret) or environment variable
if [ -f /run/secrets/bearer_token ]; then
    BEARER_TOKEN=$(cat /run/secrets/bearer_token)
elif [ -n "${BEARER_TOKEN:-}" ]; then
    : # already set
else
    echo "[dashboard] WARNING: No BEARER_TOKEN set. API proxy will fail auth."
    BEARER_TOKEN="unset"
fi

export BEARER_TOKEN

# Substitute environment variables in nginx config
envsubst '$BEARER_TOKEN' < /etc/nginx/templates/default.conf.template > /etc/nginx/conf.d/default.conf

echo "[dashboard] Starting nginx (token: ${BEARER_TOKEN:0:8}...)"
exec nginx -g "daemon off;"

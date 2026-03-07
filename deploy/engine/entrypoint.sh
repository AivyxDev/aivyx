#!/bin/bash
set -euo pipefail

# ─── Aivyx Engine Docker Entrypoint ────────────────────────────────
# Handles first-boot initialization and server startup.
#
# Required environment variables:
#   AIVYX_PASSPHRASE  — master key passphrase (read by aivyx binary directly)
#   AIVYX_API_KEY     — LLM provider API key (or "ollama" for local)

: "${AIVYX_PASSPHRASE:?AIVYX_PASSPHRASE must be set}"

# Graceful shutdown: forward SIGTERM to the aivyx process
shutdown() {
    echo "[entrypoint] Received SIGTERM, shutting down..."
    kill -TERM "$AIVYX_PID" 2>/dev/null
    wait "$AIVYX_PID"
    echo "[entrypoint] Shutdown complete."
    exit 0
}
trap shutdown SIGTERM SIGINT

# First-boot: run genesis if config.toml doesn't exist yet
if [ ! -f "$HOME/.aivyx/config.toml" ]; then
    echo "[entrypoint] First boot — running genesis..."
    aivyx genesis --yes
    echo "[entrypoint] Genesis complete."
fi

# Auto-provision agent profiles from mounted volume (VPS5 overlay)
if [ -d "/opt/aivyx-agents" ]; then
    AGENTS_DIR="$HOME/.aivyx/agents"
    mkdir -p "$AGENTS_DIR"
    for profile in /opt/aivyx-agents/*.toml; do
        [ -f "$profile" ] || continue
        name=$(basename "$profile")
        if [ ! -f "$AGENTS_DIR/$name" ]; then
            cp "$profile" "$AGENTS_DIR/$name"
            echo "[entrypoint] Provisioned agent: $name"
        fi
    done
fi

# Start the server (reads AIVYX_PASSPHRASE from env automatically,
# auto-generates bearer token on first start, writes to ~/.aivyx/bearer-token)
echo "[entrypoint] Starting aivyx server on 0.0.0.0:8080..."
aivyx server start --bind 0.0.0.0 --port 8080 &
AIVYX_PID=$!

# Print bearer token if available
if [ -f "$HOME/.aivyx/bearer-token" ]; then
    echo "[entrypoint] Bearer token: $(cat "$HOME/.aivyx/bearer-token")"
fi

# Wait for the server process
wait "$AIVYX_PID"

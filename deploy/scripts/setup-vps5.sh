#!/bin/bash
set -euo pipefail

# ─── VPS5 Autonomous Agents Setup ──────────────────────────────────
# Provisions the Aivyx Engine on VPS5 with two autonomous agents:
#   1. business-admin — Business intelligence and admin tasks
#   2. devops         — Infrastructure monitoring and maintenance
#
# This script runs INSIDE the container (via docker exec) or on
# a host with aivyx CLI installed. It assumes genesis has already run.
#
# Usage: setup-vps5.sh
# ─────────────────────────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
AGENTS_DIR="${SCRIPT_DIR}/../engine/agents"

echo "═══════════════════════════════════════════════"
echo "  Aivyx VPS5 Autonomous Agents Setup"
echo "═══════════════════════════════════════════════"
echo

# ── Step 1: Copy agent profiles ─────────────────────────────────────
echo "[setup] Provisioning agent profiles..."

AIVYX_AGENTS_DIR="${HOME}/.aivyx/agents"
mkdir -p "$AIVYX_AGENTS_DIR"

for profile in "$AGENTS_DIR"/*.toml; do
    name=$(basename "$profile")
    if [ ! -f "$AIVYX_AGENTS_DIR/$name" ]; then
        cp "$profile" "$AIVYX_AGENTS_DIR/$name"
        echo "  ✓ Created agent: $name"
    else
        echo "  · Skipped (exists): $name"
    fi
done
echo

# ── Step 2: Add schedule entries ─────────────────────────────────────
echo "[setup] Configuring autonomous schedules..."

add_schedule() {
    local name="$1" cron="$2" agent="$3" prompt="$4"
    if aivyx schedule list 2>/dev/null | grep -q "$name"; then
        echo "  · Skipped (exists): $name"
    else
        aivyx schedule add --name "$name" --cron "$cron" --agent "$agent" --prompt "$prompt"
        echo "  ✓ Added schedule: $name"
    fi
}

# Business Admin schedules
add_schedule "biz-morning-digest" \
    "0 7 * * *" \
    "business-admin" \
    "Generate a concise morning digest. Check for any notable news or updates relevant to the Aivyx project. Summarize any system notifications from the past 24 hours. Be brief and actionable."

add_schedule "biz-project-status" \
    "0 12 * * 1-5" \
    "business-admin" \
    "Check the current status of active projects. Review recent git activity across repositories. Summarize progress, blockers, and upcoming deadlines. Keep it under 500 words."

add_schedule "biz-eod-summary" \
    "0 18 * * 1-5" \
    "business-admin" \
    "Generate an end-of-day summary. What was accomplished today? What needs attention tomorrow? List any outstanding action items. Be concise."

add_schedule "biz-weekly-review" \
    "0 9 * * 1" \
    "business-admin" \
    "Generate a weekly business review. Summarize the past week's activity, key metrics, and any trends. Identify priorities for the coming week. Include any infrastructure or DevOps highlights from the ops agent notifications."

# DevOps schedules
add_schedule "ops-health-check" \
    "*/30 * * * *" \
    "devops" \
    "Run a quick server health check. Check: disk usage (df -h), memory (free -h), CPU load (uptime), and Docker container status (docker ps --format 'table {{.Names}}\t{{.Status}}'). Report any anomalies. If everything is normal, respond with a one-line OK status."

add_schedule "ops-docker-status" \
    "0 */4 * * *" \
    "devops" \
    "Check all Docker containers: their status, restart counts, resource usage, and image versions. Flag any containers that have restarted recently or are unhealthy. Use: docker stats --no-stream --format 'table {{.Name}}\t{{.CPUPerc}}\t{{.MemUsage}}\t{{.NetIO}}'"

add_schedule "ops-disk-cleanup" \
    "0 3 * * *" \
    "devops" \
    "Perform routine disk maintenance. Check for: large log files (find /var/log -size +100M), old Docker images (docker image prune --filter 'until=168h' --force), and tmp files older than 7 days. Report what was cleaned and current disk usage."

add_schedule "ops-ssl-check" \
    "0 9 * * 1" \
    "devops" \
    "Check SSL certificate expiry for all configured domains. Use: echo | openssl s_client -connect DOMAIN:443 -servername DOMAIN 2>/dev/null | openssl x509 -noout -dates. Flag any certificates expiring within 30 days."

add_schedule "ops-ci-status" \
    "0 */2 * * *" \
    "devops" \
    "Check the CI/CD pipeline status. Look at recent workflow runs and report any failures. Check if the latest builds are passing. If there are failures, summarize the error."

add_schedule "ops-backup-verify" \
    "0 4 * * 0" \
    "devops" \
    "Verify backup integrity. Check that recent backups exist, are non-zero size, and are recent (within expected schedule). List backup files with sizes and dates. Flag any missing or suspicious backups."

echo
echo "═══════════════════════════════════════════════"
echo "  Setup complete!"
echo ""
echo "  Agents:    business-admin, devops"
echo "  Schedules: $(aivyx schedule list 2>/dev/null | grep -c 'yes\|no' || echo '10') entries"
echo ""
echo "  The server scheduler will pick these up"
echo "  automatically on the next 60-second tick."
echo "═══════════════════════════════════════════════"

#!/bin/bash
set -euo pipefail

# ─── Aivyx CI Script ───────────────────────────────────────────────
# Single-command deploy wrapper replacing manual tarball → SCP → build → deploy.
#
# Usage:
#   ci.sh --engine                    # Build + deploy engine container
#   ci.sh --website                   # Build + deploy website container
#   ci.sh --release v0.2.0-beta       # Create a versioned release
#   ci.sh --all                       # Engine + website
#   ci.sh --all --release v0.2.0-beta # Everything
#
# Prerequisites:
#   - SSH access to aivyx-gitea (VPS4) and aivyx-studio (VPS1)
#   - For --release: GITEA_TOKEN env var or ~/.config/aivyx/gitea-token
# ────────────────────────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BUILD_HOST="aivyx-gitea"

# ── Colors ──
RED='\033[0;31m'
GREEN='\033[0;32m'
AMBER='\033[0;33m'
CYAN='\033[0;36m'
RESET='\033[0m'

info()  { echo -e "${CYAN}[ci]${RESET} $1"; }
ok()    { echo -e "${GREEN}[ci] ✓${RESET} $1"; }
fail()  { echo -e "${RED}[ci] ✗${RESET} $1" >&2; exit 1; }

# ── Parse flags ──
DO_ENGINE=false
DO_WEBSITE=false
DO_RELEASE=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --engine)  DO_ENGINE=true; shift ;;
        --website) DO_WEBSITE=true; shift ;;
        --all)     DO_ENGINE=true; DO_WEBSITE=true; shift ;;
        --release) DO_RELEASE="$2"; shift 2 ;;
        *)         fail "Unknown flag: $1. Usage: ci.sh [--engine] [--website] [--all] [--release <version>]" ;;
    esac
done

if ! $DO_ENGINE && ! $DO_WEBSITE && [[ -z "$DO_RELEASE" ]]; then
    fail "Specify at least one of: --engine, --website, --all, --release <version>"
fi

# ── Upload source ──
upload_source() {
    info "Creating source tarball..."
    cd "$REPO_ROOT"

    tar czf /tmp/aivyx-ci-src.tar.gz \
        --exclude='.git' \
        --exclude='target' \
        --exclude='.idea' \
        --exclude='.vscode' \
        --exclude='node_modules' \
        .

    ok "Tarball: $(du -h /tmp/aivyx-ci-src.tar.gz | cut -f1)"

    info "Uploading to build host..."
    scp -o ConnectTimeout=10 /tmp/aivyx-ci-src.tar.gz \
        "${BUILD_HOST}:/home/aivyx/ci-build/aivyx-src.tar.gz" \
        || fail "SCP failed"
    ok "Source uploaded"
}

# ── Build + deploy engine ──
deploy_engine() {
    info "Building engine Docker image on ${BUILD_HOST}..."
    ssh -o ConnectTimeout=10 "$BUILD_HOST" \
        "bash /home/aivyx/ci-build/build-engine.sh /home/aivyx/ci-build/aivyx-src.tar.gz" \
        || fail "Engine build failed"
    ok "Engine image built and pushed"

    info "Deploying engine to VPS1..."
    bash "$SCRIPT_DIR/deploy.sh" aivyx-engine engine \
        || fail "Engine deploy failed"
    ok "Engine deployed"
}

# ── Build + deploy website ──
deploy_website() {
    info "Building website Docker image on ${BUILD_HOST}..."
    ssh -o ConnectTimeout=10 "$BUILD_HOST" \
        "bash /home/aivyx/ci-build/build-website.sh /home/aivyx/ci-build/aivyx-src.tar.gz" \
        || fail "Website build failed"
    ok "Website image built and pushed"

    info "Deploying website to VPS1..."
    bash "$SCRIPT_DIR/deploy.sh" aivyx-studio website \
        || fail "Website deploy failed"
    ok "Website deployed"
}

# ── Main ──
main() {
    local start_time
    start_time=$(date +%s)

    echo ""
    echo -e "  ${CYAN}🕯️  Aivyx CI${RESET}"
    echo "  ──────────"
    $DO_ENGINE && echo "  • Engine:  build + deploy"
    $DO_WEBSITE && echo "  • Website: build + deploy"
    [[ -n "$DO_RELEASE" ]] && echo "  • Release: $DO_RELEASE"
    echo ""

    # Push any unpushed commits
    cd "$REPO_ROOT"
    if [[ -n "$(git log origin/main..HEAD --oneline 2>/dev/null)" ]]; then
        info "Pushing unpushed commits..."
        git push origin main || fail "Push failed"
        ok "Pushed to origin"
    fi

    # Upload source (needed for both engine and website builds)
    if $DO_ENGINE || $DO_WEBSITE; then
        upload_source
    fi

    # Build and deploy
    $DO_ENGINE && deploy_engine
    $DO_WEBSITE && deploy_website

    # Release
    if [[ -n "$DO_RELEASE" ]]; then
        info "Running release script..."
        bash "$SCRIPT_DIR/release.sh" "$DO_RELEASE"
    fi

    local elapsed=$(( $(date +%s) - start_time ))
    echo ""
    ok "CI complete in ${elapsed}s 🎉"
    echo ""

    # Cleanup
    rm -f /tmp/aivyx-ci-src.tar.gz
}

main

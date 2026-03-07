#!/bin/bash
set -euo pipefail

# ─── Aivyx Release Script ──────────────────────────────────────────
# Creates a Gitea release with pre-built binary tarballs.
#
# Usage:
#   release.sh <version>                 # e.g., release.sh v0.2.0-beta
#   release.sh <version> --dry-run       # Build only, skip upload
#
# Prerequisites:
#   - GITEA_TOKEN env var (or ~/.config/aivyx/gitea-token)
#   - SSH access to aivyx-gitea (VPS4 build host)
#   - Source already pushed to Gitea
#
# Output:
#   - aivyx-linux-x86_64.tar.gz      (built on VPS4)
#   - aivyx-linux-x86_64.tar.gz.sha256
#   - Gitea release created with assets attached
# ────────────────────────────────────────────────────────────────────

REGISTRY="aivyx-gitea.cloud"
OWNER="aivyxdev"
REPO="aivyx"
API_URL="https://${REGISTRY}/api/v1"
BUILD_HOST="aivyx-gitea"  # SSH alias for VPS4
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# ── Colors ──
RED='\033[0;31m'
GREEN='\033[0;32m'
AMBER='\033[0;33m'
CYAN='\033[0;36m'
RESET='\033[0m'

info()  { echo -e "${CYAN}[release]${RESET} $1"; }
ok()    { echo -e "${GREEN}[release] ✓${RESET} $1"; }
warn()  { echo -e "${AMBER}[release] ⚠${RESET} $1"; }
fail()  { echo -e "${RED}[release] ✗${RESET} $1" >&2; exit 1; }

# ── Parse args ──
VERSION="${1:?Usage: release.sh <version> [--dry-run]}"
DRY_RUN=false
[[ "${2:-}" == "--dry-run" ]] && DRY_RUN=true

# Strip leading 'v' for tag comparisons but keep it for display
TAG="$VERSION"
[[ "$TAG" != v* ]] && TAG="v$TAG"

# ── Resolve token ──
resolve_token() {
    if [[ -n "${GITEA_TOKEN:-}" ]]; then
        echo "$GITEA_TOKEN"
    elif [[ -f "$HOME/.config/aivyx/gitea-token" ]]; then
        cat "$HOME/.config/aivyx/gitea-token"
    else
        fail "No Gitea token found. Set GITEA_TOKEN or create ~/.config/aivyx/gitea-token"
    fi
}

TOKEN="$(resolve_token)"

# ── Verify we're on a clean, pushed state ──
verify_repo() {
    cd "$REPO_ROOT"

    if [[ -n "$(git status --porcelain -- ':!.idea' ':!.vscode')" ]]; then
        fail "Working tree is dirty. Commit or stash changes first."
    fi

    local local_sha remote_sha
    local_sha="$(git rev-parse HEAD)"
    remote_sha="$(git ls-remote origin HEAD 2>/dev/null | awk '{print $1}')"

    if [[ "$local_sha" != "$remote_sha" ]]; then
        warn "Local HEAD ($local_sha) differs from remote ($remote_sha)"
        info "Pushing to origin..."
        git push origin main || fail "Push failed"
        ok "Pushed to origin"
    fi
}

# ── Create source tarball and build on VPS4 ──
build_binary() {
    info "Creating source tarball..."
    cd "$REPO_ROOT"

    local tarball="/tmp/aivyx-release-src.tar.gz"
    tar czf "$tarball" \
        --exclude='.git' \
        --exclude='target' \
        --exclude='.idea' \
        --exclude='.vscode' \
        --exclude='node_modules' \
        .

    local size
    size="$(du -h "$tarball" | cut -f1)"
    ok "Source tarball: $size"

    info "Uploading to build host..."
    scp -o ConnectTimeout=10 "$tarball" "${BUILD_HOST}:/home/aivyx/ci-build/aivyx-release-src.tar.gz" \
        || fail "SCP failed"
    ok "Uploaded to ${BUILD_HOST}"

    info "Building release binary on ${BUILD_HOST} via Docker (this takes ~2 min)..."
    ssh -o ConnectTimeout=10 "$BUILD_HOST" bash -s <<'REMOTE_BUILD'
set -euo pipefail
BUILD_DIR="/home/aivyx/ci-build/release-$$"
mkdir -p "$BUILD_DIR"
tar xzf /home/aivyx/ci-build/aivyx-release-src.tar.gz -C "$BUILD_DIR"

# Build inside Docker container (same image as engine Dockerfile)
docker run --rm \
    -v "$BUILD_DIR:/build" \
    -w /build \
    rust:bookworm \
    bash -c "cargo build --release --package aivyx-cli && strip target/release/aivyx"

# Create tarball with just the binary + docs
DIST_DIR="/home/aivyx/ci-build/dist"
rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR/aivyx"
cp "$BUILD_DIR/target/release/aivyx" "$DIST_DIR/aivyx/"
cp "$BUILD_DIR/LICENSE" "$DIST_DIR/aivyx/" 2>/dev/null || true
cp "$BUILD_DIR/README.md" "$DIST_DIR/aivyx/" 2>/dev/null || true

cd "$DIST_DIR"
tar czf /home/aivyx/ci-build/aivyx-linux-x86_64.tar.gz aivyx/
sha256sum /home/aivyx/ci-build/aivyx-linux-x86_64.tar.gz | awk '{print $1}' > /home/aivyx/ci-build/aivyx-linux-x86_64.tar.gz.sha256

# Report results
ls -lh /home/aivyx/ci-build/aivyx-linux-x86_64.tar.gz
cat /home/aivyx/ci-build/aivyx-linux-x86_64.tar.gz.sha256

# Cleanup build dir
rm -rf "$BUILD_DIR" "$DIST_DIR"
REMOTE_BUILD

    ok "Binary built and packaged"

    # Download artifacts
    info "Downloading release artifacts..."
    mkdir -p /tmp/aivyx-release
    scp -o ConnectTimeout=10 \
        "${BUILD_HOST}:/home/aivyx/ci-build/aivyx-linux-x86_64.tar.gz" \
        "${BUILD_HOST}:/home/aivyx/ci-build/aivyx-linux-x86_64.tar.gz.sha256" \
        /tmp/aivyx-release/

    ok "Downloaded: aivyx-linux-x86_64.tar.gz ($(du -h /tmp/aivyx-release/aivyx-linux-x86_64.tar.gz | cut -f1))"
    ok "SHA256: $(cat /tmp/aivyx-release/aivyx-linux-x86_64.tar.gz.sha256)"
}

# ── Create git tag ──
create_tag() {
    cd "$REPO_ROOT"

    if git rev-parse "$TAG" >/dev/null 2>&1; then
        warn "Tag $TAG already exists — skipping tag creation"
    else
        info "Creating tag $TAG..."
        git tag -a "$TAG" -m "Release $TAG"
        git push origin "$TAG" || fail "Failed to push tag"
        ok "Tag $TAG created and pushed"
    fi
}

# ── Create Gitea release via API ──
create_release() {
    if $DRY_RUN; then
        warn "Dry run — skipping Gitea release creation"
        return
    fi

    info "Creating Gitea release $TAG..."

    local release_body
    release_body="## Aivyx $TAG

**Public Beta Release**

### Install

\`\`\`bash
curl -fsSL https://aivyx-studio.com/install.sh | bash
\`\`\`

Or download the binary directly from the assets below.

### Verify

\`\`\`bash
sha256sum -c aivyx-linux-x86_64.tar.gz.sha256
tar xzf aivyx-linux-x86_64.tar.gz
./aivyx/aivyx --version
\`\`\`

### What's New

See [CHANGELOG.md](https://${REGISTRY}/${OWNER}/${REPO}/src/branch/main/docs/CHANGELOG.md) for the full list of changes."

    # Create the release
    local response
    response=$(curl -s -X POST \
        -H "Authorization: token $TOKEN" \
        -H "Content-Type: application/json" \
        "${API_URL}/repos/${OWNER}/${REPO}/releases" \
        -d "$(python3 -c "
import json, sys
print(json.dumps({
    'tag_name': '$TAG',
    'name': 'Aivyx $TAG',
    'body': '''$release_body''',
    'draft': False,
    'prerelease': True
}))
")")

    local release_id
    release_id=$(echo "$response" | python3 -c "import json,sys; print(json.load(sys.stdin).get('id',''))" 2>/dev/null)

    if [[ -z "$release_id" || "$release_id" == "None" ]]; then
        echo "$response"
        fail "Failed to create release"
    fi

    ok "Release created (id: $release_id)"

    # Upload assets
    info "Uploading release assets..."

    for asset in aivyx-linux-x86_64.tar.gz aivyx-linux-x86_64.tar.gz.sha256; do
        curl -s -X POST \
            -H "Authorization: token $TOKEN" \
            -H "Content-Type: application/octet-stream" \
            "${API_URL}/repos/${OWNER}/${REPO}/releases/${release_id}/assets?name=${asset}" \
            --data-binary @"/tmp/aivyx-release/${asset}" \
            > /dev/null

        ok "Uploaded: $asset"
    done
}

# ── Main ──
main() {
    echo ""
    echo -e "  ${CYAN}🕯️  Aivyx Release: $TAG${RESET}"
    echo "  ────────────────────────────"
    echo ""
    $DRY_RUN && warn "DRY RUN — no uploads will be performed"

    verify_repo
    build_binary
    create_tag
    create_release

    echo ""
    ok "Release $TAG complete! 🎉"
    echo ""
    echo "  📦 Gitea:    https://${REGISTRY}/${OWNER}/${REPO}/releases/tag/$TAG"
    echo "  🌐 Website:  https://aivyx-studio.com/download"
    echo "  📝 Changes:  https://${REGISTRY}/${OWNER}/${REPO}/src/branch/main/docs/CHANGELOG.md"
    echo ""

    # Cleanup
    rm -rf /tmp/aivyx-release /tmp/aivyx-release-src.tar.gz
}

main

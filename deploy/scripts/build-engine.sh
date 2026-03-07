#!/bin/bash
set -euo pipefail

# ─── Build Engine Docker Image ──────────────────────────────────────
# Runs on VPS4 (build host). Expects source tarball as argument.
#
# Usage: build-engine.sh <source-tarball>
# Output: aivyx-gitea.cloud/aivyxdev/aivyx-engine:latest pushed to registry

REGISTRY="aivyx-gitea.cloud/aivyxdev"
IMAGE="${REGISTRY}/aivyx-engine:latest"
BUILD_DIR="/home/aivyx/ci-build/engine-$$"

TARBALL="${1:?Usage: build-engine.sh <source-tarball>}"

echo "[build-engine] Extracting source..."
mkdir -p "$BUILD_DIR"
tar xzf "$TARBALL" -C "$BUILD_DIR"
chmod +x "$BUILD_DIR/deploy/engine/entrypoint.sh"

echo "[build-engine] Building image: $IMAGE"
docker build --no-cache \
  -f "$BUILD_DIR/deploy/engine/Dockerfile" \
  -t "$IMAGE" \
  "$BUILD_DIR"

echo "[build-engine] Pushing to registry..."
docker push "$IMAGE"

echo "[build-engine] Cleaning up..."
rm -rf "$BUILD_DIR"

echo "[build-engine] Done ✅"

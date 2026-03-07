#!/bin/bash
set -euo pipefail

# ─── Build Website Docker Image ─────────────────────────────────────
# Runs on VPS4 (build host). Expects source tarball as argument.
#
# Usage: build-website.sh <source-tarball>
# Output: aivyx-gitea.cloud/aivyxdev/aivyx-website:latest pushed to registry

REGISTRY="aivyx-gitea.cloud/aivyxdev"
IMAGE="${REGISTRY}/aivyx-website:latest"
BUILD_DIR="/home/aivyx/ci-build/website-$$"

TARBALL="${1:?Usage: build-website.sh <source-tarball>}"

echo "[build-website] Extracting source..."
mkdir -p "$BUILD_DIR"
tar xzf "$TARBALL" -C "$BUILD_DIR"

echo "[build-website] Building image: $IMAGE"
docker build \
  -f "$BUILD_DIR/deploy/website/Dockerfile" \
  -t "$IMAGE" \
  "$BUILD_DIR"

echo "[build-website] Pushing to registry..."
docker push "$IMAGE"

echo "[build-website] Cleaning up..."
rm -rf "$BUILD_DIR"

echo "[build-website] Done ✅"

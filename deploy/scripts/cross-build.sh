#!/usr/bin/env bash
# Cross-compile Aivyx Engine for multiple targets.
#
# Usage:
#   ./deploy/scripts/cross-build.sh [TARGET...]
#
# Supported targets:
#   x86_64-linux    (default)
#   aarch64-linux
#   x86_64-darwin   (requires macOS or cross-compilation toolchain)
#   aarch64-darwin  (requires macOS with Apple Silicon or Rosetta)
#
# Examples:
#   ./deploy/scripts/cross-build.sh                    # x86_64-linux only
#   ./deploy/scripts/cross-build.sh aarch64-linux      # ARM64 Linux
#   ./deploy/scripts/cross-build.sh x86_64-linux aarch64-linux  # both Linux

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
VERSION="${VERSION:-$(git -C "$REPO_ROOT" describe --tags --always 2>/dev/null || echo dev)}"
OUT_DIR="${OUT_DIR:-$REPO_ROOT/dist}"

mkdir -p "$OUT_DIR"

TARGETS=("${@:-x86_64-linux}")

for target in "${TARGETS[@]}"; do
  case "$target" in
    x86_64-linux)
      RUST_TARGET="x86_64-unknown-linux-gnu"
      BINARY_NAME="aivyx-linux-x86_64"
      ;;
    aarch64-linux)
      RUST_TARGET="aarch64-unknown-linux-gnu"
      BINARY_NAME="aivyx-linux-aarch64"
      export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
      ;;
    x86_64-darwin)
      RUST_TARGET="x86_64-apple-darwin"
      BINARY_NAME="aivyx-darwin-x86_64"
      ;;
    aarch64-darwin)
      RUST_TARGET="aarch64-apple-darwin"
      BINARY_NAME="aivyx-darwin-aarch64"
      ;;
    *)
      echo "Unknown target: $target"
      echo "Supported: x86_64-linux, aarch64-linux, x86_64-darwin, aarch64-darwin"
      exit 1
      ;;
  esac

  echo "=== Building $RUST_TARGET ==="
  rustup target add "$RUST_TARGET" 2>/dev/null || true

  cargo build --release --target "$RUST_TARGET" -p aivyx-engine-cli \
    --manifest-path "$REPO_ROOT/Cargo.toml"

  cp "$REPO_ROOT/target/$RUST_TARGET/release/aivyx" "$OUT_DIR/$BINARY_NAME"

  cd "$OUT_DIR"
  tar czf "${BINARY_NAME}.tar.gz" "$BINARY_NAME"
  sha256sum "${BINARY_NAME}.tar.gz" > "${BINARY_NAME}.tar.gz.sha256"
  rm "$BINARY_NAME"
  cd "$REPO_ROOT"

  echo "✓ $target → $OUT_DIR/${BINARY_NAME}.tar.gz"
done

echo ""
echo "=== Build Summary ==="
ls -lh "$OUT_DIR"/*.tar.gz 2>/dev/null
echo ""
echo "Checksums:"
cat "$OUT_DIR"/*.sha256 2>/dev/null

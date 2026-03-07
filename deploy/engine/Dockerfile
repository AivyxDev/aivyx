# syntax=docker/dockerfile:1
# ─── Stage 1: Build the aivyx binary ────────────────────────────────
FROM rust:bookworm AS builder

RUN apt-get update && apt-get install -y openssh-client && rm -rf /var/lib/apt/lists/*
RUN mkdir -p /root/.ssh && \
    ssh-keyscan -p 2222 aivyx-gitea.cloud >> /root/.ssh/known_hosts 2>/dev/null
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true

WORKDIR /build

# Copy workspace manifests first for layer caching
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates/ crates/

# Build only the CLI binary (uses BuildKit SSH mount for git deps)
RUN --mount=type=ssh cargo build --release --package aivyx-engine-cli \
    && strip target/release/aivyx

# ─── Stage 2: Minimal runtime ──────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user and pre-create data directory for volume mount
RUN useradd --create-home --shell /bin/bash aivyx \
    && mkdir -p /home/aivyx/.aivyx \
    && chown -R aivyx:aivyx /home/aivyx/.aivyx

USER aivyx
WORKDIR /home/aivyx

# Copy the binary and entrypoint
COPY --from=builder /build/target/release/aivyx /usr/local/bin/aivyx
COPY deploy/engine/entrypoint.sh /usr/local/bin/entrypoint.sh

EXPOSE 8080

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]

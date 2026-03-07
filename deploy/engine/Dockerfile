# syntax=docker/dockerfile:1
# ─── Stage 1: Build the aivyx binary ────────────────────────────────
FROM rust:bookworm AS builder

RUN apt-get update && apt-get install -y openssh-client git && rm -rf /var/lib/apt/lists/*
RUN mkdir -p /root/.ssh && \
    ssh-keyscan -p 2222 aivyx-gitea.cloud >> /root/.ssh/known_hosts 2>/dev/null
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true

WORKDIR /build

# Copy workspace manifests first for layer caching
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates/ crates/

# Clone aivyx-core as sibling for [patch] path dependencies
ARG SSH_KEY=""
RUN if [ -n "$SSH_KEY" ]; then \
    echo "$SSH_KEY" > /root/.ssh/id_ed25519 && \
    chmod 600 /root/.ssh/id_ed25519; \
    fi && \
    git clone --depth 1 ssh://git@aivyx-gitea.cloud:2222/AivyxDev/aivyx-core.git /aivyx-core && \
    cargo build --release --package aivyx-engine-cli && \
    strip target/release/aivyx && \
    rm -f /root/.ssh/id_ed25519

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

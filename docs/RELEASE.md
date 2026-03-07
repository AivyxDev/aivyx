# Release Process

How to create and publish a new Aivyx release with pre-built desktop packages.

## Overview

```
Push tag → Gitea Actions CI → Docker build → Artifacts on VPS2 → Live downloads
```

The release pipeline is fully automated via `.gitea/workflows/release.yml`.

## Quick Release

```bash
# From aivyx-engine repo
cd ~/Projects/aivyx-engine

# 1. Ensure everything is committed and pushed
git push origin main

# 2. Create and push a version tag
git tag -a v0.3.0 -m "Release v0.3.0"
git push origin v0.3.0
```

That's it. The CI pipeline handles the rest:

| Job | What it does |
|-----|-------------|
| `build` | Docker build on VPS4: engine binary + Tauri desktop app (.AppImage, .deb) |
| `deploy-releases` | SCP artifacts to VPS2 `/home/aivyx/releases/{version}/` and `/latest/` |
| `deploy-engine` | Update engine binary on VPS1 |
| `deploy-website` | Rebuild and deploy the website |

## What Gets Built

| Artifact | Description | Path on VPS2 |
|----------|-------------|-------------|
| `aivyx-desktop.AppImage` | Portable Linux desktop app | `/home/aivyx/releases/latest/` |
| `aivyx-desktop.deb` | Debian/Ubuntu package | `/home/aivyx/releases/latest/` |
| `aivyx-linux-x86_64.tar.gz` | Pre-built CLI binary | `/home/aivyx/releases/latest/` |
| `aivyx-linux-x86_64.tar.gz.sha256` | SHA256 checksum | `/home/aivyx/releases/latest/` |
| `aivyx-desktop-linux-x86_64` | Standalone desktop binary | `/home/aivyx/releases/latest/` |

## Download URLs

All artifacts are served at:
```
https://aivyx-studio.com/releases/latest/<filename>
https://aivyx-studio.com/releases/<version>/<filename>
```

## Build Infrastructure

| Component | Location | Purpose |
|-----------|----------|---------|
| `release.yml` | `.gitea/workflows/release.yml` | CI workflow definition |
| `Dockerfile.build` | `deploy/desktop/Dockerfile.build` | Ubuntu 22.04 build container |
| `build-desktop.sh` | `apps/desktop/build-desktop.sh` | Local desktop build script |
| `release.sh` | `deploy/scripts/release.sh` | Manual release script (alternative) |
| Gitea Runner | VPS4 (`aivyx-gitea`) | Self-hosted act_runner |
| Website VPS | VPS2 (`aivyx-studio`) | Serves downloads via Nginx |

## Nginx Configuration

VPS2 serves `/releases/` via an Nginx location block:
```nginx
location /releases/ {
    alias /releases/;
    autoindex on;
    add_header Content-Disposition "attachment";
}
```

This is mounted as a read-only Docker volume from `/home/aivyx/releases` on the host.

## Docker Build Details

The `Dockerfile.build` (Ubuntu 22.04):
1. Installs Rust, Node.js 22, WebKitGTK, and build dependencies
2. Receives the CI runner's SSH key via `SSH_PRIVATE_KEY` build arg
3. Adds Gitea SSH host key via `ssh-keyscan`
4. Builds `aivyx-cli` binary (release profile)
5. Copies binary as Tauri sidecar
6. Builds frontend (`npm ci`)
7. Builds Tauri desktop app (`npx tauri build`)
8. Collects `.deb`, `.AppImage`, CLI tarball, and standalone binary

## Manual Release (Alternative)

If CI is down, use the manual release script:
```bash
cd ~/Projects/aivyx-engine
./deploy/scripts/release.sh v0.3.0
./deploy/scripts/release.sh v0.3.0 --dry-run  # build only, no upload
```

## Troubleshooting

| Issue | Fix |
|-------|-----|
| SSH host key failure | `ssh-keyscan -p 2222 aivyx-gitea.cloud >> ~/.ssh/known_hosts` |
| Docker build can't fetch aivyx-core | Ensure runner has SSH key and `release.yml` passes it via `--build-arg` |
| Artifacts not on VPS2 | Check `deploy-releases` job logs; ensure SSH alias `aivyx-studio` works from VPS4 |
| Download links 404 | Verify `/home/aivyx/releases/latest/` has files; Nginx container mounts the volume |

# Infrastructure

> VPS topology, SSH access, and deployment architecture.

**Last updated**: 2026-03-07

## Active Servers

| Host | IP | Plan | Role | SSH |
|------|-----|------|------|-----|
| VPS1 | 72.60.208.87 | KVM4 (4C/16G) | Engine Production | `ssh vps1` |
| VPS5 | 148.230.102.223 | KVM4 (4C/16G) | Gitea + CI Runner | `ssh vps5` |

### Available (wiped, ready)

| Host | IP | Plan |
|------|-----|------|
| VPS2 (nexus) | 72.62.120.196 | KVM4 |
| VPS3 (studio) | 76.13.198.181 | KVM2 |
| VPS6 (ops) | 148.230.103.191 | KVM4 |

## SSH Keys

| Key | Purpose |
|-----|---------|
| `id_ed25519_hostinger` | Root emergency access |
| `id_ed25519_aivyx_deploy` | Unified VPS deploy (all servers) |
| `id_ed25519_aivyx_gitea` | Gitea git SSH (port 2222) |
| `id_ed25519_aivyx_github` | GitHub git SSH |

```bash
ssh vps1        # Engine server
ssh vps5        # Gitea server
```

## Repository Hosting

| Repo | Primary | Mirror | Visibility |
|------|---------|--------|------------|
| aivyx-core | [Gitea](https://aivyx-gitea.cloud/AivyxDev/aivyx-core) | [GitHub](https://github.com/AivyxDev/aivyx-core) | Public |
| aivyx | [GitHub](https://github.com/AivyxDev/aivyx) | — | Public |
| aivyx-engine | [Gitea](https://aivyx-gitea.cloud/AivyxDev/aivyx-engine) | — | Private |

## Docker Stack (VPS1)

```
Traefik (reverse proxy + auto-SSL)
    ↓
aivyx-engine (HTTP API)
    ↓
redb (embedded storage, no Postgres/Redis)
    ↓
Ollama (local LLM inference)
```

## Security

- Non-root `aivyx` user on all servers
- SSH key-only authentication (passwords disabled)
- Custom SSH port (22022)
- Traefik with Let's Encrypt auto-SSL
- All data encrypted at rest by engine

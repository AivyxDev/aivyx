# SSH & Security Reference

> Internal reference for SSH access, key management, and security procedures.
>
> ⚠️ **This file is in the private repo only. Never commit to a public repo.**

## SSH Key Inventory

| Key File | Comment | Purpose |
|----------|---------|---------|
| `~/.ssh/id_ed25519_hostinger` | `hostinger-master` | Root emergency access to Hostinger console |
| `~/.ssh/id_ed25519_aivyx_deploy` | `aivyx-deploy` | Unified deploy key for all VPS (user: `aivyx`) |
| `~/.ssh/id_ed25519_aivyx_gitea` | `aivyx-gitea-deploy` | Gitea git SSH (port 2222) |
| `~/.ssh/id_ed25519_aivyx_github` | `aivyx-github` | GitHub git SSH |

### Archived Keys (in `~/.ssh/old/`)

Old per-VPS keys replaced by the unified deploy key:
- `id_ed25519_aivyx_engine` — was VPS1 only
- `id_ed25519_aivyx_nexus` — was VPS2 only
- `id_ed25519_aivyx_studio` — was VPS3 only
- `id_ed25519_aivyx_ops` — was VPS6 only

---

## SSH Config (`~/.ssh/config`)

```ssh-config
# ─── Hostinger emergency (root) ───
Host hostinger-*
    User root
    IdentityFile ~/.ssh/id_ed25519_hostinger
    IdentitiesOnly yes

# ─── VPS deploy (unified key) ───
Host vps1 engine.aivyx.ai
    HostName 72.60.208.87
    User aivyx
    Port 22022
    IdentityFile ~/.ssh/id_ed25519_aivyx_deploy
    StrictHostKeyChecking accept-new
    ServerAliveInterval 60
    IdentitiesOnly yes

Host vps2 web.aivyx.ai
    HostName 72.62.120.196
    User aivyx
    Port 22
    IdentityFile ~/.ssh/id_ed25519_aivyx_deploy
    StrictHostKeyChecking accept-new
    ServerAliveInterval 60
    IdentitiesOnly yes

Host vps5 gitea.aivyx.ai
    HostName 148.230.102.223
    User aivyx
    Port 22022
    IdentityFile ~/.ssh/id_ed25519_aivyx_deploy
    StrictHostKeyChecking accept-new
    ServerAliveInterval 60
    IdentitiesOnly yes

# ─── Git protocols ───
Host aivyx-gitea.cloud
    HostName aivyx-gitea.cloud
    Port 2222
    User git
    IdentityFile ~/.ssh/id_ed25519_aivyx_gitea
    IdentitiesOnly yes

Host github.com
    User git
    IdentityFile ~/.ssh/id_ed25519_aivyx_github
    IdentitiesOnly yes
```

---

## Quick Access

```bash
# Engine server
ssh vps1

# Web server (aivyx-studio.com)
ssh vps2

# Gitea server
ssh vps5

# Test Gitea git access
ssh -T git@aivyx-gitea.cloud

# Test GitHub git access
ssh -T git@github.com
```

---

## VPS Access Map

| Alias | Hostname | IP | Port | User | Key | Role |
|-------|----------|-----|------|------|-----|------|
| `vps1` | engine.aivyx.ai | 72.60.208.87 | 22022 | aivyx | `_deploy` | Aivyx Engine |
| `vps2` | web.aivyx.ai | 72.62.120.196 | 22 | aivyx | `_deploy` | Website (aivyx-studio.com) |
| `vps5` | gitea.aivyx.ai | 148.230.102.223 | 22022 | aivyx | `_deploy` | Gitea + CI |

### Standby VPS (wiped, fresh Ubuntu 24.04 + Docker)

| Host | IP | Plan | Hostinger ID |
|------|-----|------|-------------|
| VPS3 (studio) | 76.13.198.181 | KVM2 | 1405970 |
| VPS6 (ops) | 148.230.103.191 | KVM4 | 1435936 |

### VPS2 Web Stack

```yaml
# ~/web/docker-compose.yml
services:
  traefik:    # Reverse proxy, auto-SSL (Let's Encrypt)
    image: traefik:v3
    ports: ["80:80", "443:443"]
  website:    # Static Astro site
    image: nginx:alpine
    labels:
      - "traefik.http.routers.website.rule=Host(`aivyx-studio.com`) || Host(`www.aivyx-studio.com`)"
```

**Deploy command:**
```bash
cd ~/Projects/aivyx/apps/website && npm run build
rsync -avz --delete dist/ vps2:~/web/dist/
ssh vps2 'cd ~/web && docker compose restart website'
```

---

## Key Management Procedures

### Generate a New Key

```bash
ssh-keygen -t ed25519 -C "purpose-description" -f ~/.ssh/id_ed25519_purpose -N ""
```

### Deploy a Key to a VPS

```bash
# Read the public key
PUBKEY=$(cat ~/.ssh/id_ed25519_purpose.pub)

# Add to target server
ssh vps1 "echo '$PUBKEY' >> ~/.ssh/authorized_keys"
```

### Provision a Fresh (Wiped) VPS

The standby VPS were wiped via the Hostinger API. After wipe, they have a new root password (check Hostinger panel). To set up:

```bash
# 1. Get root password from Hostinger panel
# 2. SSH in as root (first-time, password auth)
ssh -p 22 root@<IP>

# 3. Create aivyx user
adduser aivyx
usermod -aG sudo aivyx
usermod -aG docker aivyx

# 4. Set up SSH directory
mkdir -p /home/aivyx/.ssh
chmod 700 /home/aivyx/.ssh

# 5. Add deploy key
echo "<deploy-pubkey>" > /home/aivyx/.ssh/authorized_keys
chmod 600 /home/aivyx/.ssh/authorized_keys
chown -R aivyx:aivyx /home/aivyx/.ssh

# 6. Harden SSH (keep port 22 — see lesson learned below)
sed -i 's/^#\?PasswordAuthentication .*/PasswordAuthentication no/' /etc/ssh/sshd_config
sed -i 's/^#\?PermitRootLogin .*/PermitRootLogin no/' /etc/ssh/sshd_config
sed -i 's/^#\?PubkeyAuthentication .*/PubkeyAuthentication yes/' /etc/ssh/sshd_config
systemctl restart ssh  # Note: Ubuntu 24.04 uses 'ssh' not 'sshd'

# 7. Test before disconnecting!
# In a NEW terminal:
ssh aivyx@<IP>
```

> ⚠️ **Lesson Learned:** Do NOT change SSH port during initial provisioning.
> Changing to port 22022 can cause lockout if the firewall doesn't allow the new port.
> VPS1/VPS5 use port 22022 (configured before hardening). VPS2 uses default port 22.
> Also note: Ubuntu 24.04 uses `systemctl restart ssh` (not `sshd`).

### Hostinger API Access

```bash
# List all VPS
curl -s "https://developers.hostinger.com/api/vps/v1/virtual-machines" \
  -H "Authorization: Bearer <API_TOKEN>" | python3 -m json.tool

# Wipe/recreate a VPS (Ubuntu 24.04 + Docker, template_id: 1121)
curl -s -X POST "https://developers.hostinger.com/api/vps/v1/virtual-machines/<ID>/recreate" \
  -H "Authorization: Bearer <API_TOKEN>" \
  -H "Content-Type: application/json" \
  -d '{"template_id": 1121}'
```

---

## Git Push Cheat Sheet

```bash
# aivyx-core → Gitea + GitHub
cd ~/Projects/aivyx-core
git push origin main                    # Gitea (uses gitea key via ssh config)
git push github main                    # GitHub (uses github key via remote)

# aivyx-engine → Gitea only (private)
cd ~/Projects/aivyx-engine
git push origin main                    # Gitea

# aivyx → GitHub only (public)
cd ~/Projects/aivyx
git push origin main                    # GitHub
```

---

## Security Checklist

- [x] All VPS use non-root `aivyx` user
- [x] SSH key-only authentication (passwords disabled)
- [x] Custom SSH port on VPS1/VPS5 (22022), default on VPS2 (22)
- [x] 4 keys total, clear separation of concerns
- [x] Old keys archived to `~/.ssh/old/`
- [x] Traefik with auto-SSL on VPS2 (website)
- [ ] Traefik with auto-SSL on VPS1 (engine API)
- [ ] Firewall rules: only SSH + 80 + 443 open
- [ ] Uptime monitoring configured
- [ ] Automated backups scheduled

---

## Domain Map

| Domain | Points To | Service |
|--------|-----------|---------|
| `aivyx-studio.com` | VPS2 (72.62.120.196) | Website |
| `www.aivyx-studio.com` | VPS2 (72.62.120.196) | Website |
| `aivyx-gitea.cloud` | VPS5 (148.230.102.223) | Gitea + CI |

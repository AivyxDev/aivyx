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

# Gitea server
ssh vps5

# Test Gitea git access
ssh -T git@aivyx-gitea.cloud

# Test GitHub git access
ssh -T git@github.com
```

---

## VPS Access Map

| Alias | Hostname | IP | Port | User | Key |
|-------|----------|-----|------|------|-----|
| `vps1` | engine.aivyx.ai | 72.60.208.87 | 22022 | aivyx | `_deploy` |
| `vps5` | gitea.aivyx.ai | 148.230.102.223 | 22022 | aivyx | `_deploy` |

### Standby VPS (wiped, fresh Ubuntu 24.04 + Docker)

| Host | IP | Plan | Hostinger ID |
|------|-----|------|-------------|
| VPS2 (nexus) | 72.62.120.196 | KVM4 | 1392872 |
| VPS3 (studio) | 76.13.198.181 | KVM2 | 1405970 |
| VPS6 (ops) | 148.230.103.191 | KVM4 | 1435936 |

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

# 6. Harden SSH
sed -i 's/#Port 22/Port 22022/' /etc/ssh/sshd_config
sed -i 's/#PasswordAuthentication yes/PasswordAuthentication no/' /etc/ssh/sshd_config
sed -i 's/PermitRootLogin yes/PermitRootLogin no/' /etc/ssh/sshd_config
systemctl restart sshd

# 7. Test before disconnecting!
# In a NEW terminal:
ssh -p 22022 aivyx@<IP>
```

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
- [x] Custom SSH port (22022)
- [x] 4 keys total, clear separation of concerns
- [x] Old keys archived to `~/.ssh/old/`
- [ ] Firewall rules: only 22022, 80, 443 open
- [ ] Traefik with auto-SSL on all public services
- [ ] Uptime monitoring configured
- [ ] Automated backups scheduled

# Product Roadmap

> Aivyx development roadmap — what's done, what's next.

**Last updated**: 2026-03-07

## ✅ Completed

### Foundation (v0.1.0)

- [x] Three-repo architecture (core, agent, engine)
- [x] 10 shared crates with full test coverage
- [x] CLI with 20+ commands
- [x] Terminal UI with markdown rendering
- [x] Multi-provider LLM (Ollama, OpenAI, Anthropic, Gemini)
- [x] Encrypted memory with semantic triples
- [x] HKDF-SHA256 key derivation + ChaCha20Poly1305 encryption
- [x] Tamper-proof HMAC audit chain
- [x] 22 built-in agent skills
- [x] MCP tool connectivity (stdio + SSE)
- [x] Agent persona system (6 dimensions)
- [x] Session management (save, resume, delete)
- [x] Secret management (encrypted key store)
- [x] Backup and restore
- [x] Scheduling system
- [x] Channel system (Telegram, email)
- [x] Desktop app (Tauri v2)
- [x] TypeScript SDK

### Infrastructure

- [x] VPS1: Engine production server
- [x] VPS5: Gitea + CI runner
- [x] SSH key consolidation (4 keys)
- [x] Fresh documentation (14 docs)

---

## 🔜 Next Up

### v0.2.0 — Polish & Stability

- [ ] CI/CD pipelines per repo (GitHub Actions + Gitea Actions)
- [ ] Binary releases on GitHub (Linux, macOS, Windows)
- [ ] Desktop app builds in CI
- [ ] Engine deployment automation (Docker + VPS1)
- [ ] Error message improvements
- [ ] Performance profiling and optimization

### v0.3.0 — Ecosystem

- [ ] Skill marketplace (user-contributed skills)
- [ ] MCP server directory
- [ ] Plugin system for custom integrations
- [ ] Improved knowledge graph queries
- [ ] Memory visualization in TUI

---

## 🔮 Future

### v0.4.0 — Social

- [ ] Marketing website (aivyx.ai)
- [ ] Discord community
- [ ] Blog with tutorials
- [ ] Video walkthroughs

### v0.5.0 — Teams

- [ ] Multi-agent team sessions (Nonagon)
- [ ] Mission orchestration (complexity-classified)
- [ ] Federation between instances
- [ ] Pro tier launch

### v1.0.0 — Production

- [ ] App store distribution (Flathub, Snap, Homebrew, Winget)
- [ ] Mobile companion app
- [ ] Enterprise SSO integration
- [ ] SLA and support contracts

---

## Principles

1. **Privacy first** — never compromise on encryption or local-first
2. **Open core** — keep core and agent MIT, monetize the Engine
3. **Developer experience** — CLI and terminal-first, always
4. **Quality over speed** — ship when it's solid, not when it's fast

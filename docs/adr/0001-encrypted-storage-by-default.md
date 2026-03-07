# ADR-0001: Encrypted Storage by Default

**Status**: Accepted
**Date**: 2026-03-07
**Deciders**: Aivyx Core Team

## Context

AI agents handle sensitive data on a routine basis: API keys, user conversations,
tool outputs, and long-term memory. Most agent frameworks store this data in
plaintext SQLite databases or JSON files on disk. A compromised disk, backup, or
container image therefore exposes everything the agent has ever processed.

Aivyx needed a storage approach that provides confidentiality guarantees without
requiring end-users to configure encryption themselves. The goal was to make
secure storage the default, not an opt-in afterthought.

Key requirements:

- Encryption must be transparent to application code above the storage layer.
- The scheme must support multi-tenant isolation (one tenant's data must not be
  decryptable with another tenant's key material).
- The master key must never persist in plaintext on disk.
- Performance overhead must remain acceptable for interactive agent workloads.

## Decision

All data at rest in Aivyx uses **ChaCha20-Poly1305 AEAD encryption** via the
`aivyx-crypto` crate. A single master key is derived from a user-supplied
passphrase using **Argon2id** (memory-hard KDF), and **HKDF** domain separation
produces per-purpose subkeys:

```
MasterKey (from passphrase via Argon2id)
  |
  +-- derive_audit_key(master)              --> audit log encryption
  +-- derive_memory_key(master)             --> memory store encryption
  +-- derive_tenant_key(master, tenant_id)  --> per-tenant isolation
```

### Storage layer

The `EncryptedStore` struct wraps `redb` (an embedded, crash-safe key-value
store written in Rust). Every value is encrypted before writing and decrypted
after reading. Keys (used for lookups) are stored as BLAKE3 hashes so that
the storage engine can perform equality lookups without exposing plaintext
key material.

### Key lifecycle

- The `MasterKey` type is **non-Clone** and implements `Zeroize + ZeroizeOnDrop`
  via the `secrecy` crate, ensuring key material is scrubbed from memory as soon
  as it goes out of scope.
- On startup, the engine prompts for (or reads from an environment variable) the
  passphrase, derives the master key, and holds it in memory for the lifetime of
  the process.
- Subkeys are derived on demand and cached in a `SecretVec` pool that is also
  zeroized on drop.

### Encryption details

| Parameter          | Value                          |
|--------------------|--------------------------------|
| AEAD cipher        | ChaCha20-Poly1305 (RFC 8439)   |
| KDF (passphrase)   | Argon2id (t=3, m=64 MiB, p=1) |
| KDF (subkeys)      | HKDF-SHA256                    |
| Nonce              | 96-bit, random per write       |
| Tag                | 128-bit (appended to ciphertext) |

## Consequences

### Positive

- **Zero-config security**: encryption is active out of the box with no user
  configuration required beyond providing a passphrase.
- **Data confidentiality**: even if the disk, container image, or backup is
  compromised, data remains encrypted.
- **Per-tenant key isolation**: in multi-tenant deployments, each tenant's data
  is encrypted with a distinct derived key, so compromise of one tenant's
  key material does not affect others.
- **Memory safety**: the `secrecy` crate ensures key material is zeroized on
  drop, reducing the window for memory-based attacks.

### Negative

- **~5% storage overhead**: each encrypted value carries a 12-byte nonce and
  16-byte AEAD tag (28 bytes per entry).
- **No queryable ciphertext**: you cannot perform range queries, LIKE searches,
  or indexing over encrypted values. All filtering must happen after decryption.
- **Debugging requires the master key**: inspecting stored data requires access
  to the passphrase or master key, which complicates support workflows.

### Trade-offs

- **ChaCha20-Poly1305 over AES-GCM**: ChaCha20 provides consistent performance
  across all platforms, including ARM devices without AES-NI hardware
  acceleration. AES-GCM would be faster on x86 with AES-NI but significantly
  slower on devices without it. Since Aivyx targets edge and IoT deployments in
  addition to cloud servers, ChaCha20-Poly1305 was the better default.
- **redb over SQLite**: redb is a pure-Rust embedded store with ACID semantics
  and no C dependencies, simplifying cross-compilation and reducing the attack
  surface. The trade-off is a less mature query model compared to SQLite.
- **Single master key with HKDF over independent keys**: a single passphrase
  reduces user burden at the cost of a single point of compromise. HKDF domain
  separation mitigates this by ensuring a leaked subkey does not reveal the
  master key or other subkeys.

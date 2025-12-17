# docs/specs/secrets/encryption-at-rest.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
Define how secrets are encrypted at rest in the control plane:
- data model for secret bundles and secret versions
- encryption scheme and key hierarchy
- rotation and rewrapping strategy
- audit requirements for secret updates and secret material access

Locked decision: secrets are delivered as a fixed-format file, and secret updates use restart semantics. See:
- `docs/adr/0010-secrets-delivery-file-format.md`
- `docs/specs/secrets/format.md`
- `docs/specs/secrets/delivery.md`

## Scope
This spec defines encryption at rest for secret material stored by the platform.

This spec does not define:
- the secrets file syntax (`docs/specs/secrets/format.md`)
- delivery timing and restart semantics (`docs/specs/secrets/delivery.md`)
- control plane auth scopes (`docs/specs/api/auth.md`)

## Threat model (v1 assumptions)
We want to reduce impact of:
- Postgres compromise (attacker gets DB contents)
- backup compromise (attacker gets DB backups)
- accidental logging of secret values

We do not fully solve:
- control plane runtime compromise while keys are loaded in memory
- a malicious operator with full machine access
- a fully compromised host agent that legitimately receives plaintext for delivery

Design stance:
- encrypt secret material before it is stored in Postgres
- keep master keys outside Postgres
- minimize plaintext persistence on disk

## Definitions
- **Secret bundle**: env-scoped container for secrets metadata.
- **Secret version**: immutable version of a secret bundle.
- **Secret material**: the canonical secrets file bytes (plaintext) that must be delivered to the guest.
- **Data key**: per-secret-version symmetric key used to encrypt secret material.
- **Master key**: operator-managed key used to wrap (encrypt) data keys (envelope encryption).
- **Wrap**: encrypt data_key with master key, producing wrapped_data_key.

## High-level model
1) Control plane validates and canonicalizes secrets input:
- parse according to `docs/specs/secrets/format.md`
- canonicalize to canonical bytes
- compute `data_hash = sha256(canonical_bytes)`

2) Control plane encrypts canonical bytes:
- generate random `data_key` for this secret version
- encrypt canonical bytes with AEAD using data_key
- wrap data_key using a master key

3) Control plane stores only ciphertext and envelope metadata in Postgres.
4) Secret values never appear in the event log. The event log stores only:
- bundle_id, version_id, data_hash, and timestamps

## Encryption scheme (normative)
### AEAD requirements
- Use an authenticated encryption mode (AEAD).
- v1 recommended cipher: AES-256-GCM.

### Nonce requirements
- Use a random nonce per encryption.
- For AES-GCM, nonce length is 12 bytes.

### Associated data (AAD) requirements
AAD binds ciphertext to its identity and prevents swapping ciphertext across versions.

AAD string (v1):
- `trc-secrets-v1|org:<org_id>|env:<env_id>|bundle:<bundle_id>|version:<version_id>|hash:<data_hash>`

Store AAD metadata indirectly by storing those fields; you do not need to store AAD separately if it can be reconstructed exactly.

### Plaintext
Plaintext is the canonical secrets file bytes from `format.md` canonicalization.

### Output
Encryption output for a secret version:
- `ciphertext` (bytes)
- `nonce` (bytes)
- `cipher` (string, `aes-256-gcm`)
- `wrapped_data_key` (bytes)
- `master_key_id` (string)

## Key hierarchy and key management
### Data keys
- One data_key per secret version.
- Data key is generated randomly (32 bytes).
- Data key is never stored in plaintext in Postgres.

### Master keys
- Master keys are operator-managed.
- Master keys are versioned by `master_key_id`.
- Master keys must be stored outside Postgres (example: SOPS + age encrypted file on control plane hosts, or a KMS later).

v1 minimum requirements:
- control plane can load master keys at startup
- control plane can look up a master key by master_key_id
- master keys can be rotated without breaking the ability to decrypt older secret versions

### Master key rotation policy
v1 recommended rotation:
- introduce a new `master_key_id` periodically (example quarterly) or after incident
- new secret versions use the newest master key
- old master keys are retained at least as long as the maximum secrets retention window

Rewrapping strategy (optional in v1, recommended later):
- background job rewraps old wrapped_data_keys to the newest master key without changing ciphertext
- rewrap does not change secret version id and does not change data_hash
- rewrap updates only:
  - master_key_id
  - wrapped_data_key

## Storage model in Postgres (recommended)
### Table: `secret_bundles`
Represents env-scoped bundle metadata.
- `bundle_id` (pk)
- `org_id`
- `app_id`
- `env_id` (unique in v1)
- `format` (platform_env_v1)
- `created_at`
- `updated_at`

### Table: `secret_versions`
Represents immutable versions and references secret material.
- `version_id` (pk)
- `bundle_id` (fk)
- `org_id`
- `env_id`
- `data_hash` (sha256 hex of canonical plaintext)
- `created_at`
- `created_by_actor_id`
- `created_by_actor_type`
- `material_id` (fk to secret_material)

Uniqueness:
- `(bundle_id, data_hash)` may be unique if you want to dedupe identical sets. If you do, be careful with audit semantics.

### Table: `secret_material`
Stores ciphertext and envelope metadata.
- `material_id` (pk)
- `cipher` (text, v1 `aes-256-gcm`)
- `nonce` (bytes)
- `ciphertext` (bytes)
- `master_key_id` (text)
- `wrapped_data_key` (bytes)
- `plaintext_size_bytes` (int)
- `created_at`

Notes:
- ciphertext can be stored as `bytea`.
- keep sizes bounded by the limits in `format.md`.

## Access control and plaintext handling
### Who can decrypt
Only the control plane secrets subsystem may decrypt secret material.
- It must be protected behind internal service boundaries and scopes.
- Tenants do not receive plaintext from the API by default.

### When decryption is allowed
Decryption occurs only to deliver secrets to workloads during boot (and optionally for controlled debug flows if you later add them).

v1 stance:
- do not provide a public API endpoint that returns plaintext secrets.
- CLI rendering is local. It renders a local file, or renders from user-provided input, not by downloading secrets material.

### How secrets reach the host agent (v1)
To avoid distributing master keys to every node, v1 recommended flow:
1) Host agent requests secret version material from control plane over mTLS.
2) Control plane decrypts secret material in memory.
3) Control plane streams plaintext bytes to host agent over the authenticated channel.
4) Host agent forwards plaintext to guest init over vsock.
5) Guest init writes plaintext into `/run/secrets/platform.env` on tmpfs.

Constraints:
- host agent must not persist plaintext secrets to disk
- if any temporary buffering occurs, it must be in memory or tmpfs and must be short-lived

### Audit for secret material access
Every decrypt-and-deliver action must be auditable internally.

v1 recommended internal audit record fields:
- timestamp
- org_id, env_id, bundle_id, version_id
- instance_id (if applicable)
- node_id (if applicable)
- actor_type = system (or agent identity)
- purpose = delivery
- result = success/failure
- failure category (no key or ciphertext details)

This audit record is not tenant-readable by default.

## Event log interaction (normative)
Events must never contain secret material or ciphertext.

Allowed in events:
- bundle_id
- version_id
- data_hash
- format
- timestamps

Not allowed in events:
- ciphertext
- nonce
- wrapped_data_key
- any plaintext values

Event types involved:
- `secret_bundle.created`
- `secret_bundle.version_set`

## Failure behavior
### Master key unavailable
If master_key_id required for a secret version is not available:
- secrets delivery fails
- instance fails with `secrets_injection_failed`
- reason_detail: `master_key_unavailable`

### Ciphertext or envelope corruption
If ciphertext cannot be decrypted or integrity check fails:
- secrets delivery fails
- instance fails with `secrets_injection_failed`
- reason_detail: `decrypt_failed` or `integrity_failed`

### Format validation bug
If plaintext is malformed (should not happen if validated at ingest):
- fail delivery
- reason_detail: `format_invalid`
- treat as a platform bug and incident

## Operational requirements
- Master keys must be backed up and protected separately from Postgres backups.
- Restore drills must include:
  - restoring Postgres (ciphertext and metadata)
  - verifying master keys are available
  - successfully decrypting and delivering secrets for a test env

## Observability requirements
Metrics:
- secrets set operations count and latency
- secrets delivery successes and failures by reason_detail
- decrypt latency
- master key lookup failures

Alerts (minimum):
- spike in secrets delivery failures
- master_key_unavailable
- repeated decrypt_failed for the same version_id (likely corruption)

Logs:
- never log plaintext
- never log wrapped_data_key or nonce
- log version_id and env_id only as metadata when needed

## Open questions (future)
- Introducing a KMS for master key operations (wrap/unwrap) to avoid long-lived master keys on control plane hosts.
- Tenant-visible “secrets export” feature. Not recommended for v1 because it expands blast radius and abuse surface.

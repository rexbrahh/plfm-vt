# docs/specs/storage/backups.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
Define asynchronous backups for local volumes:
- how snapshot data becomes a durable remote backup
- backup storage backend assumptions (S3-compatible)
- client-side encryption and envelope key handling
- integrity verification
- retention and pruning rules
- restore prerequisites (what must exist for restore to work)
- failure handling and alerting

Locked decision: storage is local volumes with async backups. See `docs/ADRs/0011-storage-local-volumes-async-backups.md`.

## Scope
This spec defines remote backup behavior.

This spec does not define:
- how volumes are created and attached (`docs/specs/storage/volumes.md`)
- how snapshots are created (`docs/specs/storage/snapshots.md`)
- restore and migration semantics (`docs/specs/storage/restore-and-migration.md`)
- runtime mount behavior inside the guest (`docs/specs/runtime/volume-mounts.md`)

## Definitions
- **Snapshot**: a point-in-time capture of a volume (see snapshots spec).
- **Backup**: a durable remote copy of snapshot bytes stored in object storage.
- **Backup store**: operator-configured S3-compatible object storage.
- **Data key**: per-backup symmetric key used to encrypt snapshot content.
- **Master key**: platform key used to wrap (encrypt) data keys (envelope encryption).
- **Backup metadata**: control plane record that ties `snapshot_id` to a remote object key, integrity hashes, and key envelope reference.

## v1 stance (important)
1) Backups are asynchronous. They do not provide synchronous replication or zero data loss.
2) Backups are platform-managed. Tenants do not download raw backup objects in v1.
3) Backups are encrypted client-side before upload.
4) Retention is cluster policy first (not user-configurable in v1).

## Relationship to snapshots (v1 integration)
This spec assumes the v1 product model:
- A snapshot request results in an upload to the backup store.
- A snapshot is considered durable only after its backup upload is complete and verified.

Practical interpretation:
- `snapshot.status = succeeded` SHOULD mean remote backup exists and passed integrity checks.
- Local snapshot artifacts are temporary and must be cleaned up after successful upload (or by TTL as a safety net).

If later you split snapshot and backup into separate job objects, this spec still applies to the backup portion, but the status model changes.

## Backup store (S3-compatible)
### Requirements
Operator must configure:
- endpoint (optional, for non-AWS)
- bucket name
- region (optional)
- credentials (access key + secret, or instance role depending on environment)
- optional prefix (for multi-cluster separation)

### Security requirements
- Backup store credentials are operator secrets.
- Credentials must not be exposed to tenants.
- Credentials must never be logged.
- Access should be restricted to:
  - write objects under the configured prefix
  - read objects for restore
  - delete objects for retention pruning

## Backup object layout
Backups are stored as objects. One backup corresponds to one snapshot.

### Object key convention (recommended)
- `backups/<cluster_id>/<org_id>/<volume_id>/<snapshot_id>.bin`

Optional separate metadata object is not required because metadata is stored in Postgres, but you may also store:
- `backups/.../<snapshot_id>.meta.json` (optional, internal)

### Object properties
- Content is ciphertext (encrypted snapshot bytes).
- Object size is the ciphertext byte length.
- Multipart upload is allowed and recommended for large volumes.

## Encryption model (normative)
Backups must be encrypted before leaving the node.

### Envelope encryption
For each backup:
1) Generate a random per-backup `data_key` (32 bytes).
2) Encrypt snapshot bytes using `data_key` with an AEAD cipher.
3) Wrap `data_key` using a platform `master_key` (envelope).
4) Store only the wrapped key and a master key id in control plane internal metadata.

### Cipher requirements
- Must use an AEAD (authenticated encryption).
- Acceptable choices:
  - AES-256-GCM
  - XChaCha20-Poly1305

v1 recommendation:
- AES-256-GCM with chunked streaming (below) because it is widely available in Go and Rust.

### Chunked streaming encryption (required for large volumes)
Volumes can be large. Encryption must support streaming without buffering the whole volume.

Normative scheme:
- Choose a `chunk_size_bytes` (recommended 4 MiB).
- Generate a random `base_nonce` (12 bytes for AES-GCM).
- For chunk index `i` starting at 0:
  - nonce_i = base_nonce + i (96-bit big-endian integer addition)
  - ciphertext_i = AEAD_Encrypt(data_key, nonce_i, plaintext_i, associated_data)
- Upload concatenation of ciphertext chunks.

Associated data (AAD) must bind the backup identity:
- `AAD = "trc-backup-v1" || org_id || volume_id || snapshot_id || chunk_index`
This prevents swapping chunks between backups.

### What is stored as metadata
Two categories:

Tenant-visible metadata (safe):
- snapshot_id, volume_id, org_id
- created_at, consistency
- plaintext_size_bytes (optional)
- ciphertext_size_bytes
- plaintext_sha256 (optional, computed during upload)
- ciphertext_sha256 (optional)
- backup_status (succeeded or failed, exposed via snapshot status in v1)

Internal-only metadata (must not be tenant-readable):
- master_key_id
- wrapped_data_key
- base_nonce
- chunk_size_bytes
- cipher id and params

Reason:
- Even wrapped keys should be treated as sensitive and unnecessary for tenant APIs.

## Key management assumptions (v1)
### Master keys
- Master keys are operator-managed.
- Master keys must be versioned by `master_key_id`.
- Rotation is allowed:
  - new backups use the newest master_key_id
  - old backups remain decryptable using old master keys retained by policy

### Storage of master keys
v1 recommendation:
- manage master keys via SOPS + age (operator-only) or a small internal KMS service later.
- master keys must not live in the event log.

### Key rotation policy (recommendation)
- rotate master keys on a schedule (example quarterly) or after incident.
- keep previous keys for at least the maximum retention window of backups, otherwise old backups become unreadable.

## Upload pipeline (normative)
Backups are executed on the volume home node.

### Steps
1) Ensure snapshot artifact exists (LVM snapshot LV or equivalent).
2) Open snapshot device for read.
3) Stream read snapshot bytes in chunks:
   - compute plaintext_sha256 incrementally
   - encrypt chunk
   - compute ciphertext_sha256 incrementally (optional but recommended)
4) Upload ciphertext stream to object store.
5) On upload completion:
   - verify upload success (ETag or multipart completion ok)
   - persist backup metadata transactionally:
     - store object key
     - store checksums and sizes
     - store internal key envelope reference
6) Mark snapshot as succeeded only after:
   - backup metadata is committed
   - integrity checks pass (at least checksum computed and stored)

### Atomicity and “done” marker
To avoid “object exists but metadata not committed” ambiguity:
- Commit metadata only after upload completes.
- If possible, use a temporary object key during upload and rename or copy to final key after completion.
  - S3 does not support atomic rename, so copying is expensive.
  - v1 recommendation: upload directly to final key, but treat the presence of a metadata record in Postgres as the authoritative “backup exists” signal.

Cleanup rule:
- If upload succeeds but metadata commit fails, the next reconciliation should detect orphan objects and either:
  - create metadata if safe, or
  - delete orphan objects after a cooling-off period.

## Integrity verification (normative)
At minimum, record:
- `plaintext_sha256`
- `ciphertext_size_bytes`
- `chunk_size_bytes`
- `base_nonce`

Restore must verify:
- ciphertext length matches recorded length (if recorded)
- decryption succeeds for all chunks (AEAD tags)
- plaintext_sha256 matches recorded value

If any check fails:
- restore must fail and not attach the volume to workloads.

## Backup status and failure handling
### Failure categories (recommended reason codes)
- `backup_store_unreachable`
- `upload_failed`
- `encryption_failed`
- `checksum_failed`
- `credentials_invalid`
- `disk_read_failed`
- `snapshot_missing`
- `internal_error`

### Retry policy
- Retries should be handled by scheduling a new snapshot and backup, not by retrying the same snapshot artifact forever.
- If you do retry an upload from the same snapshot artifact:
  - ensure the artifact still exists
  - enforce TTL to avoid thin pool exhaustion

v1 recommendation:
- one retry attempt from the same snapshot artifact, then fail and rely on next scheduled backup.

## Retention and pruning
Retention is a cluster-level policy.

### Default policy (v1 recommendation)
- Keep last `N` successful backups per volume (default N=14).
- Prune older backups after a new successful backup is recorded.

### Pruning semantics
Pruning removes:
- remote object from backup store
- backup metadata record in Postgres
- internal key envelope record (if stored separately), after object deletion

Safety rule:
- Never prune the most recent successful backup automatically.
- If a volume is deleted, pruning behavior is policy-defined:
  - v1 recommendation: keep backups for a grace period (example 7 days) unless user explicitly requests deletion.

## Restore prerequisites (what must exist)
To restore a snapshot_id:
- backup metadata record exists
- object exists at store_key
- master_key_id is available to decrypt wrapped data key
- integrity metadata is present (plaintext_sha256, chunk config)

If any prerequisite is missing:
- restore must fail with a clear reason.

## Multi-tenant access control
Tenants can:
- list snapshot metadata for volumes they own (org-scoped)
- request snapshot creation
- request restore (creates a new volume)

Tenants cannot:
- access raw backup objects
- access encryption metadata
- access other org backups

Operator can:
- inspect backup failures
- rotate keys
- manage backup store credentials

## Observability requirements
### Metrics
- backup success/failure counts per node
- backup duration
- upload throughput
- time since last successful backup per volume
- queue depth of backup jobs
- orphan object count (if tracked)

### Alerts (minimum)
- volume has no successful backup in > X days (default 2)
- backup failure rate spike on a node
- backup store unreachable
- thin pool pressure caused by lingering snapshot artifacts

### Logs
Agent logs must include:
- snapshot_id, volume_id, node_id
- upload start/end
- failure category and short detail
No credentials, no plaintext data.

## Compliance tests (required)
1) Backup encrypts data and produces a ciphertext object that cannot be mounted or interpreted as plaintext.
2) Restore decrypts and verifies checksum, producing a usable ext4 volume.
3) Corrupting one ciphertext chunk causes restore failure (AEAD integrity).
4) Retention pruning deletes old backups and keeps the last N.
5) Orphan handling: if upload succeeds but metadata commit fails, reconciliation handles it safely without breaking restores.

## Open questions (future)
- Incremental backups and deduplicated storage (not v1).
- Tenant-managed backup destinations (not v1).
- Optional customer export of raw backups (would require major security and product work).

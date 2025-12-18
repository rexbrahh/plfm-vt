# docs/specs/storage/snapshots.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
Define snapshot semantics for local volumes:
- what a snapshot is in this platform
- lifecycle, states, and idempotency
- consistency levels and what we do and do not guarantee
- execution model (who runs the snapshot and where)
- cleanup and retention rules for local snapshot artifacts
- how snapshots relate to backups and restores

Locked decision: storage is local volumes with asynchronous backups. See `docs/ADRs/0011-storage-local-volumes-async-backups.md`.

## Scope
This spec defines snapshots only.

This spec does not define:
- remote backup upload format, encryption, and retention (`docs/specs/storage/backups.md`)
- restore semantics (`docs/specs/storage/restore-and-migration.md`)
- runtime mount mapping inside the guest (`docs/specs/runtime/volume-mounts.md`)
- host agent isolation requirements (`docs/specs/runtime/limits-and-isolation.md`)

## Definitions
- **Volume**: a local persistent block device on one node.
- **Snapshot**: a point-in-time capture of a volume’s block device state.
- **Crash-consistent snapshot**: equivalent to power loss at an arbitrary time. Filesystem consistency depends on journaling and app behavior.
- **Application-consistent snapshot**: taken after flushing and (briefly) freezing the filesystem so in-flight writes are minimized.
- **Snapshot artifact**: the local snapshot device on the home node (example: an LVM thin snapshot LV).
- **Backup artifact**: the remote, encrypted copy of snapshot bytes stored in object storage.

## v1 stance
1) Snapshots exist to support asynchronous backups and manual recovery.
2) Snapshots are executed on the volume’s home node only.
3) v1 guarantees crash-consistent snapshots. Application-consistent is best-effort only and must never block snapshots indefinitely.
4) Local snapshot artifacts are temporary and must be cleaned up promptly.
5) A snapshot request is an auditable control plane operation with a visible status lifecycle.

## Snapshot object model (control plane)
A snapshot object is a tracked job with immutable identity.

### Snapshot fields (minimum)
- `snapshot_id` (opaque id)
- `org_id`
- `volume_id`
- `requested_by` (actor metadata, from event log envelope)
- `requested_at`
- `status` (queued, running, succeeded, failed)
- `consistency` (crash, application)  
  v1 default is crash. If application was achieved, mark application.
- `size_bytes` (optional, set on success)
- `failed_reason` (optional, set on failure)
- `note` (optional, user-provided label)
- `source_node_id` (the volume home node id, required)

### Status lifecycle
Monotonic transitions:
- `queued -> running -> succeeded`
- `queued -> running -> failed`
- `queued -> failed` (only if preflight fails before execution starts)

Terminal statuses:
- `succeeded`, `failed`

## Idempotency
Snapshot creation is a write endpoint and must support Idempotency-Key.

### Dedup key
- `(org_id, volume_id, idempotency_key, endpoint_name="create_snapshot")`

### Dedup behavior
- Same key and same request body: return the existing snapshot metadata.
- Same key and different request body: reject with `409 conflict` and code `idempotency_key_reuse`.

Retention for idempotency records:
- minimum 24 hours.

## Execution model
Volumes are local, so snapshots must run on the volume’s home node.

Two acceptable execution implementations:
1) **Agent-executed** (recommended v1)
- Control plane records snapshot request as events.
- The node agent on the home node performs snapshot steps and reports status changes.

2) **Operator worker + agent**
- Control plane has a worker that schedules the job, but the actual snapshot creation still occurs on the home node via agent.

v1 recommendation:
- agent-executed, because it keeps volume operations near the device and avoids central bottlenecks.

## Snapshot mechanism (v1 required)
The platform must standardize on one snapshot mechanism for predictability.

v1 recommendation (and assumed by this spec):
- LVM thin provisioning for volume pool
- LVM thin snapshots for snapshot artifacts

If you do not use LVM, you must provide an equivalent block-level snapshot mechanism and update this spec.

## Consistency levels
### Crash-consistent (required in v1)
This is always achievable as long as the volume device is readable.

Behavior:
- create snapshot without coordinating with the guest
- mark consistency as `crash`

### Application-consistent (best-effort, bounded)
If the volume is attached and mounted in a running microVM, the platform MAY attempt to quiesce writes.

Bounded behavior rules (normative):
- The quiesce attempt must have a strict timeout (v1 recommended 2 seconds).
- If quiesce fails or times out, proceed with crash-consistent snapshot.
- Never block snapshot creation indefinitely waiting for guest cooperation.

Possible quiesce techniques (choose what you can support):
- guest filesystem freeze (fsfreeze) via a platform-controlled mechanism
- app-specific hooks (not supported in v1)

v1 recommendation:
- treat quiesce as optional. Ship crash-consistent first. Add quiesce only after you have a clean, audited control channel into the guest.

## Snapshot execution steps (normative)
Given a snapshot request on volume V with home node N:

### Step 0: preflight (on N)
- Verify volume exists locally and is in `available` or `in_use` state.
- Verify thin pool has headroom for snapshot metadata and CoW growth.
- If preflight fails:
  - mark snapshot failed with `failed_reason=preflight_failed:<detail>`

### Step 1: mark running (control plane state)
- Update status to `running` via event:
  - `snapshot.status_changed` with `status=running`

### Step 2: optional quiesce (best-effort)
- If the volume is currently attached and mounted, attempt quiesce within timeout.
- Record whether quiesce succeeded.
- If succeeded, set `consistency=application`, else `consistency=crash`.

### Step 3: create snapshot artifact (LVM thin snapshot)
- Create an LVM thin snapshot LV of the volume LV.
- Name convention (recommended):
  - `snap_<snapshot_id>`
- If creation fails:
  - mark snapshot failed with `failed_reason=snapshot_create_failed:<detail>`

### Step 4: handoff to backup pipeline
Snapshots exist for backups. The snapshot artifact becomes the source for streaming upload.

Two allowed patterns:
- A) snapshot worker immediately begins upload, then deletes snapshot artifact when done.
- B) snapshot worker creates snapshot artifact and queues a backup job, then backup worker uploads and deletes artifact.

v1 recommendation:
- pattern B for clearer separation and retries, but either is acceptable.

### Step 5: mark status succeeded or failed
- On successful completion of the snapshot phase (artifact created and handed off):
  - it is acceptable to mark snapshot as succeeded even if backup upload is separate.
  - in that case, backup has its own job status.
- If you treat snapshot and backup as a single job in v1:
  - only mark succeeded after upload completes and integrity is verified.

Pick one model and keep it consistent in the API.
v1 recommendation:
- separate snapshot status from backup status:
  - snapshot succeeded means local artifact exists and is valid
  - backup status tracks remote durability

### Step 6: cleanup local artifact
- Local snapshot artifacts must not accumulate indefinitely.
- Cleanup triggers:
  - after successful backup upload (preferred)
  - after a failed backup upload when giving up
  - after TTL expiry (safety net)

TTL policy (recommended):
- default TTL 24 hours
- configurable cluster-wide
- if TTL expires and backup not completed, mark backup failed and delete artifact to protect pool health

## Concurrency and limits
### One snapshot per volume at a time (v1 rule)
- The platform must not run multiple concurrent snapshots for the same volume.
- If a snapshot is already queued or running:
  - either reject new request with `409 conflict` (`snapshot_in_progress`)
  - or coalesce new request to the existing snapshot (only if request is identical)

v1 recommendation:
- coalesce only via idempotency key. Otherwise reject with conflict.

### Global snapshot throttling
To protect thin pool performance:
- enforce a per-node concurrent snapshot limit (operator-configured, example 2).
- excess snapshot jobs remain queued.

## Failure reporting
Snapshot failures must include structured reasons.

Suggested `failed_reason` values:
- `preflight_failed:pool_low_space`
- `preflight_failed:volume_not_found_on_node`
- `snapshot_create_failed:lvm_error`
- `quiesce_failed:timeout`
- `quiesce_failed:unsupported`
- `internal_error:<detail>`

These are surfaced to the tenant, but must not leak host paths or sensitive infra details.

## Events and views mapping
Event types (as per state spec):
- `snapshot.created`
- `snapshot.status_changed`

Materialized view:
- `snapshots_view`

Required payload fields for events:
- `snapshot.created` must include: snapshot_id, org_id, volume_id, note (optional)
- `snapshot.status_changed` must include: snapshot_id, status, and optional failed_reason and size_bytes when known

If you record `consistency`, include it either:
- in `snapshot.status_changed` when transitioning to succeeded, or
- as a field in the snapshots view derived from agent report.

## Observability requirements
### Metrics (agent)
- snapshot queued count
- snapshot running count
- snapshot duration (seconds)
- snapshot failures by failed_reason category
- thin pool free space and snapshot count (to detect leaks)

### Logs (agent)
Log at info level:
- snapshot_id, volume_id, node_id
- start and end of snapshot steps
Log at error level:
- failed_reason and the failing subsystem (preflight, lvm, quiesce)

Never log:
- raw volume contents
- operator secrets (backup store creds)

## Security considerations
- Snapshot operations run as trusted agent actions on the host.
- Tenants can request snapshots for volumes they own, but cannot access raw snapshot bytes directly.
- Snapshot metadata is tenant-readable within org boundary.
- Snapshot artifacts are local and must be protected from other tenants via host permissions and isolation.

## Compliance tests (required)
1) Create a volume, write known data, snapshot, and verify restored copy contains the data.
2) Snapshot request is idempotent with Idempotency-Key.
3) Concurrent snapshot requests for the same volume are rejected or coalesced safely.
4) Snapshot fails cleanly when thin pool is low space and surfaces correct failed_reason.
5) Cleanup removes local snapshot artifacts after backup completion or TTL expiry.

## Open questions
- Whether to expose snapshot scheduling controls per volume (v1 recommendation: no, cluster policy only).
- Whether to support incremental snapshots (not v1).
- Whether to implement application-consistent snapshots via guest control channel (future, requires a clean vsock protocol and careful auditing).

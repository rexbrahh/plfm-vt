# docs/specs/storage/restore-and-migration.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
Define restore and migration semantics for local volumes:
- restore a volume from a snapshot backup into a new volume
- recovery guarantees (RPO and RTO framing)
- restore execution model (where it runs, who is responsible)
- migration across nodes (v1 is restore-based migration)
- safety rules to prevent accidental data loss
- validation and observability requirements

Locked decision: persistent storage is local volumes with asynchronous backups. See `docs/ADRs/0011-storage-local-volumes-async-backups.md`.

## Scope
This spec defines restore and migration behavior for local volumes.

This spec does not define:
- snapshot creation (`docs/specs/storage/snapshots.md`)
- backup encryption and upload pipeline (`docs/specs/storage/backups.md`)
- volume mount behavior inside microVMs (`docs/specs/runtime/volume-mounts.md`)
- scheduler placement rules beyond locality constraints (`docs/specs/scheduler/*`)

## Definitions
- **Restore**: create a new volume from a snapshot backup.
- **Migration**: move state to another node by restoring into a new volume and reattaching (no live migration in v1).
- **RPO**: recovery point objective. Bounded by the time of the last successful backup.
- **RTO**: recovery time objective. Restore duration plus time to restart workloads and reattach volumes.
- **Home node**: the node that physically hosts a local volume.

## v1 stance (important)
1) Restore creates a new volume id. No in-place overwrite restores in v1.
2) Migration is a controlled, downtime-expected workflow. There is no live migration in v1.
3) Restore and migration are auditable operations.
4) Restore correctness is verified by integrity checks before the new volume is considered available.
5) Restore always results in a local volume on one node (the destination becomes the new home node).

## Restore inputs
A restore request specifies:
- `snapshot_id` (required)
- optional `target_node_id` (optional)
- optional `new_volume_name` (optional)

The snapshot_id must refer to a snapshot that has a completed durable backup artifact. In v1, “snapshot succeeded” should imply backup succeeded as well (see backups spec). If you split snapshot and backup later, this spec must be updated to reference backup ids explicitly.

## Restore outputs
A restore produces:
- `restore_id` (restore job id)
- `new_volume_id` (assigned early, becomes available on success)
- `status` (queued, running, succeeded, failed)
- `failed_reason` (if failed)

The new volume is:
- owned by the same org as the snapshot source volume
- created on a specific destination node
- formatted ext4 (v1)
- marked `available` only after integrity verification succeeds

## Restore lifecycle
States:
- `queued`
- `running`
- `succeeded`
- `failed`

Monotonic transitions:
- queued -> running -> succeeded
- queued -> running -> failed
- queued -> failed (preflight failed before starting work)

## Restore execution model
Restore must execute on the destination node because the new local volume is created there.

v1 recommendation:
- node agent on destination node performs:
  - volume provisioning (new LV)
  - download and decrypt backup object
  - write block data to the LV
  - integrity verification
  - status reporting

The control plane coordinates:
- selecting destination node (if not provided)
- recording restore job state transitions as events
- creating the new volume record and setting its home_node_id

## Destination selection rules
If `target_node_id` is provided:
- validate node is active (not draining, not disabled)
- validate node has sufficient free capacity in the volume pool
- validate node can reach the backup store

If `target_node_id` is not provided:
- control plane selects a node using:
  - available volume pool capacity
  - node state (active)
  - optional locality preferences (same region)

v1 simplification:
- select any active node with sufficient capacity.

If no suitable node exists:
- restore remains queued or fails with `failed_reason=no_capacity`.

## Restore preconditions (normative)
Before a restore starts:
1) Snapshot exists and belongs to org_id.
2) Snapshot status is succeeded.
3) Backup metadata exists for snapshot_id:
- store_key is known
- integrity metadata exists (plaintext checksum, chunk params)
- key envelope metadata is available (master_key_id, wrapped data key, base nonce)
4) Master key material for master_key_id is available to the platform.
5) Destination node has sufficient capacity for the new volume size.

If any precondition fails:
- mark restore failed with a clear failed_reason:
  - `snapshot_not_found`
  - `snapshot_not_succeeded`
  - `backup_metadata_missing`
  - `backup_object_missing`
  - `master_key_unavailable`
  - `no_capacity`
  - `node_not_eligible`

## Restore steps (normative)
Given snapshot S for volume V, restore to new volume V2 on node N2:

### Step 1: create restore job record
- create restore_id
- record initial status `queued`

### Step 2: allocate new volume id
- create `new_volume_id`
- decide `home_node_id = N2`
- create a volume record for V2 with state `creating`

### Step 3: mark restore running
- status becomes `running`

### Step 4: provision new local volume on destination node
- create a new LV in the volume pool with size >= source volume size
- format ext4 if restore writes raw bytes that already contain a filesystem, do not reformat
  - v1 recommended model: restore writes the exact block contents from backup, so you do not format before writing
  - formatting occurs only for brand-new empty volumes created by users

### Step 5: download and decrypt backup stream
- stream ciphertext from backup store key
- decrypt using envelope metadata:
  - unwrap data key with master_key_id
  - apply chunked AEAD decryption
- write plaintext bytes to the new LV device

### Step 6: integrity verification
At minimum:
- verify AEAD tags (implicit during decryption)
- verify plaintext_sha256 matches recorded metadata
Optional (recommended):
- run a fast ext4 check (`fsck -n`) before marking volume available

If any verification fails:
- mark restore failed with `failed_reason=integrity_check_failed`
- do not attach this volume to workloads
- cleanup the partially restored LV (see cleanup rules)

### Step 7: mark volume available and restore succeeded
- set V2 state to `available`
- set restore status to `succeeded` and record `new_volume_id`

## Cleanup rules
If restore fails after provisioning the new LV:
- best effort delete the LV to avoid leaking capacity
- if cleanup fails, mark the new volume as `error` with reason, and surface it to operators for cleanup

If restore succeeds:
- no special cleanup is required beyond normal volume lifecycle

## Idempotency
Restore endpoints must support Idempotency-Key.

Dedup key:
- `(org_id, snapshot_id, idempotency_key, endpoint_name="restore")`

If duplicate request arrives:
- return the existing restore job and new_volume_id (even if still running)

If key reused with different request:
- reject with conflict.

## Attachment update behavior (restore by itself does not reattach)
Restore produces a new volume. It does not automatically modify attachments unless explicitly requested.

v1 recommended workflow:
- restore produces V2
- user (or operator) attaches V2 to the env/process at the desired mount_path
- user detaches V1 attachment
- scale process back up

Reason:
- prevents accidental cutover without user intent
- supports explicit rollback (re-attach old volume if needed)

Because attachments are unique by `(env, process_type, mount_path)`, replacing an attachment is effectively a two-step action in v1:
1) detach old attachment (requires process scaled to 0 or drained)
2) create new attachment pointing to V2 with the same mount_path

If you want to make this safer and less error-prone later:
- introduce an atomic “replace attachment” API that validates and performs both changes as one command and one set of events.

## Migration (v1 restore-based)
There is no live migration in v1. Migration is an operational workflow.

### Migration use cases
- move stateful workload to a different host
- evacuate a host before maintenance
- recover from host degradation
- move closer to edge region (future)

### Migration preconditions
- workload must tolerate downtime
- you have a recent successful backup (or you are willing to accept data loss since last backup)
- you can scale the process type to 0 or drain it cleanly

### Migration steps (recommended)
1) Drain workload:
- set desired scale for that process type to 0
- wait for instances to stop (or force stop if necessary)

2) Snapshot and backup:
- create a snapshot
- wait until snapshot (and backup) succeeded

3) Restore to destination node:
- run restore, optionally specifying target_node_id
- wait until restore succeeded and volume V2 is available

4) Swap attachment:
- detach old attachment to V1 (or delete it)
- attach V2 at the same mount_path

5) Restart workload:
- set desired scale back to previous value
- verify health checks and application behavior

6) Optional cleanup:
- keep old volume V1 for a cooling period (recommended)
- delete V1 only after validation

### Downtime expectations
Downtime includes:
- time to drain and stop workload
- snapshot and backup time
- restore time
- time to restart and pass health checks

This is a product reality in v1 and must be communicated clearly.

## Disaster recovery for node loss (stateful workloads)
If a home node is lost and does not return:
1) identify the latest successful snapshot backup
2) restore to a new node
3) replace attachment to new volume
4) restart workload

RPO:
- time since last successful backup

RTO:
- restore duration plus restart duration

## Events and views mapping
This spec expects these event types:
- `restore_job.created`
- `restore_job.status_changed`
- `volume.created` (for the new volume) or equivalent
- attachment create/delete events when you reattach

Views:
- `restore_jobs_view`
- `volumes_view`
- `volume_attachments_view`

The restore job record must reference:
- snapshot_id
- source_volume_id
- new_volume_id (on success)
- status and failure reason

## Failure reasons (recommended categories)
- `snapshot_not_found`
- `snapshot_not_succeeded`
- `backup_metadata_missing`
- `backup_object_missing`
- `master_key_unavailable`
- `node_not_eligible`
- `no_capacity`
- `download_failed`
- `decrypt_failed`
- `write_failed`
- `integrity_check_failed`
- `fsck_failed`
- `cleanup_failed`
- `internal_error`

Failure reasons must not include host filesystem paths or credentials.

## Observability requirements
Metrics:
- restore count and duration
- bytes restored
- success and failure counts by reason
- restore queue depth per node
- time since last successful restore test (operator drill metric)

Logs:
- include restore_id, snapshot_id, new_volume_id, node_id
- record step transitions (download start, decrypt start, write start, verify start)
- never log plaintext data, keys, or credentials

Alerts:
- repeated restore failures
- restore queue backlog growing
- restore duration outliers (backup store performance issues)
- master_key unavailable (operator issue)

## Compliance tests (required)
1) End-to-end restore:
- create volume, write data, snapshot+backup, restore, mount restored volume, verify data.

2) Integrity:
- corrupt one ciphertext chunk, restore must fail with integrity_check_failed.

3) Idempotency:
- repeat restore request with same idempotency key returns same restore_id and new_volume_id.

4) Capacity:
- restore fails cleanly when destination node has insufficient volume pool capacity.

5) Attachment swap workflow:
- scale down, detach old, attach new, scale up, verify workload uses restored data.

## Open questions (future)
- Incremental restore and faster migration (not v1).
- Atomic attachment replacement API (recommended future improvement).
- Live migration (explicitly not v1).

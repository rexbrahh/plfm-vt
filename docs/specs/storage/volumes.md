# docs/specs/storage/volumes.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
Define persistent volumes in the platform:
- lifecycle and states
- attachment rules and exclusivity
- locality constraints (local volumes)
- how volumes interact with scheduling, runtime mounts, and backups
- validation rules and failure semantics

Locked decision: storage is local volumes with asynchronous backups. See `docs/ADRs/0011-storage-local-volumes-async-backups.md`.

## Scope
This spec defines the control plane semantics of volumes and attachments.

This spec does not define:
- how volumes are mounted inside the microVM (`docs/specs/runtime/volume-mounts.md`)
- how snapshots are created (`docs/specs/storage/snapshots.md`)
- how backups are uploaded and encrypted (`docs/specs/storage/backups.md`)
- restore and migration flows (`docs/specs/storage/restore-and-migration.md`)

## Definitions
- **Volume**: a persistent block device managed by the platform.
- **Local volume**: a volume that physically resides on one node (host).
- **Attachment**: an env-scoped binding of a volume to `(env_id, process_type, mount_path, read_only)`.
- **Mount**: the guest-side mount of an attached volume inside the microVM.

## High-level stance (v1)
1) Volumes are block devices, not shared filesystems.
2) Volumes are local to a node.
3) A volume can be attached to at most one running instance at a time in v1 (exclusive writer).
4) Scheduler must respect locality constraints: instances requiring a volume run only on the node where the volume resides.
5) Backups exist, but they are asynchronous and do not provide synchronous failover.

## Volume properties (v1)
A volume has:
- `volume_id` (opaque id)
- `org_id` (owner tenant)
- `name` (optional, unique per org if present)
- `size_bytes`
- `filesystem` (v1: ext4 only)
- `backup_enabled` (bool)
- `home_node_id` (the node where the volume physically lives)
- `state` (see below)
- timestamps and resource_version

### Important: home_node_id requirement
Because volumes are local, every volume must have a home node for scheduling correctness.

This implies one of these must be true (choose and implement consistently):
- volume creation includes choosing a node and setting home_node_id immediately, or
- volume creation produces an unplaced volume and a later provisioning step sets home_node_id

v1 recommendation:
- choose home_node_id at volume creation time (simplest).
- this requires the event catalog to include home_node_id (either in `volume.created` payload or via a `volume.provisioned` event). If it is not currently present, update `docs/specs/state/event-types.md` accordingly.

## Volume states (v1)
Volumes are modeled as a state machine.

State enum:
- `creating`
- `available`
- `attaching`
- `in_use`
- `detaching`
- `deleting`
- `deleted`
- `error`

State meanings:
- creating: provisioning on the home node has not completed
- available: volume exists and can be attached
- attaching: attachment operation in progress
- in_use: currently attached to a running instance
- detaching: detachment in progress
- deleting: deletion requested
- deleted: terminal state
- error: broken state requiring operator intervention (provisioning failure, corruption, etc)

State transitions are driven by events and by agent reports. In v1 you can keep this simple by materializing state in views and relying on agent status.

## Volume lifecycle
### Create
Creation is requested by tenant (API/CLI).

Validation:
- `size_bytes >= 1Gi`
- `filesystem == ext4` (v1 only)
- org quota checks (max volumes, max total bytes)

Create semantics:
- assign `volume_id`
- select `home_node_id`
- provision volume on that node (how provisioning runs is operator choice, but the state must become available)

v1 provisioning recommendation:
- use an LVM thin pool on the home node and create an LV for the volume.
- ensure it is formatted ext4 at provisioning time.

### Delete
Deletion is requested by tenant (API/CLI) and is constrained by safety rules.

v1 rules:
- A volume cannot be deleted if it has an active attachment in use, unless forced by an operator-only action.
- Deleting a volume does not delete existing backups automatically unless retention policy says so (see backups spec).

Safety recommendation:
- implement soft delete in views, then run an asynchronous cleanup job that:
  - detaches if needed (operator-only)
  - deletes the LV on the home node
  - marks volume deleted

## Attachments (env-scoped)
Attachments bind a volume to an environment and process type.

Attachment fields:
- `attachment_id`
- `org_id`
- `app_id`
- `env_id`
- `process_type`
- `volume_id`
- `mount_path`
- `read_only`

### Attachment validation
- env must belong to app and org
- process_type must exist in the env’s desired release manifest
- mount_path must pass mount path constraints (see runtime volume-mounts spec)
- uniqueness:
  - `(env_id, process_type, mount_path)` must be unique among active attachments
- exclusivity:
  - a volume cannot be actively attached to multiple running instances (v1)

### Attachment semantics
- Attachment creation does not necessarily attach the volume immediately.
- Instead, it creates a constraint:
  - scheduler must ensure instances for that env/process run on the volume’s home_node_id
  - agent attaches the volume device when booting instances (or during rolling changes)

In practice:
- if `scaling.min > 0` and an attachment is created, the next scheduler reconciliation will place instances on the home node and they will mount the volume.

### Detach semantics
Detaching an attachment:
- removes the constraint for future instances
- may require draining existing instances first
- v1 recommended behavior:
  - if the attachment is currently in use, reject detach unless the user first scales that process type to 0 or triggers a drain action
  - operator-only force detach exists for emergencies

## Locality constraints and scheduling
### Placement rule (mandatory)
If a process type requires one or more volumes:
- all instances for that process type must be scheduled on the intersection of the home nodes for those volumes

v1 simplification:
- volumes attached to one process type should all be on the same home node.
- if a user attaches volumes with different home nodes to the same process type, reject it.

Reason:
- otherwise the intersection is empty and scheduling becomes impossible.

### Failure model
If home node is down:
- stateless processes can reschedule elsewhere
- stateful processes requiring local volumes cannot run until:
  - home node returns, or
  - volume is restored to a new node (restore flow)

This must be explicit in product and docs.

## Runtime mount mapping
The control plane and scheduler resolve attachments into WorkloadSpec mounts.
The host agent then attaches devices and guest init mounts them.

See:
- `docs/specs/manifest/workload-spec.md` for mounts fields
- `docs/specs/runtime/volume-mounts.md` for device ordering and mount rules

## Concurrency and exclusivity (v1)
### Exclusive writer
A volume is single-attach at a time in v1.

Enforcement points:
- control plane rejects conflicting active attachments that would create multi-writer use
- scheduler does not place multiple instances that require the same volume concurrently
- agent refuses to attach a volume already in use and reports `volume_attach_failed` with reason_detail `busy_or_already_attached`

### Multi-reader
Multi-reader read-only sharing is not supported in v1. If introduced later, it requires an explicit design and likely a new ADR.

## Observability requirements
Control plane must surface:
- volume state
- home_node_id
- active attachments
- last backup time and status (if backup enabled)

Agent must emit metrics:
- volume attach latency
- attach failures by reason_detail
- volume pool usage (thin pool usage)
- disk pressure alerts

## Events and views mapping
This spec assumes the event types exist as described in `docs/specs/state/event-types.md`:
- `volume.created`, `volume.deleted`
- `volume_attachment.created`, `volume_attachment.deleted`

Additionally, because locality is required, the event catalog must carry `home_node_id` either:
- in `volume.created`, or
- in a follow-up `volume.provisioned` event

Materialized views:
- `volumes_view`
- `volume_attachments_view`

## Open questions (v1 decisions to confirm)
- Whether volume creation always chooses a home node (recommended), or supports unplaced volumes (not recommended for v1).
- Whether detaching an in-use volume is rejected (recommended) or triggers automatic drain (possible, but riskier).

## Implementation plan

### Current code status
- **Volume events**: Event types defined in `docs/specs/state/event-types.md`.
- **Volume views**: Schema defined; materialization not implemented.
- **Node agent volume handling**: Placeholder exists in agent actors; device attach not implemented.

### Remaining work
| Task | Owner | Milestone | Status |
|------|-------|-----------|--------|
| Volume resource API endpoints | Team Control | M1 | Not started |
| Volume creation with home_node_id selection | Team Control | M1 | Not started |
| LVM thin pool provisioning on nodes | Team Runtime | M3 | Not started |
| Volume attachment API and validation | Team Control | M1 | Not started |
| Scheduler locality constraint enforcement | Team Control | M1 | Not started |
| Agent device attach and WorkloadSpec mounts | Team Runtime | M3 | Not started |
| Guest init volume mount handling | Team Runtime | M3 | Partial |
| Volume state machine transitions | Team Control | M1 | Not started |
| Exclusive writer enforcement | Team Control | M1 | Not started |
| Volume deletion safety checks | Team Control | M1 | Not started |
| Metrics: attach latency, pool usage, failures | Team Runtime | M7 | Not started |

### Dependencies
- Scheduler placement spec must enforce volume locality.
- Guest init must handle volume mount paths from WorkloadSpec.
- Event types must include `home_node_id` in volume creation.

### Acceptance criteria
1. Volume creation selects a home node and provisions storage.
2. Attachments bind volumes to env/process with mount path.
3. Scheduler places instances only on volume home node.
4. Agent attaches device and guest init mounts at specified path.
5. Concurrent attach to same volume fails with `volume_in_use`.
6. Deleting attached volume fails unless forced by operator.
7. Volume state transitions are reflected in views within 5 seconds.

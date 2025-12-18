# docs/specs/runtime/volume-mounts.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
Define how persistent volumes are attached and mounted into Firecracker microVMs:
- device attachment contract between host agent and guest
- mount path constraints
- filesystem assumptions (v1: ext4)
- read-only semantics
- failure behavior and reason codes

Locked decision: persistent storage is local volumes with async backups. See `docs/ADRs/0011-storage-local-volumes-async-backups.md`.

This runtime spec is the “how it is mounted” layer.
The “what volumes exist and how they are created/backed up” is in:
- `docs/specs/storage/*`

## Scope
This spec defines:
- block device mapping and mount conventions
- guest init responsibilities for mounting
- host agent responsibilities for attaching

This spec does not define:
- volume lifecycle, locality, backup and restore (`docs/specs/storage/*`)
- scheduler placement rules (`docs/specs/scheduler/*`)
- firecracker boot basics (`docs/specs/runtime/firecracker-boot.md`)

## Definitions
- **Volume**: a persistent local storage unit associated with a host.
- **Attachment**: a binding of a volume to `(env, process_type, mount_path)` that constrains scheduling.
- **Mount**: the actual filesystem mount inside the guest at a mount_path.

## High-level contract (v1)
1) Volumes are presented to the microVM as **virtio-blk block devices**.
2) v1 supports **ext4** filesystems only.
3) The guest init mounts volumes at explicit mount paths from WorkloadSpec.
4) Volume mounts must never target reserved system paths.
5) A volume may be attached to at most one microVM at a time in v1 (no multi-writer support).

## WorkloadSpec mount fields (normative)
The host agent receives mount requirements in WorkloadSpec:
- `mounts[]` each with:
  - `volume_id` (required)
  - `mount_path` (required, absolute)
  - `read_only` (default false)
  - `filesystem` (v1: ext4)
  - optional `device_hint` (agent-internal)

These mounts are derived from control plane attachments, not tenant input directly.

## Host agent responsibilities
### 1) Ensure the volume exists locally
Before attaching a volume to a microVM, the agent must ensure:
- the volume block device exists on the host
- it is not already attached to another running microVM (exclusive attachment)

If the volume is not present locally:
- this is a control plane placement bug (scheduler violated locality), but the agent must still fail safely:
  - fail instance start with `volume_attach_failed`
  - report clear reason_detail: `volume_not_present_on_node`

### 2) Attach the volume block device to Firecracker
The agent attaches volume devices after root and scratch disks.

Device ordering in the microVM:
- `vda`: root disk (read-only)
- `vdb`: scratch disk (read-write)
- `vdc...`: volume devices in deterministic order, sorted by `volume_id` to ensure repeatability

Each volume attachment includes:
- host path to block device (LV path or loop device)
- read-only flag when requested

Read-only behavior:
- If `read_only=true`, attach the device as read-only at the Firecracker level.
- Guest init must still mount it read-only for defense-in-depth.

### 3) Ensure filesystem exists (creation time only)
Filesystem creation is a lifecycle concern, but the agent must enforce a v1 minimum:
- A newly created volume must be formatted ext4 before first mount.

v1 rule:
- Formatting happens at volume provisioning time (control plane or agent provisioning path), not at mount time.
- If a volume is unformatted or has wrong filesystem, fail mount with `volume_attach_failed` and reason_detail `filesystem_mismatch`.

### 4) Provide mount mapping to the guest
Guest init needs to know which block device corresponds to which mount.

v1 approach:
- The WorkloadSpec mount list is provided to guest init via the vsock config handshake.
- Guest init discovers block devices and matches them by deterministic ordering rather than by host path.

Deterministic mapping rule (normative):
- The Nth mount in sorted order maps to the Nth attached volume device starting at `vdc`.
- Sorted order is ascending by `volume_id`.

Example:
- mount list sorted: `[volA, volC, volD]`
- devices: `vdc=volA`, `vdd=volC`, `vde=volD`

This avoids needing stable in-guest device identifiers beyond ordering.

## Guest init responsibilities
Guest init must:
1) Ensure root overlay is prepared and pivoted.
2) Ensure secrets file is written.
3) Mount volumes as specified.

Mount procedure for each mount:
- validate mount_path is absolute and allowed
- ensure mount_path exists (create directory if needed)
- discover the corresponding block device (by mapping rule above)
- mount with filesystem type ext4
- apply read-only options if requested

Mount options (recommended):
- default: `defaults,noatime`
- read-only: `ro,defaults,noatime`

Directory creation:
- If mount_path directory does not exist, create it with `0755` unless a stricter policy is specified later.
- Do not follow symlinks when creating mount directories (avoid mount path traversal).

If any required mount fails:
- guest init must exit non-zero
- the agent reports `volume_attach_failed`

## Mount path constraints (normative)
Mount path must:
- be an absolute path
- not be:
  - `/proc` or any subpath
  - `/sys` or any subpath
  - `/dev` or any subpath
  - `/run` or any subpath
  - `/run/secrets` or any subpath
  - `/tmp` or any subpath
- not be the root directory `/`
- not contain `..` segments after normalization

The control plane should validate these constraints at attachment creation time, but guest init must enforce them too.

## Exclusive attachment invariant
v1 invariant:
- A volume is attached to at most one microVM at a time.

Enforcement points:
- Control plane must reject creating two active attachments that would mount the same volume simultaneously.
- Scheduler must not place two instances that require the same volume concurrently.
- Host agent must refuse to attach if it detects the device is already in use.

## Unmount and detach semantics
When an instance transitions to draining or stopped:
- guest init receives SIGTERM and may shut down the workload gracefully.
- The agent terminates the microVM after grace period if needed.
- Host agent detaches the volume device when the microVM is fully stopped.
- No attempt is made to live-migrate attached volumes in v1.

## Failure handling and reason codes
Mount failures must be reported as structured reasons.

Primary reason code:
- `volume_attach_failed`

Required reason_detail (one of):
- `volume_not_present_on_node`
- `device_attach_failed`
- `filesystem_mismatch`
- `mount_path_invalid`
- `mount_failed`
- `read_only_attach_failed`
- `busy_or_already_attached`

These map into `instance.status_changed` events.

## Observability requirements
Agent metrics:
- volume attach success/failure counts
- attach latency
- count of active attachments
- per-node volume pool usage (belongs in storage spec but surfaced here)

Agent logs must include:
- instance_id
- volume_id
- mount_path
- failure detail (no sensitive data)

## Compliance tests (required)
Automated tests must validate:
1) A volume device attaches as `vdc` and mounts at the desired path.
2) Read-only mount denies writes inside the guest.
3) Invalid mount paths are rejected by guest init.
4) Concurrent attachment attempt fails safely.
5) A stopped instance releases the volume so another instance can attach later.

## Open questions (future)
- Whether to support per-volume encryption at rest (host-level or guest-level).
- Whether to support incremental snapshots exposed to tenants.
- Whether to support read-only shared mounts (multi-reader) in a later version.

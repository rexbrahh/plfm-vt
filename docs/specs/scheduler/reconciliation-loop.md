# docs/specs/scheduler/reconciliation-loop.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
Define the scheduler reconciliation loop:
- how desired instances are computed from control plane state
- how rolling updates are executed
- how instance allocations and desired-state transitions are emitted
- how failures and unschedulable states are handled
- how the scheduler stays idempotent and deterministic

Placement constraints are defined in:
- `docs/specs/scheduler/placement.md`

Drain and reschedule mechanics are defined in:
- `docs/specs/scheduler/drain-evict-reschedule.md`

The scheduler-to-agent contract is defined in:
- `docs/specs/workload-spec.md`

## Scope
This spec defines:
- scheduler inputs and derived desired state
- reconciliation algorithm per (env, process_type)
- rollout strategies (stateless vs volume-attached)
- decision rules for creating, draining, and stopping instances
- observability and safety requirements

This spec does not define:
- exact event schemas (see `docs/specs/state/event-types.md`)
- projection mechanics (see `docs/specs/state/materialized-views.md`)
- quota enforcement details (see `docs/specs/scheduler/quotas-and-fairness.md`)

## Definitions
- **Desired group**: the desired runtime configuration for an (env, process_type) at a point in time, summarized by a deterministic `group_spec_hash`.
- **Group spec hash**: a hash representing all runtime-relevant inputs for instances of that group (release, vars, resources, mounts, secrets version, health).
- **Active instance**: an instance whose desired_state is running or draining and has not reached terminal stopped state.
- **Terminal instance**: an instance whose desired_state is stopped and is not expected to run again.
- **Stateless process type**: a process type with no volume mounts.
- **Stateful process type**: a process type with one or more volume mounts (local volumes).

v1 stance:
- One process type per microVM instance.
- Any process type with volume mounts must have desired replicas <= 1 (recommended and enforced by scheduler in v1).

## Inputs to reconciliation (v1)
Scheduler reads from materialized views:

Required:
- `env_desired_releases_view` (desired release per process type)
- `env_scale_view` (desired replica counts)
- `releases_view` (release metadata)
- `secret_bundles_view` (current secret version per env, if any)
- `volume_attachments_view` (env/process mounts)
- `volumes_view` (must include home_node_id)
- `nodes_view` (state and allocatable capacity)
- `instances_desired_view` (current desired instances and assignments)
- `instances_status_view` (agent-reported status, readiness)

Optional derived helper views (recommended):
- `route_backends_view` (to drive edge, not required for scheduler itself)
- `env_status_view` (to surface unschedulable reasons)

Cluster config inputs:
- cpu_overcommit_ratio
- vmm_overhead_bytes_per_instance
- rollout knobs (max surge, max unavailable)
- timeouts (startup timeout, drain timeout)

## Scheduler loop structure
There are two layers:

1) **Global loop**
- runs continuously or on a short periodic tick (example 1s to 5s)
- identifies which (env, process_type) groups need reconciliation
- calls per-group reconcile

2) **Per-group reconcile**
- computes desired group spec
- compares to current instances
- emits the minimal set of events needed to converge

### Triggering reconciliation
Reconcile is triggered by any relevant change, including:
- env.desired_release_set
- env.scale_set
- secret_bundle.version_set (rotation)
- volume_attachment.created or deleted
- node state or capacity changes
- instance.status_changed (failures, readiness)
- node offline detection

v1 requirement:
- scheduler must also run periodic full reconciliation (example every 30 seconds) to correct missed triggers.

## Desired group computation
For each (env_id, process_type):

### Step 1: determine desired release
- Read `release_id` from `env_desired_releases_view`.
- If no desired release exists:
  - desired replicas for this group is treated as 0.
  - scheduler should not place instances.

### Step 2: determine desired replicas
- Read desired replica count from `env_scale_view`.
- If missing, use the manifest-derived default:
  - if the env has exactly one process type, default desired = 1
  - otherwise default desired = 0
(These defaults match `docs/specs/manifest/manifest-schema.md`.)

### Step 3: resolve secrets binding
- If the env has a secret bundle:
  - use `current_version_id` as desired secrets version for the group
- If the process type requires secrets (manifest `secrets.required=true`) and env has no bundle:
  - group is unschedulable with reason `secrets_missing`

### Step 4: resolve volume mounts
- Read mounts from `volume_attachments_view` for (env_id, process_type).
- If mounts exist:
  - classify group as stateful
  - enforce desired replicas <= 1 (v1 rule)
  - ensure all mounts refer to volumes whose home_node_id matches (v1 rule)

If these constraints fail:
- group is unschedulable with reason `volume_constraints_invalid`

### Step 5: resolve runtime inputs that affect spec hash
Runtime inputs that must influence the group_spec_hash include:
- release_id
- manifest_hash for that release
- resolved command and workdir
- env vars (env-level + process-level)
- resources (cpu_request, memory_limit_bytes, ephemeral_disk_bytes)
- health check config
- mount list (volume_id, mount_path, read_only)
- secrets_version_id (or explicit “no secrets” marker)

The scheduler does not need to compute the full WorkloadSpec itself if a separate WorkloadSpecBuilder exists, but the group_spec_hash must be computed deterministically from the same inputs the builder will use.

### Step 6: compute group_spec_hash
- Use a canonical JSON representation of the inputs and hash it (sha256).
- Canonicalization rules:
  - stable key ordering
  - stable list ordering
    - mounts sorted by volume_id then mount_path
    - env vars sorted by key
- Store the resulting hash string as group_spec_hash.

## Instance classification for a group
For each group (env_id, process_type), partition instances in `instances_desired_view`:

- `desired_state=running` instances
- `desired_state=draining` instances
- `desired_state=stopped` instances (terminal)

Also track each instance’s `spec_hash` (stored on instance.allocated event payload and in view).

Define:
- `instances_current` = instances where desired_state in {running, draining}
- `instances_matching` = instances_current where spec_hash == group_spec_hash
- `instances_old` = instances_current where spec_hash != group_spec_hash

Read runtime status from `instances_status_view`:
- ready, booting, failed, stopped, draining

This status drives rollout progress and replacement decisions.

## Rollout strategies (v1)
There are two rollout strategies based on statefulness.

### Strategy A: stateless rolling (no volumes)
Applies when the group has no mounts.

Defaults (v1 recommended):
- max_surge = 1
- max_unavailable = 0 for desired_replicas > 1
- for desired_replicas == 1:
  - allow surge 1 if capacity allows, otherwise allow a brief unavailable window by draining first

Algorithm (high level):
1) Ensure at least desired_replicas matching instances exist.
2) If there are old instances:
   - create new matching instances up to max_surge (or until matching count reaches desired_replicas)
   - wait for new instances to become ready
   - then drain old instances one by one until only matching remain
3) If matching count exceeds desired_replicas:
   - drain extras

### Strategy B: stateful replace-in-place (volumes attached)
Applies when the group has one or more mounts.

v1 constraints:
- desired_replicas must be 1 (enforced by scheduler). If user sets higher, scheduler must reject the scale change or mark group unschedulable.

Because volumes are exclusive:
- do not create a new instance while the old instance is running if it would require the same volume device.

Algorithm:
1) If there is exactly one matching running instance and it is ready:
   - done
2) If spec hash changed (release, secrets version, mounts, resources, env vars):
   - drain the current running instance
   - wait until it reaches stopped (or force stop after drain timeout)
   - then create a new matching instance on the same home node of the volume(s)
3) If the instance fails to boot repeatedly:
   - stop and surface failure
   - do not thrash endlessly

This implies downtime during updates for stateful workloads in v1. This is intentional and must be reflected in product docs.

## Reconciliation actions and emitted events
Scheduler acts by appending events. It never edits state directly.

### Allocate new instance
When scheduler decides to create a new instance:
1) Choose node using placement rules (see placement.md).
2) Allocate overlay_ipv6 from IPAM.
3) Emit `instance.allocated` with:
   - instance_id
   - node_id
   - env_id, app_id, org_id, process_type
   - release_id
   - secrets_version_id (if any)
   - overlay_ipv6
   - resources snapshot
   - spec_hash (group_spec_hash)

Instance_id generation:
- v1 recommendation: ULID or UUIDv7.

### Drain an instance
When scheduler decides to remove capacity or replace an old instance:
- Emit `instance.desired_state_changed` with:
  - desired_state = draining
  - drain_grace_seconds (default 10)
  - reason (scale_down, rollout_replace, node_draining, node_offline)

### Stop an instance
When scheduler needs the instance gone (after drain timeout or emergency):
- Emit `instance.desired_state_changed` with desired_state = stopped.

v1 note:
- Stopped is terminal. A stopped instance_id is not reused.

### No in-place spec mutation
v1 rule:
- Scheduler does not change release_id, mounts, or secrets version for an existing instance_id.
- Any spec change is represented by creating a new instance_id (stateless) or draining and then creating a new instance_id (stateful).

This keeps agent behavior simple and makes audit history clearer.

## Handling scale changes
### Scale up
If desired_replicas increases:
- allocate additional matching instances until matching running count == desired_replicas.

### Scale down
If desired_replicas decreases:
- select instances to drain in deterministic order.

Drain selection order (v1):
1) instances in failed state
2) instances not ready
3) instances with oldest created_at (or lexicographically smallest instance_id if created_at not available)
4) instances on the most loaded nodes (optional soft preference)

Then drain until matching desired count is achieved.

## Handling release changes (deploy and rollback)
A deploy causes env.desired_release_set changes per process type.

On release change:
- group_spec_hash changes
- scheduler begins rollout strategy based on statefulness

Deploy status updates:
- A deploy controller (may be part of scheduler service) should update `deploy.status_changed` based on observed convergence:
  - queued -> rolling when env.desired_release_set applied
  - rolling -> succeeded when all targeted process types have:
    - matching instances count == desired replicas
    - instances are ready
    - no old instances remain (or old instances are draining and have no traffic, depending on policy)
  - rolling -> failed if a timeout or repeated failures exceed thresholds

Timeouts (v1 recommendation):
- per process type rollout timeout: 10 minutes (configurable)
- per instance startup timeout: derived from health grace, default 2 minutes

## Handling secrets rotation
Secret rotation changes `secret_bundle.current_version_id`.

v1 rule:
- secrets change triggers restart semantics.
- this is implemented as a spec hash change, which triggers rollout.

Stateless:
- rolling replacement following Strategy A

Stateful (volumes):
- replace-in-place following Strategy B (downtime expected)

If secrets are required and missing:
- scheduler marks group unschedulable and does not place instances.

## Handling volume attachment changes
Mount set changes are spec hash changes.

Because volumes are exclusive and local:
- treat as stateful Strategy B.
- enforce desired replicas <= 1.

Additionally:
- if a mount references a volume on a different home node than existing mounts, mark unschedulable.
- if a mount is removed, new instances start without it after replacement.

## Handling node state changes
Node state changes affect placement and rescheduling.

### Node draining
- Scheduler does not place new instances on draining node.
- Existing stateless instances on draining node are evicted:
  - mark desired_state draining, then allocate replacements elsewhere
- Stateful instances with local volumes:
  - cannot be rescheduled automatically unless a restore-based migration is initiated.
  - scheduler should surface env degraded status and require operator action.

### Node offline or disabled
- For stateless instances on the node:
  - allocate replacements on other nodes
  - mark old instances desired_state stopped (or draining then stopped if reachable is unknown)
- For stateful instances:
  - surface degraded and require restore to a new node.

## Handling instance failures
Instances can fail to boot or can crash.

Inputs:
- `instance.status_changed` events with status failed and reason codes.

v1 failure handling rules:
- If a new instance fails to reach ready within startup timeout:
  - mark it draining or stopped (deterministic)
  - create a replacement instance (subject to retry limits)

Retry limits (v1 recommendation):
- max 3 replacement attempts per group per deploy_id within 10 minutes
- after that, mark deploy failed and surface reason

Node de-prioritization:
- if a node causes repeated firecracker_start_failed or disk_full issues, lower its score for placements (soft constraint).

Crash loop handling:
- agent may restart inside its restart policy.
- if agent reports repeated crash_loop_backoff, scheduler may decide to stop the instance and surface failure rather than thrash.

## Idempotency and determinism requirements
Scheduler must be safe under:
- duplicate events
- restarts
- concurrent reconciliation attempts (if multiple scheduler workers)

Normative requirements:
1) A reconciliation pass must not emit duplicate instance.allocated events for the same desired need.
2) Allocation decisions must be deterministic given the same inputs.
3) All writes are appended as events, in transactions, with request ids for traceability.

Recommended implementation technique:
- Use a per-group reconciliation lock in Postgres (advisory lock keyed by env_id + process_type) to avoid two workers allocating concurrently for the same group.

## Derived outputs for agents (NodePlan)
Agents need a node-scoped plan of desired instances.

v1 rule:
- NodePlan is derived from `instances_desired_view` and related views (release, secrets, mounts), not stored as a giant event payload.

Requirements:
- NodePlan generation must be deterministic.
- When instances_desired_view changes for a node, the agent must be able to observe and apply those changes.

The exact transport is defined in `docs/specs/workload-spec.md`.

## Observability requirements
Scheduler must emit metrics:
- reconcile loop duration
- reconcile runs per env/process
- allocation counts
- drain counts
- unschedulable counts by reason
- rollout durations and outcomes
- retry counts and failure categories

Scheduler logs must include:
- env_id, process_type, deploy_id if applicable
- group_spec_hash
- chosen node ids for allocations
- reasons for unschedulable states

## Compliance tests (required)
1) Stateless scale up and scale down produces correct instance allocations and drains.
2) Deploy changes release_id and results in rolling replacement with no wrong-tenant routing.
3) Secrets rotation triggers rollout restart and results in matching secrets_version_id in new instances.
4) Stateful process with volume:
- replicas > 1 is rejected or marked unschedulable
- release change drains then starts new instance on the volume home node
5) Node draining evicts stateless instances and does not schedule new work onto that node.
6) Node offline causes stateless reschedule and stateful degraded status.
7) Scheduler restart does not allocate duplicate instances.

## Open questions (explicitly deferred)
- Whether to introduce a dedicated “workload set” aggregate for (env, process_type) to track rollout state explicitly.
- Whether to support max_surge and max_unavailable knobs per process type in the manifest (not v1).

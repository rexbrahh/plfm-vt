# docs/specs/scheduler/placement.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
Define scheduler placement rules:
- what inputs the scheduler uses
- what constraints are hard vs soft
- how node capacity is modeled
- how volumes constrain placement (local volumes)
- how networking constraints influence placement
- how the scheduler chooses nodes deterministically

Locked decisions this depends on:
- CPU is soft, memory is hard-capped: `docs/ADRs/0012-scheduling-cpu-oversubscribe-mem-hardcap.md`
- Storage is local volumes + async backups: `docs/ADRs/0011-storage-local-volumes-async-backups.md`
- MicroVM per instance and one process type per microVM instance: `docs/ADRs/0001-isolation-microvm-per-instance.md`
- IPv6-first, WireGuard overlay: `docs/ADRs/0004-overlay-wireguard-full-mesh.md`, `docs/ADRs/0007-network-ipv6-first-ipv4-paid.md`

This spec is authoritative for the scheduler’s placement decisions. Reconciliation mechanics are defined in `docs/specs/scheduler/reconciliation-loop.md`.

## Scope
This spec defines:
- placement constraints
- capacity accounting
- node eligibility rules
- tie-breaking and determinism rules

This spec does not define:
- event log mechanics (`docs/specs/state/*`)
- workload spec schema (`docs/specs/manifest/workload-spec.md`)
- drain/evict/reschedule workflow (`docs/specs/scheduler/drain-evict-reschedule.md`)
- quotas (`docs/specs/scheduler/quotas-and-fairness.md`)

## Definitions
- **Node**: host capable of running Firecracker microVMs.
- **Allocatable resources**: resources the scheduler is allowed to assign to workloads, excluding host-reserved overhead.
- **Instance**: one microVM running one process type.
- **Desired instance count**: desired replicas for (env, process_type).
- **Hard constraint**: must be satisfied for placement to be valid.
- **Soft constraint**: influences scoring but can be violated.
- **Placement**: assigning an instance_id to a node_id.
- **Home node**: node that hosts a local volume.

## Placement inputs (v1)
Scheduler reads from materialized views:
- env desired releases (`env_desired_releases_view`)
- env desired scales (`env_scale_view`)
- volume attachments (`volume_attachments_view`)
- volumes (`volumes_view`, must include home_node_id)
- nodes (`nodes_view`: state, allocatable capacity, labels)
- instance desired (`instances_desired_view`)
- instance status (`instances_status_view`) for readiness and lifecycle

Scheduler also needs cluster-level config:
- `cpu_overcommit_ratio` (float, default recommendation 4.0)
- `reserved_host_memory_bytes` (per node or cluster default)
- `vmm_overhead_bytes_per_instance` (default recommendation 64Mi)
- `default_instance_ephemeral_disk_bytes` (default 4Gi)
- optional placement spread knobs (see below)

## Node eligibility (hard)
A node is eligible for placement only if:
- node state is `active`
- agent is reachable recently enough (heartbeat freshness policy)
- node reports allocatable capacity > 0
- node has required runtime capabilities:
  - KVM available
  - sufficient disk for scratch
  - overlay connectivity is not degraded beyond policy threshold

v1 simplification:
- Treat node state `degraded` as ineligible for new placements unless an operator overrides.

## Hard constraints (v1)
These must be satisfied for placement.

### 1) Memory hard cap and allocatable memory
Memory is a hard constraint.

For a candidate node:
- `allocatable_memory_bytes` is reported by node capacity (or computed from physical - reserved).
- `used_memory_bytes` is the sum of memory caps of instances assigned to the node that are not in terminal stopped state.
- `instance_memory_bytes` is from WorkloadSpec resolution for the process type.

Constraint:
- `used_memory_bytes + instance_memory_bytes <= allocatable_memory_bytes`

Accounting rule:
- include per-instance overhead in used_memory_bytes:
  - `instance_memory_bytes_effective = instance_memory_bytes + vmm_overhead_bytes_per_instance`

Rationale:
- Avoid host instability. Prefer failing placement to oversubscribing memory.

### 2) Volume locality (local volumes)
If the process type requires attached volumes:
- all required volumes must have `home_node_id` equal to the candidate node id.

If multiple volumes are attached:
- they must all share the same home node (v1 rule).
- otherwise the attachment set is invalid and must be rejected at attachment creation time.

Constraint:
- `candidate_node_id == volume.home_node_id` for each required volume.

Rationale:
- volumes are local. There is no shared storage in v1.

### 3) Exclusive volume usage (single attach)
Because volumes are exclusive writer in v1:
- scheduler must not place two concurrently running instances that would attach the same volume.

Constraint:
- for any volume_id required by the instance:
  - there must be at most one non-draining, non-stopped instance assigned that uses it

Practical implementation:
- treat volume attachments as implying that all replicas of that process would use that volume, which means:
  - for stateful process types with a volume, desired replicas should generally be 1 in v1
  - if desired replicas > 1 and volume attached, reject scale or placement (choose one policy and document it)

v1 recommendation:
- enforce: if a process type has any volume attachments and any attachment is read_write, then max replicas for that process type is 1.
- allow multiple replicas only if all mounts are read_only (but multi-reader is not supported in v1). So keep it simple:
  - max replicas = 1 for any process type with volumes in v1.

This avoids accidental multi-writer corruption.

### 4) Required networking identity
Each instance must be assigned a unique overlay IPv6 address.

Constraint:
- scheduler must allocate `overlay_ipv6` from IPAM and ensure uniqueness.

Failure:
- if IPAM allocation fails, placement fails.

### 5) Port conflicts (edge is separate)
Within a microVM, port conflicts are app-level. The scheduler does not manage in-guest port conflicts.

At the platform level:
- public port bindings and hostnames are enforced in route validation, not scheduler placement.

Therefore:
- scheduler does not use ports as placement constraints in v1.

## Soft constraints and scoring (v1)
Soft constraints influence where we place but do not invalidate placement.

### 1) CPU utilization (soft)
CPU is oversubscribable. Still, we want to avoid obvious overload.

Define:
- `allocatable_cpu = physical_cpu_cores * cpu_overcommit_ratio`
- `used_cpu_request = sum(cpu_request of assigned instances)`
- `instance_cpu_request` from WorkloadSpec

Score preference:
- prefer nodes where `(used_cpu_request + instance_cpu_request)` is lower
- avoid nodes where used_cpu_request is far above allocatable_cpu, but do not treat as hard reject in v1

v1 recommended scoring:
- `cpu_score = 1 - min(1, (used_cpu_request / allocatable_cpu))`

### 2) Spread across nodes (anti-affinity)
To reduce blast radius:
- prefer spreading instances of the same (env, process_type) across different nodes when replicas > 1.

But if volumes exist, this is overridden by locality.

v1 rule:
- If replicas > 1 and no volume mounts:
  - prefer unique nodes per replica until capacity forces packing.

### 3) Avoid recent-failure nodes
If a node has recent repeated instance boot failures (firecracker failures, disk pressure):
- de-prioritize it for new placements.

This is soft because you may have no other nodes.

### 4) Edge proximity (optional future)
If you later have multiple edge regions:
- you can add latency-based scoring.
Not in v1.

## Determinism and tie-breaking (mandatory)
Schedulers that “randomize” produce hard-to-debug behavior.

v1 determinism rules:
1) Candidate node set must be computed deterministically from current views.
2) Node scoring must be deterministic.
3) If scores tie, pick the node with the smallest lexicographic node_id (or stable numeric ordering).

Instance ids must also be generated deterministically by the scheduler’s reconciliation loop (see reconciliation spec), not by the node agent.

## Instance identity generation (v1)
When scaling up from desired replicas:
- scheduler creates new `instance_id` values.

v1 recommendation:
- instance_id is a ULID (sortable) or UUIDv7.
- instance_id is stable for the lifetime of that desired instance slot.
- a restart does not change instance_id.

If you want a more deterministic mapping:
- derive instance_id from `(env_id, process_type, ordinal, deploy_generation)` but this can cause unpleasant collisions on scale changes. v1 recommendation is random ULID/UUIDv7.

## Placement algorithm (v1 recommended)
Given a desired set of instances to place:
1) Build candidate node list (eligible nodes).
2) Filter by hard constraints:
   - memory
   - volume locality
   - volume exclusivity
3) Score remaining nodes by soft constraints:
   - CPU pressure
   - spread
   - failure de-prioritization
4) Pick best node (highest score, tie-break by node_id).
5) Allocate overlay_ipv6 from IPAM.
6) Emit `instance.allocated` event with:
   - instance_id
   - node_id
   - overlay_ipv6
   - resources snapshot
   - release_id and secrets version id
   - spec_hash

## Handling unschedulable instances
If no node satisfies hard constraints:
- the scheduler must not “force” placement.
- it must mark the env/process as degraded in a derived status view and surface a clear reason:
  - `no_capacity_memory`
  - `volume_locality_no_node`
  - `no_nodes_active`
  - `ipam_exhausted`
  - `volume_in_use`

These reasons are not events by default, but they must appear in:
- env status view
- deploy status messages
- CLI output

Optionally, emit an event:
- `env.scheduling_failed` (future) if you want history.

## Interaction with secrets (placement implication)
Secrets are env-scoped.
Placement must ensure:
- if process requires secrets, the env has a secrets bundle and current_version_id.
- if secrets required but missing:
  - scheduler should not place instances
  - surface reason `secrets_missing`

This prevents endless crash loops.

## Interaction with drain and reschedule
Placement must respect node state:
- draining nodes receive no new placements.
- instances on draining nodes are evicted and rescheduled if stateless.
- for stateful workloads with local volumes:
  - draining requires explicit migration/restore plan.

Details are in `drain-evict-reschedule.md`.

## Open questions (to be resolved in reconciliation spec)
- How rolling deploys create new instances vs in-place generation bumps:
  - v1 recommendation: create new instances, drain old instances (blue/green per process type).
- Whether to allow process types with volumes to have replicas > 1:
  - v1 recommendation: forbid (max 1) unless you later design multi-reader or sharded volumes.

## Implementation plan

### Current code status
- **Scheduler skeleton**: Basic scheduler structure exists in `services/control-plane/src/scheduler/`.
- **Materialized views**: Core views (`nodes_view`, `instances_desired_view`) defined but not fully populated.
- **Instance allocation events**: Event schema exists; emission logic in progress.

### Remaining work
| Task | Owner | Milestone | Status |
|------|-------|-----------|--------|
| Node eligibility filtering (state, capacity, heartbeat) | Team Control | M1 | Not started |
| Memory hard cap enforcement | Team Control | M1 | Not started |
| Volume locality constraint enforcement | Team Control | M1 | Not started |
| Volume exclusivity validation (max 1 replica with volumes) | Team Control | M1 | Not started |
| IPAM integration for overlay_ipv6 allocation | Team Control | M1 | Not started |
| CPU soft scoring | Team Control | M1 | Not started |
| Anti-affinity spread scoring | Team Control | M1 | Not started |
| Deterministic tie-breaking (lexicographic node_id) | Team Control | M1 | Not started |
| Unschedulable instance reason surfacing | Team Control | M1 | Not started |
| Secrets-missing placement gate | Team Control | M5 | Not started |
| Scheduler reconciliation loop integration | Team Control | M1 | Not started |

### Dependencies
- Materialized views (`nodes_view`, `volumes_view`, `volume_attachments_view`) must be populated.
- IPAM service must be implemented for overlay address allocation.
- WorkloadSpec must include memory and CPU resource requirements.

### Acceptance criteria
1. Instances are placed only on nodes with sufficient memory.
2. Instances requiring volumes are placed only on the volume's home node.
3. Process types with volumes enforce max 1 replica.
4. IPAM allocates unique overlay IPv6 per instance.
5. Unschedulable instances surface clear reason codes in env status.
6. Placement is deterministic: same inputs produce same outputs.
7. Placement algorithm completes within 100ms for 100 instances.

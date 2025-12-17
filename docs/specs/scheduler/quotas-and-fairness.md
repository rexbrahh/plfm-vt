# docs/specs/scheduler/quotas-and-fairness.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
Define how the platform enforces:
- org-level quotas (hard ceilings)
- fairness (prevent one org from consuming the fleet)
- what happens when quota is exceeded
- where enforcement occurs (API validation vs scheduler vs runtime)

This spec is authoritative for quota semantics and enforcement points.

Locked decisions this depends on:
- CPU is soft, memory is hard-capped: `docs/adr/0012-scheduling-cpu-oversubscribe-mem-hardcap.md`
- Multi-tenancy and org boundary: `docs/architecture/05-multi-tenancy-and-identity.md`
- IPv6-first, IPv4 is paid add-on: `docs/adr/0007-network-ipv6-first-ipv4-paid.md`
- L4-first ingress: `docs/adr/0008-ingress-l4-sni-passthrough-first.md`

## Scope
This spec defines quotas and fairness for:
- instances and scheduling-related resources
- networking allocations that affect scheduling and edge capacity
- storage resources that constrain placement

This spec does not define:
- pricing and billing amounts (`docs/product/*`)
- detailed abuse policy (`docs/policy/abuse-and-rate-limits.md`)
- runtime CPU fairness inside a node (that is cgroup policy in runtime specs)

## Definitions
- **Org**: tenant boundary.
- **Quota**: a hard ceiling on a resource dimension for an org.
- **Usage**: current counted consumption for a resource dimension.
- **Reservation**: in-flight consumption that is not yet running but should count (example: allocated instances).
- **Hard enforcement**: request is rejected or desired state is clamped.
- **Fairness**: controls that prevent disproportionate resource capture by one org when cluster is shared.

## High-level stance (v1)
1) Quotas are the primary fairness mechanism in v1.
2) Runtime CPU shares provide per-node fairness, but they are not sufficient to prevent fleet capture. Quotas are required.
3) Memory is treated as the most important quota dimension because it is the hard cap and the primary safety boundary.
4) Quotas must be enforceable at the control plane command layer and also rechecked in scheduler reconciliation.
5) v1 does not implement complex fair-share scheduling across orgs. It enforces explicit quotas and uses simple deterministic placement.

## Quota dimensions (v1)
Quotas are defined per org. Defaults come from the service tier, but enforcement is mechanical.

### Compute quotas
- `max_instances` (int)
- `max_total_memory_bytes` (int)
- `max_total_cpu_request` (float)
- `max_envs` (int)
- `max_apps` (int)

### Networking quotas
- `max_routes` (int)
- `max_public_ports` (int)  
  Count ports exposed by routes, including raw TCP ports.
- `max_ipv4_allocations` (int)  
  Dedicated IPv4 add-on allocations (per env in v1).
- `max_hostnames` (int)  
  Typically equals max_routes, but separate dimension is allowed.

### Storage quotas
- `max_volumes` (int)
- `max_total_volume_bytes` (int)
- `max_volume_attachments` (int)
- `max_snapshots_per_volume` (int)  
  Optional in v1, but recommended to prevent abuse.
- `max_restore_jobs` (int)  
  Concurrency limit to prevent restore storms.

### Operational surface quotas (recommended)
These are not scheduler placement, but are part of fairness and platform stability:
- `max_log_streams` (int per org)
- `max_exec_sessions` (int concurrent)
- `max_exec_sessions_per_hour` (int)

These should be enforced in API layer, not scheduler.

## What counts as usage (v1 rules)
The control plane maintains a usage view per org.

### Instances usage
Count instances in `instances_desired_view` where desired_state is not `stopped`.
This includes:
- running
- draining
- booting
- failed but not yet stopped

Reason:
- draining and booting still consume capacity.
- failed instances must not be free until they are stopped and cleaned up, otherwise you can bypass quotas by failing repeatedly.

Instance memory usage:
- sum of `memory_limit_bytes + vmm_overhead_bytes_per_instance` for counted instances.

Instance cpu usage:
- sum of `cpu_request` for counted instances.

### Volume usage
Count volumes in `volumes_view` that are not deleted.
Volume bytes usage:
- sum of `size_bytes` for non-deleted volumes.

Attachments usage:
- count active attachments in `volume_attachments_view` that are not deleted.

### Route usage
Count routes in `routes_view` that are not deleted.

Public port usage:
- Count distinct `(listener_address_scope, listen_port)` bindings owned by the org.
In v1 where listeners are shared on IPv6:
- You can simplify to counting `listen_port` per route, but that undercounts raw TCP port exposure when you later add per-env IPv4 bindings. Prefer counting actual public bindings when available.

IPv4 allocations usage:
- count active IPv4 allocations in `env_networking_view` where ipv4_enabled is true.

### Apps and envs usage
Count apps and envs that are not deleted.

### Snapshots and restore jobs usage
Count in `snapshots_view` and `restore_jobs_view` with status in {queued, running} for concurrency limits.

## Where enforcement happens
Quotas must be enforced in two places.

### 1) API command validation (primary)
All user-triggered commands that increase usage must validate quotas before appending events.

Examples:
- create app, create env
- scale up
- deploy that introduces additional process types with default scaling
- create volume
- attach volume (if you want to cap attachments)
- create route
- enable IPv4 add-on
- create snapshot (if you cap concurrent snapshots)
- create restore job (if you cap concurrent restores)
- create exec session, start log stream (operational quotas)

If quota would be exceeded:
- reject request with `409 conflict` and stable code `quota_exceeded`
- include details:
  - `dimension`
  - `limit`
  - `current_usage`
  - `requested_delta`

### 2) Scheduler reconciliation (secondary safety net)
Even if the API layer validated, state can change:
- multiple concurrent requests
- retries
- eventual consistency windows
- operator actions

Therefore scheduler must re-check quotas before allocating new instances.

If quota would be exceeded during reconcile:
- scheduler must not allocate the instance
- it must surface an unschedulable condition for that group:
  - reason `org_quota_exceeded:<dimension>`
- it must not thrash by repeatedly attempting allocations every loop without backoff

Backoff rule (v1 recommendation):
- if quota exceeded for a group, do not retry allocation more than once per 30 seconds unless usage decreases.

## Quota enforcement rules by operation (v1)

### Scale up
When desired replicas increase, the delta in instances is:
- `delta_instances = new_desired - old_desired`, bounded to >= 0

Quota checks:
- `max_instances`
- `max_total_memory_bytes` (based on per-instance memory cap)
- `max_total_cpu_request` (based on per-instance cpu_request)

If exceeded:
- reject the scale update at API layer.

Scheduler behavior if desired scale exists but quota is exceeded (should be rare):
- mark group unschedulable with quota reason and do not allocate.

### Deploy
Deploy can change the WorkloadSpec, including memory and cpu requests.

Quota checks on deploy:
- If new release increases per-instance memory and current instances are replaced:
  - the net quota impact depends on rollout strategy.
- For stateless with max_surge:
  - temporary increase is possible. Quota must allow surge.

v1 recommendation:
- Count surge instances against quota. If quota does not allow it:
  - either reject deploy, or
  - force a no-surge rollout mode (drain first) for that deploy.
Pick one consistent behavior.

v1 recommended behavior:
- Prefer correctness and simplicity:
  - If stateless desired replicas > 1 and quota cannot accommodate max_surge=1, run a drain-first rollout and accept temporary unavailability within the group.
  - Record this choice in deploy status message.

For stateful (volume-attached):
- no surge occurs because volume is exclusive. Quota impact is stable.

### Create route
Quota checks:
- `max_routes`
- `max_public_ports`
- if route requires IPv4: `max_ipv4_allocations` must already be satisfied by an existing allocation, or the user must enable IPv4 add-on and pass that quota.

### Enable IPv4 add-on
Quota checks:
- `max_ipv4_allocations`

### Create volume
Quota checks:
- `max_volumes`
- `max_total_volume_bytes`

### Attach volume
Quota checks:
- `max_volume_attachments`
- optional: enforce that any process type with a read-write volume must have desired replicas <= 1

### Snapshot and restore
Quota checks (optional but recommended):
- per-org concurrent restore jobs
- per-volume concurrent snapshots
- global per-org snapshot requests per hour (abuse control)

If exceeded:
- reject request with `quota_exceeded` and dimension.

## Fairness model (v1)
v1 fairness is explicit and deterministic:
- quotas limit the maximum footprint per org
- scheduling uses deterministic placement rules and does not attempt proportional fairness under contention

What this means in practice:
- If two orgs are within quota and the cluster has capacity, both can schedule.
- If the cluster lacks capacity, some desired instances remain unschedulable due to capacity constraints. This is not a quota violation.
- The platform does not preempt one org to satisfy another org in v1 (no preemption).

## Capacity constraints vs quota constraints
These are different and must be surfaced differently.

### Quota exceeded
- The org is trying to exceed its allowed share even if the cluster has free capacity.

Surface as:
- `quota_exceeded` (API)
- `org_quota_exceeded:<dimension>` (scheduler status)

### Cluster capacity exceeded
- The org is within quota, but the cluster is full or constrained (memory hard cap).

Surface as:
- `no_capacity_memory`
- `no_nodes_active`
- `volume_locality_no_node`
- `ipam_exhausted`

Do not confuse these. Users will make incorrect decisions if the messaging is ambiguous.

## Quota configuration model (v1)
### Defaults
Defaults come from service tier policy (free vs paid). The actual numbers live in product docs.

### Overrides
Operator can override quotas per org.

Recommended storage:
- Table `org_quotas`:
  - org_id
  - dimension
  - limit_value
  - updated_at

Recommended derived view:
- `org_usage_view` that aggregates current usage per dimension from materialized views.

### Enforcement consistency
- API layer and scheduler must read the same effective quota limits.
- Effective limits must be computed deterministically:
  - explicit org override if present, otherwise tier default.

## Auditing requirements
All quota-affecting operations must be auditable via events:
- scale changes
- route create/delete
- IPv4 enable/disable
- volume create/delete
- attachment create/delete
- snapshot and restore requests

Quota limit changes (operator action) must also be auditable:
- `org.quota_set` event (recommend adding if not present)

Do not include secrets or credentials in audit payloads.

## Observability requirements
Expose metrics:
- current usage per org per dimension (bounded by org count)
- number of quota_exceeded rejections per endpoint and dimension
- scheduler unschedulable reasons counts, including quota reasons
- time-to-schedule for new instances when within quota

Alerting (operator-facing):
- sudden spikes in quota_exceeded can indicate abuse or a buggy client.
- org nearing quota thresholds (optional warning alerts).

## Compliance tests (required)
1) Scale up beyond max_instances is rejected with quota_exceeded and correct details.
2) Scale up within quota succeeds and results in instance allocations.
3) Deploy with max_surge violates quota:
- either rejected or forced drain-first, depending on chosen policy, and behavior is consistent.
4) Create volume beyond max_total_volume_bytes is rejected.
5) Enable IPv4 beyond max_ipv4_allocations is rejected.
6) Scheduler safety net:
- if two concurrent scale requests race, scheduler does not exceed quota and surfaces org_quota_exceeded for the losing attempt.

## Open questions (future)
- Preemption: whether the platform should ever evict low-priority org workloads to satisfy higher-tier org workloads.
- Reservation model: whether to introduce explicit reservations for high-tier customers beyond quotas.
- Per-process type quotas (not v1).

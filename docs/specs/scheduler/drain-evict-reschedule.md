# docs/specs/scheduler/drain-evict-reschedule.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
Define drain, eviction, and rescheduling behavior:
- what happens when a node is drained for maintenance
- what happens when a node is degraded/offline
- how instances transition through draining and stopped
- how stateless vs stateful workloads differ under eviction
- timeouts, retries, and safety rules

This spec is core to operational reliability.

Locked decisions this depends on:
- local volumes + async backups: `docs/adr/0011-storage-local-volumes-async-backups.md`
- CPU soft, memory hard: `docs/adr/0012-scheduling-cpu-oversubscribe-mem-hardcap.md`

Placement rules are defined in:
- `docs/specs/scheduler/placement.md`
Reconciliation loop is defined in:
- `docs/specs/scheduler/reconciliation-loop.md`

## Scope
This spec defines node lifecycle operations and the resulting scheduling behavior.

This spec does not define:
- restore/migration behavior for stateful volumes (see `docs/specs/storage/restore-and-migration.md`)
- host agent shutdown semantics (agent enforces instance termination, see `docs/specs/workload-spec.md`)
- edge routing behavior (see `docs/specs/networking/ingress-l4.md`)

## Definitions
- **Drain**: an operator action to prepare a node for maintenance by moving workloads off it.
- **Evict**: scheduler-driven process of transitioning instances on a node to draining/stopped and placing replacements elsewhere.
- **Reschedule**: placing replacement instances on other nodes when instances are lost or evicted.
- **Stateless instance**: instance whose process type has no volume mounts.
- **Stateful instance**: instance whose process type has volume mounts (local).
- **Grace period**: time allowed for an instance to stop accepting traffic and exit cleanly.
- **Hard stop**: forced termination after grace period expires.

## Node states (scheduler-relevant)
Nodes have states (from node events / nodes_view):
- `active`: eligible for placement
- `draining`: not eligible for new placement; existing instances should be evicted if possible
- `disabled`: not eligible; treat as removed for scheduling
- `degraded`: eligible or ineligible based on policy (v1 recommendation: ineligible for new placement)
- `offline`: not reachable; treat as failed node

The exact transitions are operator-driven plus health detection.

## Drain operation (operator initiated)
### Goal
- Stop scheduling new instances on the node.
- Move stateless instances to other nodes.
- Handle stateful instances explicitly (because volumes are local).

### Step 1: mark node draining
Operator triggers:
- `node.state_changed` to `draining`

Scheduler must immediately:
- remove node from eligible placement set
- begin eviction workflow for instances on that node

### Step 2: evict stateless instances
For each stateless instance on the draining node:
1) Create a replacement instance on another eligible node (respecting placement constraints).
2) Once replacement is ready (or at least started, depending on policy), mark the old instance desired_state draining.
3) After grace period, mark old instance desired_state stopped if it has not stopped.

v1 recommendation:
- prefer “surge then drain” to reduce downtime for stateless services:
  - create new, wait ready, then drain old

### Step 3: handle stateful instances (local volumes)
Stateful instances cannot move without migration/restore.

v1 behavior:
- Scheduler does not automatically move them.
- Scheduler leaves them running unless an explicit migration plan exists.

Operator options:
- A) Keep stateful instance running during maintenance:
  - drain stateless workloads, then perform maintenance that preserves the volume and instance if possible.
- B) Planned migration:
  - follow restore-based migration procedure:
    - scale down
    - snapshot+backup
    - restore to new node
    - reattach
    - scale up
  - then allow eviction of old instance

Scheduler responsibilities:
- surface a clear “node draining blocked by stateful instances” status:
  - list instances and volumes preventing full drain
- do not thrash or repeatedly attempt impossible moves

### Step 4: complete drain
Node is considered “drained” when:
- all stateless instances are stopped or moved
- only stateful instances remain (if any), and operator acknowledges

In v1, drained is not a separate node state unless you add it. Operator can proceed with maintenance once stateless workloads are off.

## Eviction policy details
### Grace periods
Each instance has a termination grace period from WorkloadSpec lifecycle, default 10 seconds.

Eviction uses:
- drain -> wait grace -> stop

### Readiness and traffic handling
When an instance is marked draining:
- host agent should stop advertising readiness (health becomes not ready)
- edge should remove the backend from routing set as it sees readiness drop

This ensures new connections are not routed to draining instances.

### Timeouts
If an instance does not stop after grace period:
- scheduler emits desired_state stopped
- host agent must force terminate

If host agent is offline and cannot confirm stop:
- scheduler still emits desired_state stopped to represent intent.
- eventual consistency: the node’s failure will be handled by offline node path.

## Rescheduling behavior (node failure)
When a node is `offline` or `disabled` unexpectedly:

### Stateless workloads
Scheduler must:
- treat instances on that node as lost
- allocate replacement instances on other eligible nodes
- mark old instances desired_state stopped (for bookkeeping)

Edge behavior:
- should remove unreachable backends quickly based on readiness and reachability.

### Stateful workloads (local volumes)
Scheduler must:
- mark env/process as degraded with reason:
  - `volume_home_node_offline`
- do not attempt reschedule unless restore/migration is initiated.

Recovery path:
- restore from latest backup to a new node
- update attachment
- start new instance

This is a v1 fundamental constraint.

## Node disabled vs offline
### Disabled (operator action)
- Node is intentionally removed from scheduling.
- Scheduler should evict stateless instances as in drain, but more aggressively if required.

### Offline (detected)
- Node is unreachable.
- Scheduler reschedules stateless immediately.
- For stateful, surface degraded.

v1 detection inputs:
- node heartbeat missing for > threshold (example 60 seconds)
- overlay reachability loss might also be signal, but heartbeats are primary.

## Reschedule retries and stability
To avoid thrash:
- do not repeatedly reallocate replacements if the replacement is already pending or booting.
- use per-group reconciliation locks (see reconciliation-loop spec).
- cap retries when new instances fail repeatedly.

Reschedule should be deterministic:
- replacement instance_id must be new and unique
- placement tie-break rules apply

## Interaction with deploy rollouts
During a deploy rollout, node drain or failure can occur.

Rules:
- scheduler must continue reconciling toward the desired release.
- if an old instance is on a draining node, it should be drained sooner.
- do not count draining/failed instances as satisfying desired replicas.

Deploy status implications:
- deploy may remain rolling longer
- if capacity is insufficient, deploy may fail with a clear reason

## Interaction with capacity pressure
If cluster is at memory capacity:
- draining a node can cause temporary unavailability if there is no room for replacements.

v1 policy:
- surface `no_capacity_memory` as the reason
- allow operator to:
  - add nodes
  - scale down other workloads
  - temporarily accept reduced availability

Scheduler must not violate memory hard constraint.

## State machines and events
Scheduler expresses eviction/reschedule via instance desired state events:
- `instance.desired_state_changed` (draining, stopped)
- `instance.allocated` for replacements

Node state changes are expressed via:
- `node.state_changed`

Agent reports actual transitions via:
- `instance.status_changed`

The system converges through these events.

## Observability requirements
Scheduler metrics:
- number of nodes in draining/disabled/offline
- eviction actions per node
- reschedule actions per node
- time to drain stateless workloads on a node
- blocked drains due to stateful volumes

Operator UX requirements:
- command to show node drain status:
  - list instances remaining, classify stateless vs stateful blockers
- clear messages and reason codes for why drain cannot complete

## Compliance tests (required)
1) Mark node draining:
- no new placements go to node
- stateless instances are replaced and drained

2) Node offline:
- stateless instances reschedule
- stateful instances surface degraded with clear reason

3) Instance does not stop within grace:
- scheduler sets stopped
- agent force terminates and reports stopped

4) Capacity pressure:
- drain triggers unschedulable replacements and surfaces no_capacity reasons without violating memory constraints

5) Edge readiness gating:
- draining instances become not ready and are removed from backend sets

## Open questions (future)
- Whether to support automated stateful migration workflows as a first-class feature.
- Whether to support per-node maintenance windows and automation hooks.

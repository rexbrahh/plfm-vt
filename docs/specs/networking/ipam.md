# docs/specs/networking/ipam.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
Define IPv6 address allocation strategy for:
- nodes (hosts and edge nodes) on the WireGuard overlay
- workload instances (microVMs) for east-west and edge-to-backend routing
- environment-scoped IPv4 allocations (paid add-on) at the edge (IPv4 is referenced here, but dedicated behavior is specified in ipv4-addon.md)

Locked decisions:
- IPv6-first: `docs/ADRs/0007-network-ipv6-first-ipv4-paid.md`
- WireGuard overlay full mesh (v1): `docs/ADRs/0004-overlay-wireguard-full-mesh.md`
- One microVM per instance: `docs/ADRs/0001-isolation-microvm-per-instance.md`

## Scope
This spec defines address allocation, uniqueness guarantees, and lifecycle (allocate, reserve, release).

This spec does not define:
- host firewall rules or routing (see `docs/specs/networking/overlay-wireguard.md` and ingress specs)
- guest networking configuration (see `docs/specs/runtime/networking-inside-vm.md`)
- ingress routing rules (see `docs/specs/networking/ingress-l4.md`)

## Goals (v1)
- Deterministic, conflict-free address allocation.
- Simple mental model for debugging: you can map any instance IPv6 back to (org, env, instance).
- Avoid reassigning addresses too quickly (reduce stale routing issues).
- Support cluster growth without renumbering.

## Non-goals (v1)
- Multi-region global IPAM with complex routing policy.
- Customer-managed IP ranges or BYOIPv6.
- Advertising prefixes via BGP.
- SLAAC, DHCPv6, and dynamic addressing inside microVMs.

## Address spaces
IPAM manages three address categories:
1) Node overlay addresses
2) Instance overlay addresses
3) Edge public addresses (IPv6 default, IPv4 add-on)

### Cluster IPv6 prefix
The cluster must have an IPv6 prefix reserved for overlay and instance addressing.

v1 requirement:
- Operator provides a cluster IPv6 prefix, at least /64, ideally /56 or /48.

We will refer to it as:
- `CLUSTER_PREFIX`

Example:
- `CLUSTER_PREFIX = 2a01:4f8:abcd::/48` (example only)

IPAM subdivides this prefix.

## Node overlay addressing (wg0)
Each node gets a stable /128 on the overlay.

### Allocation rule
- Allocate from a reserved subprefix:
  - `NODE_PREFIX = CLUSTER_PREFIX + ::/64` (example division)
- Node address:
  - `node_overlay_ipv6 = NODE_PREFIX + <node_id_derived_suffix>/128`

Suffix derivation options (choose one and keep stable):
- v1 recommendation: allocate sequentially from a pool and store in DB (simplest).
- Alternative: derive suffix from node_id hash (stable, but harder to reason about if collisions or prefix changes).

v1 recommendation:
- sequential allocation with DB uniqueness.

Invariants:
- node_overlay_ipv6 is unique.
- node_overlay_ipv6 is stable for the life of the node identity.

Lifecycle:
- allocated at node enrollment
- reserved after node removal for a cooldown period (recommend 30 days) before reuse

## Instance overlay addressing
Each running microVM instance is assigned a unique IPv6 /128 used for:
- edge-to-backend routing
- east-west communication (if you enable it)
- debug identity

### Allocation rule (v1)
Allocate instance addresses from a reserved subprefix:
- `INSTANCE_PREFIX = CLUSTER_PREFIX + 1::/64` (example division)

Each instance gets:
- `overlay_ipv6 = INSTANCE_PREFIX + <allocated_suffix>/128`

Allocation method:
- sequential allocation from a pool stored in Postgres, with uniqueness constraints.

Reasons:
- simple
- deterministic
- avoids hash collision handling

### Address lifetime and reuse policy
- Allocate at instance allocation time (scheduler emits instance.allocated event including overlay_ipv6).
- Release when instance is stopped and garbage collected.
- Do not immediately reuse addresses. Use a cooldown pool:
  - v1 recommendation: reuse only after 1 hour minimum, ideally longer (24 hours) to reduce stale routing issues.

### Linkage to WorkloadSpec
WorkloadSpec includes:
- `network.overlay_ipv6`
- `network.gateway_ipv6`
- `network.mtu`
- optional DNS list

The gateway address is defined by runtime networking design. v1 recommendation:
- gateway is a link-local address on the host side, typically `fe80::1`.

## Environment public IPv6 addressing
Public exposure is IPv6-first. There are two ways this can work:
- Edge nodes have public IPv6 addresses and terminate at edge with routing to instance overlay addresses.
- Or each env gets a dedicated public IPv6 address.

v1 recommendation:
- Do not allocate dedicated public IPv6 per env by default.
- Use edge public IPv6 addresses and route by SNI/port to backends.

Reasons:
- simplicity
- avoids public IPv6 address management per env
- aligns with L4 SNI routing

If later you want dedicated IPv6 per env:
- add an explicit feature and adjust IPAM to allocate per env.

## Dedicated IPv4 add-on allocations
IPv4 is a paid add-on with dedicated allocation per env (v1 recommendation).

IPAM responsibilities:
- allocate an IPv4 address from an operator-provided pool
- reserve and release addresses with cooldown
- link allocation to env_id and org_id

The full product behavior and constraints are in:
- `docs/specs/networking/ipv4-addon.md`

## Database model (recommended)
IPAM is stored in Postgres with strong uniqueness constraints.

### Tables
#### `ipam_nodes`
- `node_id` (pk)
- `overlay_ipv6` (unique)
- `allocated_at`
- `released_at` (nullable)

#### `ipam_instances`
- `instance_id` (pk)
- `overlay_ipv6` (unique)
- `allocated_at`
- `released_at` (nullable)
- `cooldown_until` (nullable)

#### `ipam_ipv4_allocations`
- `allocation_id` (pk)
- `env_id` (unique for active allocations)
- `org_id`
- `ipv4_address` (unique)
- `allocated_at`
- `released_at` (nullable)
- `cooldown_until` (nullable)

### Allocation algorithm (v1)
- Use a transaction that:
  1) selects the next available address from a pool table
  2) inserts allocation row with unique constraint
  3) commits

On unique violation:
- retry with next candidate.

Pool representation options:
- a table of free addresses
- or a sequential counter + uniqueness constraint (simpler)

v1 recommendation:
- sequential counter for instance and node IPv6
- explicit pool table for IPv4 (since IPv4 pool is limited and may have gaps)

## Safety and validation rules
- All allocations must be scoped to a cluster.
- No overlap between NODE_PREFIX and INSTANCE_PREFIX.
- No reusing instance addresses inside a cooldown window.
- IPAM operations are auditable:
  - node enrollment allocation is recorded
  - instance allocations are recorded in `instance.allocated` events
  - env ipv4 allocations are recorded in `env.ipv4_addon_enabled` events

## Operational requirements
- Provide operator tooling to:
  - list allocations by org/env/node/instance
  - detect leaks (allocated but no longer referenced)
  - reclaim leaked allocations safely
- Provide metrics:
  - pool usage
  - allocation failures
  - cooldown queue sizes

## Failure behavior

### IPAM allocation failure handling (normative)

Scheduler behavior on IPAM exhaustion:
- Scheduler MUST NOT partially create instance if IPAM allocation fails.
- Scheduler MUST emit `instance.allocation_failed` event with reason `ipam_exhausted`.
- Scheduler MUST mark env as degraded with clear reason in `env_status_view`.
- Scheduler MUST retry allocation on next reconciliation cycle.
- No orphaned resources: if IPAM fails, no instance record is created.

Rollback semantics:
- IPAM allocation is the last step before emitting `instance.allocated`.
- If allocation fails, the entire placement transaction is aborted.
- Any resources reserved earlier in the transaction are released.

### Pool exhaustion alerting

- Alert when pool utilization exceeds 80%.
- Alert when allocation failures occur (rate > 0 for 5 minutes).
- Dashboard MUST show pool utilization by category (node, instance, ipv4).

### IPAM database unavailable

- If IPAM database is down:
  - No new allocations can be created.
  - Existing workloads keep running.
  - Scheduler marks itself as degraded.
  - New deploys fail with clear error: `ipam_unavailable`.

## Open questions (explicitly deferred)
- Whether to allocate per-node instance prefixes for simpler routing (requires changes in overlay AllowedIPs and routing design).
- Whether to support dedicated public IPv6 per env as a paid tier later.

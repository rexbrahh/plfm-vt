# docs/architecture/07-scaling-plan-multi-host.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
This document describes how the platform scales from a single machine to multiple hosts, and what constraints and transitions we expect as the system grows.

This is narrative. Authoritative details live in:
- `docs/specs/scheduler/*`
- `docs/specs/networking/*`
- `docs/specs/runtime/*`
- `docs/specs/storage/*`
- `docs/architecture/06-failure-model-and-degraded-modes.md`
- ADR 0004 (WireGuard mesh), ADR 0011 (local volumes), ADR 0012 (CPU soft, memory hard)

## Scaling stance
- Start with a single-region, multi-host cluster.
- Scale primarily by adding hosts (horizontal scaling).
- Keep control plane writes centralized (Postgres primary) in v1.
- Keep networking simple (WireGuard full mesh) until node count forces a topology change.
- Treat memory as the true scarce resource (hard caps) and CPU as oversubscribable (soft).

## What “multi-host” means here
Multi-host means:
- multiple worker nodes running microVM instances (Firecracker)
- one or more edge nodes handling ingress
- a control plane deployment (may be co-located early, but logically separate)
- all nodes connected over a WireGuard overlay, IPv6-first

## Scaling dimensions

### 1) Compute density (instances per host)
The platform scales by packing microVM instances onto hosts.

Constraints:
- Memory is hard-capped and drives safe density.
- CPU is oversubscribed and drives performance variability.

Practical policy (v1 intent):
- Set a cluster-level `cpu_overcommit_ratio` (example 4.0).
- Define per-node `allocatable_memory = physical - reserved - safety_buffer`.
- Scheduler places instances until allocatable budgets are exhausted.

What we monitor:
- p95 and p99 CPU throttling and run queue pressure
- memory usage vs caps, OOM rates
- boot time and churn (too many restarts implies capacity or config issues)

### 2) Control plane throughput
Control plane scaling is mostly about:
- Postgres write throughput (events)
- projection throughput (materialized views)
- scheduler throughput (allocation decisions)
- distribution throughput (streaming to agents and edge)

v1 approach:
- keep it on one Postgres primary with a warm standby
- scale the control plane app services horizontally (stateless) behind the DB
- scale projection workers independently
- keep event types tight and payloads minimal so events remain cheap to append

When it becomes a bottleneck:
- add read replicas for heavy reads (views)
- optimize projections and indexes
- split projection responsibilities by domain
- only after real load, consider partitioning or sharding strategies

### 3) Edge ingress capacity
Edge capacity is driven by:
- concurrent connections (TCP)
- connection rate (new connections per second)
- routing table size (routes and backends)
- health gating behavior

v1 approach:
- run multiple edge nodes so one edge node failure is not a full outage
- edge consumes routing config from control plane and applies it atomically
- keep routing decisions per-connection (L4) and avoid expensive per-request logic

Open decision (not locked here):
- how external traffic is distributed across edge nodes (DNS strategy, anycast later). The choice affects time-to-failover and operational complexity.

### 4) Networking overlay scaling
v1 overlay topology is WireGuard full mesh.

This is simple and works well at small node counts, but peer count grows with O(n²).

Practical thresholds:
- up to ~25 nodes: full mesh is usually comfortable
- ~25 to ~50 nodes: still workable, but operations and churn become noticeable
- beyond ~50 nodes: plan a topology shift

Planned evolution path:
- v2: introduce regional hubs (still WireGuard), nodes peer with hubs rather than everyone
- v3: consider routed overlay or more advanced approaches if needed

Overlay invariants:
- IPv6-first addressing and IPAM remains authoritative
- overlay failures must degrade cleanly (remove unreachable backends from routing)

### 5) Storage scaling (local volumes)
Storage is local volumes plus async backups.

Scaling implications:
- Stateless workloads scale horizontally with no storage coupling.
- Stateful workloads scale only with application-level replication (not provided by platform).
- Volume locality constrains scheduling:
  - workloads that need a volume must run on the host where the volume lives
  - host failure takes that stateful workload down until restore or migration

Operational approach to scale stateful use cases:
- push users toward external managed databases for early use cases, or
- document application-level replication patterns if users run stateful services

Planned improvements:
- restore tooling and runbooks get more important as stateful usage increases
- volume migration is a future feature, not a v1 promise

### 6) Observability and operational scaling
As node count grows, the platform must remain operable.

v1 requirements:
- per-node and per-instance metrics with controlled label cardinality
- logs that can be retrieved by instance id and env
- projection lag metrics and scheduler loop timing metrics
- overlay health metrics
- disk pressure metrics (image cache and volume pool separate)

The platform must have a single “debug story” at any size:
- what is desired
- what is running
- what is routing
- why is it not converging

## Growth stages (practical plan)

### Stage 0: Single-box reference implementation
- everything on one machine
- proves the contracts and end-to-end flow

Exit criteria:
- deploy, route, logs, rollback, secrets delivery work reliably

### Stage 1: Single region, few hosts (v1 production shape)
- 3 to 10 worker nodes
- 2+ edge nodes
- control plane on dedicated host(s)
- WireGuard full mesh

Exit criteria:
- node loss recovery for stateless services works
- control plane outage does not immediately stop traffic
- volume restore runbook works

### Stage 2: One region, dozens of hosts
- optimize scheduler and projections
- harden edge config reloads
- start planning overlay topology change if mesh management becomes painful

Exit criteria:
- predictable capacity planning for memory
- bounded blast radius for failures
- backup restore drills pass consistently

### Stage 3: Multi-region (future, not v1)
- multi-region introduces new problems:
  - DNS and routing strategy
  - control plane consistency across regions
  - data residency and latency
- do not attempt until the single-region system is boring and reliable

## Host lifecycle at scale
Adding nodes:
- provision host OS consistently (NixOS intent)
- enroll node into control plane
- join overlay
- report capacity and become schedulable

Maintenance (drain):
- mark node as draining
- scheduler stops placing new instances
- stateless instances are evicted and rescheduled
- stateful instances require explicit action (keep on node, or restore elsewhere if needed)

Decommission:
- ensure no volumes remain, or explicitly migrate/restore
- revoke node identity and remove overlay membership

## Risks and mitigations (scaling-specific)
- WireGuard mesh churn: mitigate with stable endpoints, key rotation tooling, and clear MTU policy.
- Projection lag: mitigate by splitting projections, adding workers, and keeping events lean.
- Disk pressure: mitigate with cache eviction and separate volume pool from image cache.
- IPv6 reachability variability: mitigate with clear IPv4 add-on UX and route ownership rules.
- Stateful workload expectations: mitigate by making local-volume constraints explicit and providing restore tooling early.

## Next document
- `docs/architecture/08-security-architecture.md`

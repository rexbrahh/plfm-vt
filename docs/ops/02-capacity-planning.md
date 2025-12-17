# Capacity planning

Capacity planning is the discipline of preventing avoidable outages caused by running out of compute, storage, or networking headroom.

This platform is built around microVMs (Firecracker) on hosts, with an overlay network (WireGuard) and a control plane that reconciles desired state.

## Goals

- Maintain enough headroom to survive common failure scenarios (N+1 hosts, edge node loss, Postgres failover)
- Predict capacity exhaustion early and respond before customers notice
- Make capacity decisions transparent and reversible

## Key capacity domains

### 1) Compute (hosts)

We plan for:

- **Memory** as the primary limiting resource (microVM density is usually memory bound)
- **CPU** as the second limiter (spikes, noisy neighbors, build workloads)
- **IOPS** and **disk throughput** for storage heavy apps

#### Host headroom targets (starting point)

- Keep **>= 20% free memory** headroom at the region level after reserving for N+1 host loss
- Keep **>= 25% CPU headroom** at peak p95
- Keep **>= 30% disk free** on host OS disks (avoid full disk cascades)

Adjust based on observed workloads.

#### MicroVM overhead assumptions

Firecracker microVMs are cheap but not free.

Track:

- per-VM memory overhead (kernel + init + agent)
- per-VM CPU overhead (virtio, networking, IO)
- per-VM file descriptor and process count on the host

Do not guess. Measure overhead in production like workloads.

### 2) Edge ingress capacity

Edge capacity is limited by:

- concurrent connections
- per-connection memory
- packet processing throughput
- per-port listener fanout (if using dedicated ports)

Headroom target:

- survive loss of one edge node without exceeding 70% utilization on remaining nodes

### 3) Control plane capacity

The control plane is often limited by:

- Postgres CPU and IOPS
- API request concurrency
- reconciliation backlog and retry storms

Track:

- Postgres connections, replication lag, slow queries
- reconcile queue depth and time-to-converge SLI
- API p95 latency and 5xx

### 4) Storage capacity

Storage includes:

- per-volume allocated size
- actual used bytes
- snapshot growth (copy-on-write amplification)
- backup storage (Postgres + snapshot metadata + volume backups)

Targets:

- keep snapshot storage below 70% of allocated backing store
- maintain tested restore speed that meets RTO/RPO targets

### 5) Network overlay (WireGuard)

Overlay capacity is affected by:

- UDP packet loss
- handshake stability
- MTU issues and fragmentation
- per-host peer count scaling

Track:

- peer handshake age distribution
- per-peer throughput and drops
- overlay RTT distribution between hosts

## Forecasting and triggers

### Forecast cadence

- Weekly: region headroom review (compute, edge, Postgres)
- Monthly: growth forecast and procurement plan
- After every major customer onboarding: update forecast

### Trigger thresholds (starting point)

Trigger a capacity action when any of these are true:

- region memory headroom projected < 20% within 30 days
- edge connection utilization projected > 70% within 30 days
- Postgres primary CPU > 60% sustained at peak hours for 7 days
- snapshot store utilization projected > 70% within 30 days

Projections should use p95 daily peak and a simple linear fit, then validate with reality.

## Failure-budgeted capacity

Plan to survive:

- one host loss per cluster (N+1)
- one edge node loss per region
- Postgres primary failure and replica promotion

When calculating "available" capacity, subtract the largest host and one edge node.

## Capacity operations

### Adding hosts (high level)

1. Provision host (hardware, OS, baseline hardening)
2. Install and start host-agent, Firecracker supervisor, storage services
3. Join overlay network (WireGuard), confirm peer connectivity
4. Register host with control plane, confirm healthy heartbeats
5. Run a canary workload and verify performance
6. Mark schedulable

### Decommissioning hosts

1. Cordon host (no new placements)
2. Drain workloads (evict and reschedule)
3. Verify zero workloads remain and volumes are detached
4. Remove host from overlay and control plane inventory
5. Wipe disks if required and retire hardware

## Capacity dashboards (minimum set)

- Region overview: total vs reserved vs used CPU/RAM, N+1 adjusted headroom
- Host heatmap: CPU, memory, disk, packet loss, instance counts
- Edge overview: connect success, resets, connections, throughput
- Control plane: API latency/errors, reconcile backlog, Postgres health
- Storage: volume usage, snapshot growth, backup jobs

## Common failure modes to plan for

- "Slow bleed" memory fragmentation on hosts leading to OOM
- Postgres connection exhaustion causing API outages
- Retry storms during partial network partitions
- Edge node health flaps causing connect failures
- Snapshot growth consuming storage faster than expected

Each failure mode should map to an alert and a runbook.

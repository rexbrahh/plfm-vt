# docs/architecture/06-failure-model-and-degraded-modes.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
This document defines the expected failure modes of the platform and the degraded behaviors we accept in v1.

It answers:
- What can fail
- What keeps working when it fails
- What becomes unavailable
- How the system recovers
- What we explicitly do not promise

This is narrative. The authoritative details live in:
- `docs/specs/state/*`
- `docs/specs/networking/*`
- `docs/specs/runtime/*`
- `docs/specs/storage/*`
- `docs/specs/scheduler/*`
- `docs/ops/runbooks/*`

## Core stance
- The platform must continue serving existing traffic during control plane outages.
- Data plane components must be able to operate on their last applied desired state.
- The platform is not “always available” for writes in v1. It is “recoverable and explainable”.
- For stateful workloads, availability is bounded by the local volume model (ADR 0011).

## Failure domains
We separate failures into these domains because they have different blast radius and recovery paths:

1) Control plane (API, scheduler, projections)
2) Database (Postgres)
3) Edge ingress
4) Host / node agent
5) Overlay network (WireGuard)
6) Storage (local volumes + backups)
7) Artifact supply chain (OCI registry, image fetch)
8) Customer reachability (IPv6 internet reality, DNS)

## Golden invariants under failure
These must remain true even during incidents:

- Events are append-only. We do not “fix state” by editing history.
- Agents never guess desired state. They converge to what control plane declared last.
- Secrets do not leak across env boundaries.
- Memory hard caps remain enforced per instance.
- Routing never silently routes hostnames to the wrong tenant.
- Recovery actions are auditable.

## Degraded modes and behavior

### A) Control plane unavailable (API down, scheduler down, or projections down)
**What still works:**
- Existing workload instances keep running on hosts.
- Edge continues routing using the last applied routing config.
- Existing connections continue where possible (edge dependent).
- Host agents continue supervising microVMs and restarting crashed instances if they are still desired in their local view.

**What does not work:**
- New deploys, scale changes, route updates, secrets updates, volume operations (control plane writes).
- New exec grants (if they require control plane authorization).
- Fresh reads that require live views may be stale or unavailable depending on implementation.

**Recovery expectation:**
- Restore control plane service.
- Ensure projections catch up to event log.
- Resume scheduling and distribution.
- No data plane restart should be required as a first step.

**Operator rule:**
- “Control plane down” is a serious incident, but not immediately customer traffic-impacting if edge and hosts are healthy.

---

### B) Postgres unavailable or corrupted
**What still works (best effort):**
- Same as “control plane unavailable” if edge and hosts operate from cached config.
- Data plane continues until it needs new desired state.

**What does not work:**
- All control plane operations. This is the root dependency.

**Recovery expectation:**
- Failover to standby if configured.
- Otherwise restore from backups with WAL replay (point-in-time restore).
- Projections may need rebuilds from the event log.

**Hard truth (v1):**
- If Postgres is down and you have no working standby, the platform becomes read-only and eventually “stale-running” until restored.

---

### C) Projections lagging or broken (event log ok, views incorrect)
**Symptoms:**
- Reads return stale or inconsistent data.
- Scheduler decisions may be delayed or incorrect.
- Route updates may not propagate.

**What still works:**
- Event appends can still work if API can write (depending on architecture), but reads may be wrong.
- Data plane continues serving last applied routing and allocations.

**Recovery expectation:**
- Fix projection code.
- Rebuild affected views from the event log.
- Verify with the end-to-end demo script and health checks.

**Operator rule:**
- Never patch view tables manually as a permanent fix.
- Append corrective events if you need to reflect a real change in desired state.

---

### D) Scheduler lagging or unavailable
**What still works:**
- Existing instances run.
- Edge routes to existing healthy backends.
- Deploys that only change routing or scale will not converge if scheduler is required.

**What does not work:**
- Scaling, rescheduling, rollouts that require new allocations.

**Recovery expectation:**
- Restart scheduler workers.
- Verify scheduler cursor and reconciliation loops resume.
- Confirm no runaway reschedule thrashing.

---

### E) Edge ingress failure
We consider two failure types.

#### E1) Single edge node failure (multiple edge nodes exist)
**What still works:**
- Traffic continues via surviving edge nodes (depending on DNS or routing strategy).
- Control plane and hosts continue.

**What degrades:**
- Reduced capacity.
- Potential higher latency if traffic shifts.

**Recovery expectation:**
- Replace or repair edge node.
- Apply latest routing config.

#### E2) All edge nodes down
**What still works:**
- Workloads continue running.
- Internal service-to-service communication over overlay can still work.

**What does not work:**
- New inbound connections from the public internet.
- Any public-facing availability is effectively down.

**Recovery expectation:**
- Restore edge service.
- Confirm routing tables are applied and health gating is correct.

---

### F) Host failure (node lost)
#### F1) Stateless workloads
**What happens:**
- Instances on that node are lost.
- Scheduler should place replacement instances on other nodes.

**Recovery expectation:**
- Rescheduling happens automatically once scheduler observes node loss.

#### F2) Stateful workloads with local volumes
**What happens:**
- If the node is down, the local volume is unavailable.
- The workload cannot be restarted elsewhere without a restore or explicit migration procedure.

**Recovery expectation:**
- If node returns, reattach and continue.
- If node does not return, restore volume from backup to a new node, then start workload.

**Important constraint:**
- v1 does not promise automatic, zero-downtime failover for stateful workloads.

---

### G) Host agent failure (daemon down, host still up)
**What still works:**
- Running microVMs may continue running.
- Edge routing may still function if backends remain healthy.

**What degrades:**
- No reconciliation, no restarts for crashed instances, no new allocations applied on that host.
- Observability may degrade (logs, metrics).

**Recovery expectation:**
- Restart agent.
- Agent must reconstruct actual state and resume reconciliation safely.

---

### H) Overlay network partition (WireGuard issues)
We consider two common cases.

#### H1) Control plane cannot reach some nodes
**What happens:**
- Scheduler cannot reliably place new work on those nodes.
- Those nodes may continue running existing workloads.
- Edge may also lose reachability, which will remove backends from routing if health gating detects it.

**Recovery expectation:**
- Repair underlay or WireGuard config.
- Nodes rejoin and resynchronize event cursor.

#### H2) Edge cannot reach backends on some nodes
**What happens:**
- Those backends are removed from routing.
- Service may degrade or go down if all backends are unreachable.

**Recovery expectation:**
- Fix overlay connectivity.
- Edge re-adds backends once health is restored.

---

### I) Secrets subsystem issues
#### I1) Secrets update fails mid-rollout
**What happens:**
- Some instances may be running old secret version, some new, if you allow partial rollout.
- This can break apps that require consistent secrets across instances.

**Recommendation for v1 safety:**
- Treat secrets rotation as a controlled rollout similar to deploy.
- Allow rollback to previous secret version.

**Recovery expectation:**
- Roll forward to complete rotation, or roll back to previous version.
- Audit must show which version each instance is using.

#### I2) Secrets delivery file missing or wrong permissions
**What happens:**
- Instance fails to start or fails health checks.
- Scheduler may thrash restarts unless backoff exists.

**Recovery expectation:**
- Fix agent delivery logic.
- Ensure failures are visible as “secrets injection failed” rather than generic crash loops.

---

### J) Storage failure modes
#### J1) Disk full on host
**What happens:**
- New instances may fail to start (rootfs build, scratch disk allocation).
- Volume operations may fail.
- Risk of broader host instability.

**Recovery expectation:**
- Evict caches first (image cache, old scratch disks).
- Stop scheduling new work to the host.
- Repair disk capacity.

#### J2) Volume corruption
**What happens:**
- Stateful workload data loss or application errors.

**Recovery expectation:**
- Restore from last known good backup.
- Document and run postmortem.

---

### K) OCI registry or image fetch unavailable
**What still works:**
- Instances already running continue.
- Deploys that do not require pulling new images might still work only if cached.

**What does not work:**
- New deploys that require new image pulls on nodes without cache.

**Recovery expectation:**
- Retry image pulls.
- Prefer digest-pinned caching to maximize resilience.
- Consider mirrored registry later if needed.

---

### L) Customer reachability issues (IPv6 reality, DNS)
#### L1) Clients cannot reach IPv6-only endpoints
**What happens:**
- The service is “up” but unreachable for some users.

**v1 stance:**
- This is expected given IPv6 adoption variability.
- Users who need IPv4 reachability must use the IPv4 add-on.

**Recovery expectation:**
- Enable IPv4 add-on for that environment if required.
- Provide clear CLI and docs messaging about IPv6-only exposure.

#### L2) DNS outage or misconfiguration
**What happens:**
- Hostname routing fails for customers even if edge and backends are healthy.

**Recovery expectation:**
- Fix DNS quickly.
- Keep TTLs reasonable.
- Have operator runbooks for DNS changes and rollbacks.

## Recovery tooling requirements
To make these failure modes survivable, we require:

- A canonical end-to-end demo script that validates core flows (deploy, route, logs, rollback).
- Rebuild tools for projections (replay from event log).
- Runbooks for:
  - Postgres failover and restore
  - edge restart and config reapply
  - node drain and reschedule
  - overlay partition debugging
  - volume restore
- Alerting on:
  - control plane unavailability
  - projection lag
  - edge backend reachability loss
  - node agent down
  - disk pressure
  - backup failures

## What we explicitly do not promise in v1
- Zero-downtime failover for stateful workloads.
- Multi-region active-active control plane writes.
- Perfect IPv6 reachability for all end users without IPv4 add-on.
- Automatic remediation of every failure without operator involvement.

## Next document
- `docs/architecture/07-scaling-plan-multi-host.md`

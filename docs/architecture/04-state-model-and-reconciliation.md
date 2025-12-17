# docs/architecture/04-state-model-and-reconciliation.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
This document explains the platform’s state model and how the system converges from desired state to actual state.

Authoritative details live in:
- `docs/specs/state/event-log.md`
- `docs/specs/state/event-types.md`
- `docs/specs/state/materialized-views.md`
- `docs/specs/scheduler/reconciliation-loop.md`
- `docs/specs/scheduler/drain-evict-reschedule.md`
- ADR 0005

## Core stance
- The platform is declarative: the control plane declares desired state.
- The data plane is convergent: host agents and edge components reconcile until actual state matches desired state.
- The source of truth is immutable history: an append-only event log.
- Current state is derived: materialized views are projections of the event log.

## Why this model exists
We want:
- auditability (who did what and when)
- reproducibility (ability to replay state)
- debuggability (explain why something is in a given state)
- safe recovery (rebuild views after corruption or bugs)
- a platform that behaves predictably under partial failures

## Event log: what it is and what it is not
### Event log is
- Append-only.
- The authoritative record of all control plane state transitions.

### Event log is not
- A stream of “best effort logs”.
- A place where we rewrite history to “fix” states.
- A replacement for runtime telemetry (events describe intent and transitions, not full metrics).

## Event shapes and ordering (narrative)
An event includes:
- a global monotonic event id (for streaming and replay)
- an aggregate identity:
  - aggregate type
  - aggregate id
  - per-aggregate sequence number
- event type and version
- actor identity (user or service principal)
- payload (structured)

Ordering model:
- Global event id gives replay order and consumer cursors.
- Per-aggregate sequencing prevents conflicting transitions and enforces invariants.

Events are versioned. We add new event versions rather than editing old ones.

## Materialized views (projections)
Materialized views are:
- tables that represent current state for reads
- updated by projection workers that consume events

Projection requirements:
- idempotent
- restart-safe (checkpointed)
- replayable (can rebuild from scratch)

Views are authoritative for reads. APIs do not scan raw events for normal reads except for audit endpoints.

## Commands and events
Write requests are commands.

Command handling rules:
1) authenticate and authorize
2) validate invariants using current views
3) append events in a single transaction
4) return response:
   - either eventually consistent
   - or read-your-writes via waiting for projection checkpoints

This ensures we do not mix “mutable tables as truth” with event history.

## Reconciliation: the convergent loop
Reconciliation is the system that turns desired state into actual state.

There are multiple reconcilers:
- scheduler reconciler: desired scale and placements -> allocation events
- host agent reconciler: allocation events -> running microVM instances
- edge reconciler: route events -> applied L4 routing config
- volume reconciler: volume intents -> attached block devices and mounts
- secrets reconciler: secret versions -> rollout restart triggers

Each reconciler follows the same pattern:
- read desired state (from views)
- observe actual state (from agents or local observation)
- compute diff
- apply bounded actions
- report outcome and persist progress

## Desired state vs actual state
### Desired state
Derived from:
- manifests
- env configuration
- scale settings
- routes
- volumes
- secrets versions
- scheduling constraints

Desired state is stored as events and views.

### Actual state
Derived from:
- host agent observations (what microVMs are running and healthy)
- edge observation (which routes are active and where they forward)
- storage observation (volume attachment state)
- control plane observation (projection checkpoints)

The system’s job is to reduce the gap between desired and actual.

## State machines (narrative)
Several domains are state machines. They must be explicit in event types and views.

### Release lifecycle
- created -> available -> promoted -> superseded -> rolled back
(Exact states live in event types spec.)

### Deployment lifecycle per env and process type
- desired release set
- instances rolling
- instances healthy
- route cutover complete

### Instance lifecycle
- allocated -> preparing -> booting -> ready -> draining -> stopped -> garbage collected

### Route lifecycle
- requested -> validated -> active -> updating -> removed

### Secrets lifecycle
- created -> new version -> rollout restart scheduled -> applied to new instances

### Volume lifecycle
- created -> attached -> in use -> snapshotting -> backed up -> detached -> deleted
(Deletion in v1 should be cautious: ensure backups exist or require explicit force.)

## Read-your-writes semantics
Some endpoints must provide read-your-writes.

Mechanism:
- projection workers maintain `last_applied_event_id` checkpoints
- write endpoints can block until specific projections have caught up to the event id they appended
- bounded timeout with a clear error is mandatory

This avoids implicit eventual consistency surprises.

## Change distribution model
Agents and edge consume changes using an event cursor.

Requirements:
- at-least-once delivery
- idempotent consumers
- replay support by cursor
- server-side filtering

Streaming is preferred with polling fallback.

## Idempotency and retries
Reality includes retries. The state model must explicitly support them.

Rules:
- Commands accept idempotency keys for client retries.
- Event append is transactional.
- Projections must tolerate duplicate events or repeated delivery.
- Reconcilers must be safe to rerun after crashes.

## Handling failure and repair
### Projection bug
- fix projection code
- rebuild views from event log
- verify state using end-to-end demo checks

### Stuck reconciler
- identify diff between desired and actual
- repair by appending explicit corrective events (never by editing history)
- record the remediation steps as an ops runbook if it is recurring

### Partial system outages
- control plane down: data plane continues with last applied config
- edge down: no new connections, but internal workloads still run
- host down: stateless workloads reschedule, stateful workloads require restore/migration plan

## Invariants (high level)
- Events are append-only.
- Views are derived and rebuildable.
- Agents do not mutate truth. They report observations.
- Reconciliation actions are bounded and idempotent.
- Memory is never oversubscribed by scheduling.
- Secrets never cross env boundaries.

## Next document
- `docs/architecture/05-multi-tenancy-and-identity.md`

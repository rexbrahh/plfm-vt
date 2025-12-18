# docs/ADRs/0005-state-event-log-plus-materialized-views.md

## Title

Control plane state is an append-only event log with materialized views

## Status

Locked

## Context

We need a control plane state architecture that supports:

* auditable, explainable changes (who did what, when, why)
* deterministic rollbacks and incident debugging
* multi-node convergence (agents and edge config must reconcile to the same desired state)
* safe schema evolution over time without rewriting history
* the ability to rebuild derived state if we change projections

We also want to avoid a system where mutable tables are the only truth and “current state” can be silently corrupted without a trace.

This ADR chooses the state model. It does not choose the database (Postgres is chosen in ADR 0006).

## Decision

1. **The source of truth for the control plane is an append-only event log.**
   All state transitions are recorded as immutable events.

2. **Current state is represented by materialized views derived from the event log.**
   These views are projection outputs (tables) rebuilt from events and used for reads and queries.

3. **Writes are modeled as commands that produce events.**

* API receives a command
* validate authorization and invariants
* append one or more events
* projections update materialized views asynchronously or synchronously depending on endpoint semantics

4. **Events are versioned and schema-evolved, never edited in place.**

* event type has a stable name
* payload has an explicit version
* new versions are additive where possible

5. **Projections are idempotent and replayable.**

* projections can be rebuilt from scratch from the event log
* projection handlers must tolerate duplicate delivery and at-least-once processing

6. **Agents and edge components reconcile from desired state derived from the views, not by issuing imperative mutations.**
   The control plane declares desired state. Node agents converge actual state to match it.

## Definitions

* **Event log**: an append-only sequence of records representing validated state transitions.
* **Materialized view**: a derived table or index representing current state for fast reads.
* **Projection**: the code that consumes events and updates materialized views.
* **Command**: a requested action that is validated and translated into events.

## Rationale

* An event log provides a durable audit trail and makes incidents explainable.
* Materialized views keep read patterns simple and fast without sacrificing traceability.
* Rebuildability lets us fix bugs in projections and recover from corruption by replaying events.
* This model supports distributed components that need a consistent desired state contract.

## Consequences

### Positive

* Strong auditability and debuggability
* Deterministic rollbacks and clear release history
* Safer evolution of the control plane over time
* Enables reliable fan-out to agents and edge config via a change stream

### Negative

* More engineering effort than “just mutable tables”
* Projection correctness becomes critical
* Some queries require careful view design
* Operational tooling is needed (replay, backfill, migration of projections)

## Alternatives considered

1. **Mutable tables as the only truth**
   Rejected due to weak auditability and difficult incident forensics.

2. **CRDT style distributed state**
   Rejected due to complexity and unclear fit for strong invariants around releases, routing, and secrets.

3. **External event store**
   Rejected for v1 because it increases dependency surface. We can evolve later if needed.

## Invariants to enforce

* Events are append-only. No updates, no deletes, no in-place edits.
* Every state transition that matters for behavior must be represented by events.
* Events must include sufficient identifiers to rebuild state deterministically (org, project, app, env, release, workload instance ids as applicable).
* Projection updates must be idempotent and must record progress (for restart safety).
* Read APIs must be served from materialized views, not by scanning raw event log, except for audit endpoints.

## What this explicitly does NOT mean

* We are not doing “event sourcing everywhere” inside customer workloads.
* We are not forcing every read to be eventually consistent. Some endpoints may wait for projections when needed, but the architecture remains event-first.
* We are not allowing agents to mutate control plane truth directly. Agents report observations; control plane emits events.

## Open questions

* Per-aggregate ordering model: global sequence vs per-aggregate sequence (recommendation: per-aggregate sequencing plus a global monotonic id for replay ordering).
* Which endpoints require read-your-writes semantics and therefore need synchronous projection or query of a hot view.
* How we expose the change stream to agents and edge components (polling, streaming, or both) without changing the event log contract.

Proceed to **ADR 0006** when you’re ready.

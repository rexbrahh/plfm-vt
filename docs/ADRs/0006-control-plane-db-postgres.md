# docs/adr/0006-control-plane-db-postgres.md

## Title

Control plane primary database is Postgres

## Status

Locked

## Context

The control plane needs a durable, strongly consistent data store for:

* the append-only event log (ADR 0005)
* materialized views derived from events
* auth and tenancy data
* scheduler decisions and allocations
* audit and billing-relevant records

We want a store that is:

* operationally boring and well understood
* supports transactions and strong invariants
* easy to back up and restore
* easy to run on a single dedicated machine at first, with a clear path to replication

## Decision

1. **Postgres is the primary and authoritative database for the control plane in v1.**

2. **The event log and materialized views live in Postgres.**

* Event log is stored in an append-only table (or set of tables) with enforced immutability rules.
* Materialized views are stored as normal tables maintained by projection code.

3. **All control plane writes go through Postgres transactions.**

* Commands validate invariants, then append events.
* Projection progress is tracked in Postgres to support idempotent resume after crashes.

4. **High availability starts as “simple, correct, recoverable”.**

* v1 can run as a single primary with backups.
* We add replication and failover when the product needs it, without changing the state model.

## Rationale

* Postgres is mature, reliable, and has excellent tooling for migrations, backups, and observability.
* Strong transactional semantics simplify correctness for event append, idempotency tracking, and invariant enforcement.
* It lets a small team ship quickly without taking on a distributed database operational burden.

## Consequences

### Positive

* Clear correctness story for invariants and state transitions
* Straightforward backup and restore strategy
* Easier incident response and debugging
* Fits naturally with an event log plus projections architecture

### Negative

* A single primary can become a bottleneck at scale
* HA and failover require careful operational work
* Some scaling strategies (multi region writes) are non-trivial and out of scope for v1

## Alternatives considered

1. **etcd or other distributed KV**
   Rejected because it pushes us into a distributed systems operations problem early and makes relational querying and migrations more awkward.

2. **MySQL**
   Rejected due to less alignment with our team preferences and ecosystem for this style of event sourcing and projections.

3. **Distributed SQL (CockroachDB, Yugabyte, etc)**
   Rejected for v1 because the operational and performance tradeoffs are not worth it before we have product-market proof and clear scaling requirements.

4. **Custom database**
   Rejected for v1 because control plane correctness and operability matter more than novelty.

## Invariants to enforce

* Event log is append-only at the database level (permissions, constraints, triggers, or equivalent guardrails).
* Every event has stable identifiers and monotonic ordering fields required for deterministic replay.
* Projection handlers record checkpoints so they can resume safely after restart.
* Schema migrations are versioned and applied in a controlled release process.

## What this explicitly does NOT mean

* We are not using Postgres as the main customer data store for user applications.
* We are not promising multi region active-active writes in v1.
* We are not building the platform around heavy SQL business logic in stored procedures. Application code owns projections and invariants, Postgres enforces constraints where appropriate.

## Open questions

* Exact event ordering model and indexing strategy (global sequence id, per-aggregate sequence, or both).
* Replication and failover posture for early production (managed Postgres vs self-hosted).
* Backup cadence, retention, and restore testing requirements for v1 launch.

Proceed to **ADR 0007** when ready.

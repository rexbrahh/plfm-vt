# docs/architecture/01-control-plane.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
This document describes the control plane responsibilities, boundaries, and internal structure.

Authoritative contracts live in:
- `docs/specs/api/*`
- `docs/specs/state/*`
- `docs/specs/scheduler/*`
- `docs/specs/networking/*`
- `docs/specs/secrets/*`
- `docs/specs/storage/*`

Locked decisions are in:
- `docs/DECISIONS_LOCKED.md`
- `docs/adr/*`

## What the control plane is
The control plane is the platform’s source of truth for:
- identity and authorization
- desired state for workloads, routing, secrets, and volumes
- auditability and history (event log)
- scheduling decisions and allocations
- configuration distribution to agents and edge

The control plane does not run user workloads. It declares desired state and drives convergence.

## What the control plane is not
- Not a general workflow engine.
- Not a place where host agents push imperative mutations to “make state true”.
- Not a multi-writer distributed database. Postgres is the authoritative store in v1.
- Not a scheduler that assumes shared storage. Volumes are local and constrain placement.

## Top-level architecture
The control plane can be implemented as one deployable service in v1, but it is conceptually split into these subsystems:

1) API and auth
2) Command handling (validation and event appends)
3) Event log storage
4) Projection workers (materialized views)
5) Scheduler and reconcilers
6) Change stream distribution (to agents and edge)
7) Admin and operator tooling (migrations, repairs, backfills)

## Source of truth: event log and views
### Event log
- Append-only.
- Every meaningful state transition is an event.
- Events are ordered with a global monotonic id and also sequenced per aggregate.

Event rules (high level):
- No updates or deletes to events.
- Event schemas are versioned.
- Events must carry enough identifiers to rebuild state deterministically.

See: `docs/specs/state/event-log.md` and `docs/specs/state/event-types.md`.

### Materialized views
- Used for reads and queries.
- Maintained by projection workers.
- Rebuildable by replaying the event log.

See: `docs/specs/state/materialized-views.md`.

### Read-your-writes behavior
Control plane endpoints fall into two categories:

- Eventually consistent is acceptable:
  - lists, dashboards, general reads

- Read-your-writes required:
  - deploy/create operations that immediately return the created object or status

For read-your-writes endpoints, the control plane waits for required projections to reach a checkpoint at or beyond the new events, then reads from views.

This rule must be implemented explicitly and tested. It prevents “API returned success but object does not exist yet” surprises.

## Identity model and authz boundary
The control plane enforces tenancy. The base ownership boundary is an org (tenant).

Conceptual entities:
- Org
- Project (optional grouping)
- App
- Environment
- Release
- Process type
- Route
- Secret bundle and secret version
- Volume, snapshot, backup record
- Node (host) and node identity
- Allocation (desired placement) and instance records

Auth principles:
- Every API call is authenticated.
- Every action is authorized against org ownership and scoped permissions.
- Sensitive operations (secrets, routes, node enrollment, admin) require narrow scopes.

Exact auth mechanics are specified in `docs/specs/api/auth.md`.

## Command handling: validation then event append
A write request is a command.

Command lifecycle:
1) Authenticate and authorize.
2) Validate request shape and invariants using current materialized state.
3) Assign idempotency key (client-provided or server-generated).
4) Append one or more events in a single Postgres transaction.
5) Return:
   - immediate response for eventually-consistent endpoints, or
   - wait-for-projection and then return view-based object for read-your-writes endpoints.

Invariants are enforced at multiple layers:
- application validation
- database constraints for ordering and uniqueness (aggregate sequencing)
- projection idempotency and checkpointing

## Scheduler and reconciliation responsibilities
The scheduler consumes desired configuration (from views) and produces desired allocations (events).

Scheduler inputs:
- env configuration and desired scale per process type
- resource requests (cpu soft, memory hard)
- node capacity and allocatable budgets
- volume locality constraints
- routing requirements (which instances must exist for a route to be healthy)

Scheduler outputs:
- allocation events that assign instances to specific nodes
- drain/evict events during maintenance
- reschedule decisions when nodes are degraded or missing

Scheduler guarantees (v1 intent):
- Memory caps are never oversubscribed on a node.
- CPU requests are treated as soft and may be oversubscribed by a configured ratio.
- Volume attachments constrain placement.
- One process type per microVM instance.

See: `docs/specs/scheduler/*` and `docs/specs/workload-spec.md`.

## Change distribution to agents and edge
Agents and edge need to react to control plane state changes.

Model:
- Consumers maintain a cursor (event id).
- Control plane supports:
  - streaming updates for steady-state operation
  - polling by cursor for reconnect and recovery

Distribution requirements:
- At-least-once delivery.
- Idempotent consumers.
- Server-side filtering so nodes do not ingest irrelevant events.

Edge consumers care about:
- Routes and their bindings
- Backend health signals if edge participates in health gating
- PROXY protocol enablement flags per route

Agents care about:
- Allocations targeting that node
- Secret version bindings for instances on that node
- Volume attach/detach intents for that node

## Node enrollment
The control plane is the authority for which hosts are members of the platform.

Enrollment requirements:
- Nodes have stable identities.
- Joining requires operator intent (enrollment token or manual approval).
- Node keys and allowed IPs are distributed by control plane.
- Key rotation and revocation are supported.

Exact mechanics belong in `docs/specs/networking/overlay-wireguard.md` and the control plane API specs.

## Failure modes and degraded behavior
### Control plane down
- No new deploys, no scale changes, no route updates.
- Existing workloads and edge routing should continue operating using their last applied config.
- Agents continue running current instances.

This is a hard requirement: data plane must not require constant control plane availability to serve traffic.

### Projections lagging
- API may return stale reads for eventually-consistent endpoints.
- Read-your-writes endpoints may block until projections catch up or time out with a clear error.

### Scheduler lagging
- Desired state changes may not translate into allocations immediately.
- Agents do not invent state. They wait for allocations.

### Database failure
- Control plane becomes unavailable.
- Recovery depends on Postgres backup and failover posture.
- Restore drills and runbooks are mandatory.

## Security posture
Control plane holds sensitive tenant state:
- encrypted secrets at rest
- routing ownership and hostname bindings
- audit logs

Requirements:
- Strict authn and authz on every endpoint.
- Audit trail for high-risk operations (secrets, routes, ipv4 allocations, node enrollment).
- Principle of least privilege for internal components.
- No raw secret material in logs.

See: `docs/security/*` and `docs/specs/secrets/encryption-at-rest.md`.

## Operational requirements
- Schema migrations are versioned and gated.
- Projections support replay and backfill.
- Backups and restore drills are scheduled and tracked.
- Admin tooling exists to:
  - inspect the event log for an aggregate
  - rebuild projections
  - detect and remediate stuck states

## Interfaces owned by control plane
- Public HTTP API and auth: `docs/specs/api/*`
- Event log contract: `docs/specs/state/*`
- Scheduler-to-agent contract: `docs/specs/workload-spec.md`
- Route model and ingress config: `docs/specs/networking/*`
- Secrets model and delivery semantics: `docs/specs/secrets/*`
- Storage metadata and lifecycle: `docs/specs/storage/*`

## Next document
- `docs/architecture/02-data-plane-host-agent.md`

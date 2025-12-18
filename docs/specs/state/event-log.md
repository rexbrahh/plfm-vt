# docs/specs/state/event-log.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
This document defines the control plane event log contract:
- ordering guarantees
- idempotency keys and dedup rules
- retention and immutability rules
- replay rules for projections, agents, and edge consumers

Locked decision: source of truth is an append-only event log plus materialized views. See `docs/ADRs/0005-state-event-log-plus-materialized-views.md`.  
Locked decision: control plane DB is Postgres. See `docs/ADRs/0006-control-plane-db-postgres.md`.

## Scope
This spec defines the event log as a storage and streaming interface.

This spec does not define:
- the full event catalog (see `docs/specs/state/event-types.md`)
- the views and projection schemas (see `docs/specs/state/materialized-views.md`)
- scheduler internals (see `docs/specs/scheduler/*`)

## Definitions
- **Event**: an immutable record of a validated state transition.
- **Aggregate**: a logical entity whose state is derived from events, identified by `(aggregate_type, aggregate_id)`.
- **Global order**: total order of all events by `event_id`.
- **Aggregate order**: strict order of events for one aggregate by `aggregate_seq`.
- **Projection**: code that consumes events and updates materialized views.
- **Consumer**: any reader of the event log stream (projections, agents, edge).

## Core invariants
1) **Append-only**
- Events are never updated or deleted in-place.
- Any correction is represented by new events.

2) **Two ordering dimensions**
- Every event has a globally monotonic `event_id` (global order).
- Every event belongs to exactly one aggregate and has a monotonic `aggregate_seq` for that aggregate (aggregate order).

3) **Replayable**
- All materialized views must be rebuildable from the event log.
- Consumers must be able to resume from a cursor.

4) **No secret material**
- Events must never contain raw secret values.
- Events may contain secret version identifiers and metadata only.

## Event schema (canonical, v1)
This is the minimum required shape. Postgres column types are recommended, not mandated.

### Required fields
- `event_id` (int64, globally monotonic, required)
- `occurred_at` (timestamp with timezone, required)
- `aggregate_type` (string, required)
- `aggregate_id` (string, required, opaque id)
- `aggregate_seq` (int32, required, starts at 1 for each aggregate)
- `event_type` (string, required)
- `event_version` (int32, required, starts at 1 for each event_type)
- `actor_type` (string, required, `user` | `service_principal` | `system`)
- `actor_id` (string, required)
- `org_id` (string, required when aggregate is tenant-scoped)
- `request_id` (string, required)
- `idempotency_key` (string, optional but strongly recommended for all user-triggered commands)
- `payload` (json object, required)

### Optional fields (recommended)
- `app_id` (string)
- `env_id` (string)
- `correlation_id` (string, for grouping, example deploy_id)
- `causation_id` (string, event_id of the event that caused this event, if applicable)

## Storage model in Postgres (recommended)
### Table: `events`
Recommended columns:
- `event_id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY`
- `occurred_at TIMESTAMPTZ NOT NULL DEFAULT now()`
- `aggregate_type TEXT NOT NULL`
- `aggregate_id TEXT NOT NULL`
- `aggregate_seq INT NOT NULL`
- `event_type TEXT NOT NULL`
- `event_version INT NOT NULL`
- `actor_type TEXT NOT NULL`
- `actor_id TEXT NOT NULL`
- `org_id TEXT NULL` (required for tenant aggregates)
- `request_id TEXT NOT NULL`
- `idempotency_key TEXT NULL`
- `payload JSONB NOT NULL`

### Constraints (required)
- `UNIQUE (aggregate_type, aggregate_id, aggregate_seq)`

This enforces aggregate ordering and prevents two conflicting "next events" for the same aggregate.

### Indexes (recommended v1)
- `PRIMARY KEY (event_id)`
- `INDEX (aggregate_type, aggregate_id, aggregate_seq DESC)` for latest-by-aggregate queries
- `INDEX (org_id, event_id)` for org-scoped tail queries
- `INDEX (event_type, event_id)` for consumers filtering by type

Do not partition in v1 unless proven necessary.

### Immutability enforcement (required)
At the database privilege level:
- The application role must not have `UPDATE` or `DELETE` on `events`.
- Only `INSERT` and `SELECT` are permitted.

Optional defense-in-depth:
- A trigger that raises an exception on update/delete attempts.

## Ordering guarantees
### Global ordering
- `event_id` defines a total order of all events.
- Consumers must process events in ascending `event_id` order.

Important notes:
- `occurred_at` is informational and must not be used as the primary order.
- If multiple events share the same timestamp, `event_id` still provides deterministic ordering.

### Aggregate ordering
- For a fixed `(aggregate_type, aggregate_id)`, events are ordered by increasing `aggregate_seq`.
- The control plane must assign `aggregate_seq` values without gaps unless an explicit "reserved sequence" feature is introduced (not in v1).

### Cross-aggregate ordering
There is no semantic guarantee beyond `event_id`. If one command touches multiple aggregates, the global ordering reflects append order, but consumers must not infer business meaning from interleaving across aggregates unless the event types define it.

## Idempotency and deduplication
Idempotency must be handled at the command layer and reflected in events for auditability.

### Command idempotency keys
- Clients SHOULD send `Idempotency-Key` for write endpoints.
- The server MUST enforce deduplication for all endpoints that are likely to be retried (deploy, route create/update, secrets update, volume attach, scale changes).

Scope of idempotency keys:
- `(org_id, actor_id, endpoint_name, idempotency_key)` is the dedup key.

### Storage for idempotency records (recommended)
Table: `idempotency_records`
- `org_id`
- `actor_id`
- `endpoint_name`
- `idempotency_key`
- `request_hash` (hash of normalized request body)
- `response_body` (or pointer to stored response)
- `created_at`

Rules:
- If key reused with same request_hash, return stored response.
- If key reused with different request_hash, return `409 conflict` with code `idempotency_key_reuse`.

Retention for idempotency records:
- Minimum 24 hours in v1.
- Can be longer, but must be bounded.

### Event-level idempotency
Events include `request_id` and optionally `idempotency_key` for traceability.
Consumers must not rely on `idempotency_key` to deduplicate events. Consumers deduplicate by `event_id` cursoring and by idempotent projection logic.

## Replay rules
Replay is a first-class requirement.

### Consumer cursoring
Every consumer maintains:
- `cursor_event_id` (the last fully processed event_id)

Processing rule:
- Consumer reads events where `event_id > cursor_event_id` in ascending order.
- Consumer applies each event exactly once from its perspective.
- Consumer persists cursor updates only after the event is fully processed and its effects are durable.

At-least-once delivery:
- The system may deliver the same event to a consumer more than once via retries.
- Consumers must be idempotent.

### Projection replay
Projection workers must support:
- rebuild from scratch: set cursor to 0, truncate projection tables, replay full log
- incremental catch-up: resume from last cursor

Projection checkpointing (required)
Store per-projection cursor:
- `projection_name -> last_applied_event_id`

This can be in a table like `projection_checkpoints`.

### Agent and edge replay
Agents and edge components must:
- maintain their own cursor
- be able to reconnect and catch up
- tolerate duplicate delivery
- apply updates idempotently

They must treat the control plane event stream as authoritative for desired state changes.

## Retention policy
### v1 policy (required stance)
- The event log is retained indefinitely in v1.
- Any future retention or deletion policy requires an ADR and explicit operational tooling.

Reason:
- Event history is needed for auditability, debugging, and rebuilding views.

### Data minimization rule
Because retention is long, the event payload must be designed to avoid sensitive material.
- No secrets values
- No private keys
- No full request bodies that may contain secrets unless sanitized

## Migration and versioning rules
### Event type evolution
- `event_type` names are stable identifiers.
- Payload schema changes require incrementing `event_version`.

Compatibility:
- Projections must support at least the current version and the immediately previous version of each event type during rolling upgrades, unless a coordinated cutover is planned.

### Backfills
If you need derived data for older events:
- Do not rewrite old events.
- Either:
  - rebuild projections with new logic that can interpret old payloads, or
  - append explicit backfill events that are idempotent and clearly labeled.

## Access patterns
### Reading events (API / internal)
Consumers fetch by cursor:
- `after_event_id` and `limit`
- ordered by `event_id ASC`

Filtering:
- Filters may include `org_id`, `app_id`, `env_id`, `event_type`.
- Filtering must never break ordering. Filtered results still preserve ascending `event_id`.

### Audit queries
Audit queries may scan by `org_id` and time window, but should still expose `event_id` ordering.

## Security requirements
- Only trusted control plane services can append events.
- Tenants cannot append events directly.
- Tenants can read only org-scoped events they are authorized for.
- Do not expose infrastructure-only aggregates to tenants unless explicitly needed.

## Failure semantics
### Partial failure during command handling
Events are appended in a single DB transaction.
- Either all events for the command are committed, or none are.

### Projection lag
Projection lag is expected and observable.
- Read-your-writes endpoints may wait for projection checkpoints for bounded time.
- Eventually consistent reads may return stale state.

## Observability requirements (event log system itself)
Emit platform metrics:
- event append rate and latency
- event table size and growth rate
- projection lag per projection
- consumer cursor lag (where tracked)

## Open questions (must be resolved in later specs)
- Exact aggregate taxonomy (which aggregates exist and how they map to ids) belongs in `event-types.md`.
- Exact projection checkpoint storage schema belongs in `materialized-views.md`.

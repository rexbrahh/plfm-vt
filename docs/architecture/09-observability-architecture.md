# docs/architecture/09-observability-architecture.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
This document describes the observability architecture of the platform:
- logs
- metrics
- traces
- health signals
- debugging workflows

The goal is operational clarity: at any moment we can answer:
1) what is desired
2) what is actually running
3) what is routing to what
4) why the system is not converging

Authoritative details live in:
- `docs/specs/observability/*`
- `docs/specs/state/*`
- `docs/specs/scheduler/*`
- `docs/specs/runtime/*`
- `docs/specs/networking/*`
- `docs/ops/*`

## Observability stance
- The platform must be operable from day one.
- We value boring, consistent signals over fancy dashboards.
- Every critical loop (event append, projections, scheduler, agent reconcile, edge routing) must emit:
  - health status
  - latency metrics
  - error counters
  - enough context to debug without guesswork
- Tenant data access (logs, metrics) is scoped and auditable.

## Signal taxonomy
We split signals into:
1) **Control plane signals** (API, DB, projections, scheduler)
2) **Data plane signals** (agent, Firecracker lifecycle, host health)
3) **Edge signals** (routing config, backend reachability, connection stats)
4) **Tenant signals** (workload logs and optional workload metrics)

This matters because failure modes are different. We should not treat everything as one blob.

## Logging

### Platform logs (control plane, agent, edge)
Requirements:
- Structured logs (JSON) for platform components.
- Mandatory fields:
  - timestamp
  - component (control-plane, projector, scheduler, agent, edge)
  - node_id (if applicable)
  - request_id / trace_id
  - org_id (only when safe and needed)
  - resource ids (env_id, instance_id, route_id) as applicable
  - error class and error message
- No raw secrets ever.

Retention:
- Operator-defined, but must be explicit in docs/policy.

### Workload logs
Workload logs must be accessible via CLI by:
- org/app/env
- process type
- instance id
- time range

Capture mechanism:
- The runtime must define a standard capture channel (serial console and/or vsock).
- The agent collects and forwards logs.

Access control:
- Logs are tenant data.
- Access requires `logs:read` scope and is auditable.

Multi-tenant safety:
- Ensure logs cannot be queried across org boundaries.
- Apply limits:
  - max tail rate
  - max historical query window
  - max concurrent log streams

## Metrics

### Platform metrics (Prometheus-style)
Every platform component must expose metrics.

#### Control plane metrics
- HTTP request rate, error rate, latency
- event append rate, event append latency
- DB connection pool stats
- projection lag:
  - `projection_last_applied_event_id`
  - `projection_lag_events`
  - processing time per event type
- scheduler loop metrics:
  - loop duration
  - placement decisions count
  - failures and backoff counters

#### Agent metrics
- reconciliation loop duration and success/failure counts
- instances running by state
- Firecracker boot latency, failure counts
- image cache:
  - bytes used
  - hit rate
  - eviction counts
- cgroup stats per instance (bounded cardinality):
  - memory usage vs cap
  - CPU throttling stats
- disk pressure:
  - volume pool usage
  - image cache usage
- overlay health:
  - peer count
  - packet loss indicators if available
  - handshake recency

#### Edge metrics
- routes loaded count
- config reload success/failure
- backend sets per route
- connection rate and concurrent connections
- upstream connection failures
- backend reachability and health gating stats
- PROXY v2 enabled routes count

### Cardinality rules (non-negotiable)
High-cardinality labels will kill metrics systems.

Rules:
- Do not label metrics with unbounded identifiers (hostnames, user ids, request urls).
- Instance-level metrics are allowed only if bounded by quotas and sampling or aggregated forms exist.
- Prefer aggregation by:
  - org_id (bounded)
  - env_id (bounded)
  - process_type (bounded)
  - node_id (bounded)

If we want per-instance details, we use logs or an explicit “describe instance” endpoint, not high-cardinality metrics.

## Tracing
### Control plane tracing
Tracing is mandatory for control plane internal calls:
- API handler -> command validation -> event append -> projection wait -> response
- scheduler loop -> allocation decisions -> event appends
- change stream distribution to consumers

Trace requirements:
- consistent trace_id propagation
- spans for DB operations and external calls
- sampling policy defined (default: sample errors heavily, sample successes lightly)

### Data plane tracing
Agent and edge tracing is optional in v1 but recommended for:
- instance boot path
- image pull and conversion
- config apply path
- exec session setup

## Health and readiness
We must distinguish:
- liveness: process is up
- readiness: component can do useful work

### Control plane readiness
- DB connectivity ok
- event append ok
- projections running (or at least not catastrophically behind)
- scheduler running (optional for partial readiness)

### Agent readiness
- enrolled and authenticated
- overlay up
- can launch microVMs (KVM ok, disk ok)
- can report heartbeats

### Edge readiness
- route config loaded
- can reach overlay
- can establish upstream connections to at least some backends
- safe reload functional

## Debug workflows (must be supported)
These workflows define what tooling we need.

### 1) Why is my deploy not serving traffic?
Answer requires:
- desired release for env/process
- instance states and host assignments
- route bindings and backend set for the route
- edge reachability to backends
- health check outcomes

We need CLI commands (or API endpoints) that show:
- `platform describe env`
- `platform describe route`
- `platform describe instance`
- `platform events tail --env ...`

### 2) Why is the system not converging?
Answer requires:
- event cursor positions for scheduler, agents, edge
- projection lag
- reconcile loop errors and backoffs

We need:
- projection lag dashboard
- agent reconcile error logs
- edge config apply status

### 3) Why did an instance restart?
Answer requires:
- instance lifecycle events (allocated, booted, ready, stopped)
- termination reason (OOM, crash, operator action, upgrade)
- resource usage near death (memory cap exceeded, CPU throttled)

We need:
- structured reason codes and consistent reporting from agent to control plane

### 4) What changed recently?
Answer requires:
- audit events filtered by org/env/app/route
- “diff-like” summary for high-level changes

We need:
- event log query endpoints and CLI wrappers

## Tenant observability stance
- v1 supports workload logs as a first-class feature.
- Workload metrics and tracing are optional and should not be required for the platform to function.
- Provide a path for users to ship their own metrics/traces externally.

## Storage and retention
We need explicit retention for:
- platform logs
- workload logs
- metrics (Prometheus retention)
- traces (if stored)
- audit events (event log is permanent unless an explicit policy is adopted later)

The platform must never silently discard the event log without an intentional retention policy and an ADR.

## Alerts (baseline)
Minimum alerts to avoid flying blind:
- control plane API down
- Postgres down / replication broken
- event append error spike
- projection lag beyond threshold
- scheduler loop stalled
- edge config apply failing
- edge cannot reach backends
- agent heartbeat missing
- host disk pressure high
- backup failures

## Next steps
Create the authoritative specs:
- `docs/specs/observability/logging.md`
- `docs/specs/observability/metrics.md`
- `docs/specs/observability/alerts.md`
- `docs/specs/observability/dashboards.md`
and wire each requirement to a concrete metric, log field, or runbook action.

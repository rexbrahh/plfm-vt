# docs/specs/observability/logging.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
Define logging for the platform:
- log types (platform vs workload)
- required log schema and fields
- log collection and shipping pipeline (v1)
- retention policies and limits
- access control and tenant isolation
- safety rules (no secrets, bounded cardinality)

This spec is authoritative for logging behavior.

Related:
- overall observability architecture: `docs/architecture/09-observability-architecture.md`
- API log endpoints: `docs/specs/api/http-api.md` and `docs/specs/api/openapi.yaml`

## Scope
This spec defines log format and pipeline for:
- control plane services
- scheduler and projection workers
- node agents
- edge nodes
- tenant workload logs

This spec does not define:
- metrics and alerts (separate specs)
- full tracing (future spec or internal standard)

## Definitions
- **Platform logs**: logs emitted by control plane, scheduler, projections, agents, edge.
- **Workload logs**: stdout/stderr (or equivalent) produced by tenant processes inside microVMs.
- **Log line**: a single emitted record, not necessarily a single newline from source, but presented as one entry to consumers.

## v1 stance
1) Platform logs are structured JSON.
2) Workload logs are treated as opaque text lines, but shipped with structured metadata.
3) Logs are tenant data. Strict org isolation is required.
4) No raw secret material in logs.
5) Logging pipeline must be robust enough for debugging v1 incidents. Fancy querying is optional; correctness and retrieval are mandatory.

## Log categories
### A) Platform logs (structured)
Emit as JSON objects.

Producers:
- control plane API
- auth service
- projection workers
- scheduler
- node agent
- edge ingress

### B) Workload logs (text + metadata)
Captured from microVM and shipped with metadata:
- org_id, app_id, env_id, process_type, instance_id
- timestamp
- stream: stdout or stderr
- line text

Workload logs are not parsed for structure by default in v1.

## Platform log format (normative)
Platform components MUST log JSON objects with these fields:

### Required fields
- `ts` (RFC3339 timestamp with timezone)
- `level` (string: `debug`, `info`, `warn`, `error`)
- `component` (string: `control-plane`, `scheduler`, `projector`, `agent`, `edge`)
- `msg` (string)
- `request_id` (string, required for request-scoped logs; empty allowed otherwise)
- `trace_id` (string, optional but recommended if tracing exists)

### Context fields (optional but strongly recommended)
- `org_id`
- `app_id`
- `env_id`
- `process_type`
- `instance_id`
- `route_id`
- `volume_id`
- `snapshot_id`
- `restore_id`
- `node_id`
- `deploy_id`
- `release_id`

### Error fields (required for level=error)
- `error` (string, human-readable)
- `error_code` (stable string if available)
- `retryable` (bool, if applicable)

### Sanitization rules (mandatory)
Platform logs MUST NOT include:
- plaintext secret values
- ciphertext blobs, wrapped keys, master keys
- access tokens, refresh tokens, client secrets
- private TLS keys
- full request bodies that may contain secrets

If a log includes a key that matches common secret patterns, it must be redacted.
v1 minimum:
- avoid logging request bodies entirely for secrets endpoints
- log only metadata like secret version ids and hashes

## Workload log capture (normative)
### Capture source
The platform must define a single capture mechanism. v1 acceptable options:
- capture from microVM serial console
- capture via vsock log channel
- capture via a virtio device

v1 recommendation:
- use serial console for simplest bootstrapping, and keep it bounded.

### Capture semantics
- Each workload process outputs to stdout/stderr.
- Guest init should forward or ensure outputs reach the capture channel.
- Agent reads from the capture channel and timestamps log entries.

### Log entry format
When stored/shipped, each workload log entry has:
- `ts` (timestamp set by agent on receive, RFC3339)
- `org_id`
- `app_id`
- `env_id`
- `process_type`
- `instance_id`
- `node_id`
- `stream` (`stdout` or `stderr`, optional if not distinguishable)
- `line` (string, may be empty but should not be null)

Line size limits:
- v1 recommended max line length: 16 KiB
- if longer, truncate and append `…` and set `truncated=true`

### Ordering
- Ordering is best-effort by timestamp and receive order.
- Exact total ordering across nodes is not guaranteed, but per-instance order should be preserved as much as possible.

## Shipping pipeline (v1)
v1 has two layers:
1) per-node capture and buffering
2) centralized storage or relay for retrieval

### Agent buffering requirements
Agent must maintain a bounded buffer per instance:
- ring buffer on disk or in memory
- v1 recommendation: in memory + spill to local disk under size cap

Minimum retention at node level:
- at least the last N lines per instance (example: 10k lines), bounded.

If central storage is unavailable, agent still allows short-term tailing from local buffers.

### Central storage options (v1)
You can choose one of:
- A) store logs in Postgres (not recommended beyond tiny scale)
- B) ship to a log backend (Loki recommended) via an agent shipper (Vector)
- C) implement a minimal internal log store (not recommended for v1)

v1 recommendation:
- Vector on nodes -> Loki backend
- Grafana for exploration later
- Control plane provides a query and tail interface that maps to Loki queries.

This keeps platform code small.

### Control plane role
The control plane provides:
- auth and org isolation for log queries
- stable API endpoints for:
  - query logs (bounded window)
  - stream tail (SSE or WebSocket)
- mapping from logical selectors (env_id, instance_id) to log labels in backend

The control plane must not bypass tenant isolation by issuing overly broad backend queries.

## Retention policy (v1)
Retention must be explicit.

### Workload logs
v1 recommended defaults:
- retain 7 days for workload logs
- cap total storage per org (quota-based), example:
  - max log bytes per org per day
  - max query window per request

If retention is shorter for cost reasons, document it clearly.

### Platform logs
v1 recommended:
- retain 14 days for platform logs, or align with your ops needs
- platform logs are operator-only by default

### Local buffering
- bounded ring buffer per instance for resilience
- eviction policy: oldest first

## Query and streaming semantics
### Query logs (HTTP)
- supports filters:
  - env_id, process_type, instance_id
  - time window (since/until)
  - tail_lines

Constraints:
- enforce max query window (example: 1 hour) unless privileged operator scope
- enforce max results count

### Stream logs
- streaming endpoint tails logs forward
- best-effort delivery
- enforce max concurrent streams per org and per token

v1 recommended transport:
- SSE (`text/event-stream`) for simplicity.

## Access control and auditing
Workload logs are tenant data.

Requirements:
- Caller must have `logs:read` scope for the org.
- Queries are org-scoped by path.
- Log access should be auditable at least at coarse level:
  - actor_id, org_id, selectors, time window, request_id

Operator access:
- platform logs require operator scopes.
- cross-org log access is forbidden unless explicit operator emergency tooling exists and is audited.

## Cardinality rules
Logging labels used for indexing must be bounded.

v1 recommended label set for workload logs:
- org_id (bounded)
- env_id (bounded)
- process_type (bounded)
- instance_id (bounded by org quota)
- node_id (bounded)

Do not index by:
- full hostname
- request URL
- arbitrary user ids

Those can appear in log line content, but are not used as labels.

## Failure modes
### Log backend down
- agents buffer locally up to limits
- tailing may work from local buffers via agent relay
- central query may be unavailable
- emit alerts for backend unavailability

### Agent down
- logs for instances on that node may be temporarily unavailable
- once agent restarts, it resumes shipping

### Log storms
- enforce rate limits and per-org quotas
- agent can drop logs if buffers are full, but must:
  - emit platform log about dropping
  - increment drop counters
  - surface to tenant as “logs truncated/dropped” metadata

## Required metrics for logging system
- log lines ingested per second (by component and by org)
- log bytes ingested per second
- dropped lines count
- backend write failures
- query request rates and latencies
- stream connection counts

## Compliance tests (required)
1) Workload produces stdout logs, CLI can tail them.
2) Org isolation: org A cannot query org B logs.
3) Secrets are not logged by secrets endpoints.
4) Under backend outage, agent buffers and recovers without crashing.
5) Log line truncation works and is signaled.

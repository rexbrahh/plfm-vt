# Monitoring and oncall

This document defines our observability standards and oncall expectations.

## Goals

- Detect customer impact quickly and accurately
- Reduce time to mitigate via clear dashboards and runbooks
- Keep oncall sustainable: fewer, better alerts

## Observability stack expectations

Tooling can change. The requirements should not.

Minimum capabilities:

- Metrics: counters, gauges, histograms, labels for tenancy and region
- Logs: structured logs with correlation IDs, searchable by host and instance
- Traces: end-to-end request tracing for control plane and edge
- Events: durable event stream for state transitions and operator actions

## Standard identity and correlation

All telemetry should include:

- `region`
- `service`
- `host_id` (if applicable)
- `org_id`, `project_id`, `app_id`, `env_id` (when tenant scoped)
- `workload_id`, `instance_id`, `release_id` (when lifecycle scoped)
- `request_id` and `trace_id` (for distributed correlation)

## What we monitor

### Golden signals (symptoms)

- Latency: p50/p95/p99 for API and connect paths
- Traffic: requests per second, connections, bytes
- Errors: 5xx, timeouts, connect failures, failed deploys
- Saturation: CPU, memory, disk, queue depth, Postgres connections

### Platform specific signals

- Reconciliation backlog and retries
- Time-to-converge SLI (desired to ready)
- Image fetch latency and cache hit rate
- Secrets delivery freshness (desired version vs applied version)
- WireGuard peer health (handshake age, drops)
- Firecracker lifecycle failures (start, stop, snapshot restore)

## Alerting policy

### Pager eligibility

A page is allowed only if:

- it indicates current or imminent customer impact
- it has a linked runbook with concrete actions
- it is deduped and rate-limited
- it has an owner who agrees to maintain it

Otherwise it is a ticket.

### Alert hygiene rules

- Alerts must be actionable within 5 minutes of waking up.
- Prefer SLO burn-rate alerts over component health alerts.
- Avoid paging on host CPU or memory unless proven predictive of impact.
- Every alert includes: summary, impact, scope, first steps, and runbook link.

## Oncall roles

- **Primary oncall**: receives pages, triages, mitigates, opens incidents
- **Secondary oncall**: backup, helps in active incident, takes overflow
- **Incident commander (IC)**: assigned during Sev0/Sev1 incidents

## Oncall expectations

- Acknowledge pages quickly, then assess impact.
- If impact is unclear, assume impact and investigate.
- Use runbooks first. Deviate only with a written note in the incident timeline.
- Keep the incident log updated (what, when, why).

## Shift handoff checklist

At the start of shift:

- review current status dashboard
- review ongoing incidents and mitigations
- review outstanding tickets created by alerts
- confirm paging system is reachable (phone, laptop, VPN)

At end of shift:

- hand off ongoing investigations with a short summary
- ensure incident timelines are up to date
- open follow-up tickets for unfinished work

## Dashboard minimum set

- Global overview (by region): SLOs, API availability, edge connect success
- Control plane: API, scheduler, reconciler, Postgres
- Edge: connect success, resets, timeouts, per-endpoint health
- Host fleet: host health, drain/cordon status, microVM failures
- Overlay network: peer health, RTT, packet loss, MTU anomalies
- Storage: volume errors, attach failures, snapshot/backup job status

## Runbook standards

Every runbook must include:

- symptoms and related alerts
- impact and scope questions
- safe mitigation steps
- verification steps
- escalation conditions
- follow-up actions

Runbooks must be tested in staging at least quarterly.

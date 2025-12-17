# docs/specs/observability/alerts.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
Define alert rules and paging thresholds for the platform:
- what conditions are alertable
- which alerts page vs which are ticket-only
- minimum viable alert set for v1
- alert labels and routing conventions
- how alerts map to runbooks

This spec is authoritative for v1 alerting.

Related:
- metrics: `docs/specs/observability/metrics.md`
- dashboards: `docs/specs/observability/dashboards.md`
- incident response: `docs/ops/04-incident-response.md` (planned)
- runbooks: `docs/ops/runbooks/*`

## Scope
This spec defines operator-facing alerts.

It does not define:
- SLA/SLO policy (separate ops doc)
- customer paging policies

## Alerting stance (v1)
1) Page only on alerts that indicate likely customer impact or imminent data loss.
2) Everything else is ticket-level or warning-level.
3) Alerts must be actionable:
- include a clear title
- include the likely cause
- include an immediate mitigation step
- link to a runbook
4) Avoid alert spam:
- use appropriate for durations and thresholds
- avoid high-cardinality alert labels

## Alert severities
- `page` (critical): wake someone up
- `ticket` (high): requires attention during working hours
- `warn` (low): informational, may not require action

## Global alert label conventions
Every alert should include:
- `severity`: page|ticket|warn
- `component`: control-plane|db|scheduler|projector|agent|edge|storage|secrets|auth
- `runbook`: relative path to a runbook in docs/ops/runbooks

Optional labels:
- `node_id` (only when alert is node-scoped and node count is bounded)
- `projection` (bounded)
- `reason` (bounded)

Forbidden labels:
- org_id (unless you explicitly want per-tenant paging, not v1)
- env_id (can be high cardinality)
- instance_id (too high churn)

## Minimum alert set (v1)

## A) Control plane availability and errors

### A1) Control plane API down (PAGE)
Condition:
- No successful API requests / health endpoint failures across all replicas

Metric basis:
- `trc_http_requests_total` or a dedicated `up` metric from Prometheus scrape

Rule (example):
- page if `up{job="control-plane"} == 0` for 2 minutes

Severity:
- page

Runbook:
- `docs/ops/runbooks/control-plane-down.md`

---

### A2) API error rate spike (TICKET)
Condition:
- sustained increase in 5xx responses

Metric basis:
- `trc_http_requests_total{status=~"5.."}`

Rule:
- ticket if 5xx rate > 1% for 10 minutes (tune later)

Severity:
- ticket

Runbook:
- `docs/ops/runbooks/control-plane-down.md` (or a dedicated api-errors runbook)

---

## B) Postgres and database health

### B1) Postgres down or unreachable (PAGE)
Condition:
- DB connection failures or Postgres exporter down

Metric basis:
- Postgres exporter `up` or control plane DB error counters (if emitted)

Rule:
- page if Postgres `up == 0` for 2 minutes

Severity:
- page

Runbook:
- `docs/ops/runbooks/postgres-failover.md`

---

### B2) DB connection pool exhausted (TICKET)
Condition:
- DB connections in use near max for sustained window

Metric basis:
- `trc_db_connections_in_use`, `trc_db_connections_max`

Rule:
- ticket if in_use / max > 0.9 for 10 minutes

Severity:
- ticket

Runbook:
- `docs/ops/runbooks/control-plane-down.md`

---

## C) Event log and projections

### C1) Projection lag high (PAGE or TICKET depending on threshold)
Condition:
- critical projections are far behind event log, likely causing deploy/routing issues

Metric basis:
- `trc_projection_lag_events{projection=...}`

Rule:
- ticket if lag > 5,000 for 10 minutes
- page if lag > 50,000 for 10 minutes
(Thresholds must be tuned based on event rate.)

Severity:
- ticket/page based on threshold

Runbook:
- `docs/ops/runbooks/control-plane-down.md` (or a dedicated projection-lag runbook)

---

### C2) Projection errors sustained (TICKET)
Condition:
- projection error counter increasing

Metric basis:
- `trc_projection_errors_total{projection}`

Rule:
- ticket if errors rate > 0 for 10 minutes (or > N/min)

Severity:
- ticket

Runbook:
- `docs/ops/runbooks/control-plane-down.md`

---

## D) Scheduler health

### D1) Scheduler loop stalled (PAGE)
Condition:
- reconcile loop not running or stuck

Metric basis:
- `trc_scheduler_reconcile_runs_total`

Rule:
- page if increase over 5 minutes is 0

Severity:
- page

Runbook:
- `docs/ops/runbooks/control-plane-down.md` (or scheduler-stuck runbook)

---

### D2) Unschedulable spike (TICKET)
Condition:
- large number of unschedulable decisions, possibly capacity exhaustion or bug

Metric basis:
- `trc_scheduler_unschedulable_total{reason}`

Rule:
- ticket if unschedulable rate > threshold for 15 minutes
- include reason breakdown in dashboard

Severity:
- ticket

Runbook:
- `docs/ops/runbooks/host-degraded.md` and capacity planning doc

---

## E) Edge ingress

### E1) Edge down (PAGE)
Condition:
- all edge nodes down or not scraping

Metric basis:
- `up{job="edge"} == 0`

Rule:
- page if all edge targets are down for 2 minutes

Severity:
- page

Runbook:
- `docs/ops/runbooks/edge-partial-outage.md`

---

### E2) Edge config apply failing (TICKET)
Condition:
- repeated config apply failures

Metric basis:
- `trc_edge_config_apply_total{result="error"}`

Rule:
- ticket if failures > 0 for 10 minutes

Severity:
- ticket

Runbook:
- `docs/ops/runbooks/edge-partial-outage.md`

---

### E3) Upstream connect failures spike (PAGE)
Condition:
- edge cannot connect to backends, likely overlay partition or node failures

Metric basis:
- `trc_edge_upstream_connect_failures_total{reason}`

Rule:
- page if failure rate crosses threshold for 5 minutes

Severity:
- page

Runbook:
- `docs/ops/runbooks/wireguard-partition.md`

---

### E4) SNI sniff failures spike (TICKET)
Condition:
- a large fraction of TLS connections have no SNI or sniff failures

Metric basis:
- `trc_edge_sni_sniffs_total{result}`

Rule:
- ticket if `no_sni` or `timeout` grows unusually fast for 15 minutes

Severity:
- ticket

Runbook:
- edge runbook and docs on non-SNI clients

---

## F) Node agents and fleet health

### F1) Agent heartbeat missing (TICKET / PAGE if large)
Condition:
- agent not reporting or Prometheus cannot scrape it

Metric basis:
- `up{job="agent"}` or agent heartbeat metric

Rule:
- ticket if any single node down for 10 minutes
- page if > X% of nodes down for 5 minutes

Severity:
- ticket/page based on blast radius

Runbook:
- `docs/ops/runbooks/host-degraded.md`

---

### F2) Instance boot failure spike (TICKET)
Condition:
- many failures to start instances

Metric basis:
- `trc_agent_instance_failures_total{reason}`

Rule:
- ticket if failure rate > threshold for 10 minutes
- investigate reasons: image pull, firecracker start, network setup, secrets, volume attach

Severity:
- ticket

Runbook:
- `docs/ops/runbooks/firecracker-failure.md`

---

### F3) OOM kills spike (PAGE)
Condition:
- workloads repeatedly hitting memory hard caps, likely customer-impacting

Metric basis:
- `trc_agent_instance_failures_total{reason="oom_killed"}`

Rule:
- page if OOM kill rate > threshold for 10 minutes (tune)
- or ticket if lower

Severity:
- page/ticket depending on volume

Runbook:
- `docs/ops/runbooks/host-degraded.md` plus customer guidance

---

## G) Storage, backups, and restore

### G1) Volume pool disk pressure (PAGE)
Condition:
- thin pool free bytes below threshold or disk pressure flagged

Metric basis:
- `trc_agent_volume_pool_free_bytes`
- `trc_agent_disk_pressure{kind="volume_pool"}`

Rule:
- page if free < 10% for 10 minutes
- ticket if free < 20% for 30 minutes

Severity:
- page/ticket

Runbook:
- `docs/ops/runbooks/volume-corruption.md` (and capacity planning)

---

### G2) Backup failures sustained (TICKET)
Condition:
- backups failing repeatedly

Metric basis:
- `trc_agent_backups_total{result="failed"}`

Rule:
- ticket if failures > 0 for 30 minutes
- page if failures persist > 4 hours or if "no successful backup in > X days" triggers

Severity:
- ticket/page depending on duration

Runbook:
- `docs/ops/07-backup-restore-runbook.md`

---

### G3) No successful backup for too long (PAGE)
Condition:
- at least one volume has not had a successful backup in > X days

Metric basis:
- This is hard to do purely with Prometheus without per-volume labels.
v1 recommendation:
- implement an operator job that computes this daily and emits a bounded metric:
  - `trc_backup_staleness_max_days` (max across volumes)
  - `trc_backup_stale_volumes_count` (count of volumes beyond threshold)

Rule:
- page if `trc_backup_stale_volumes_count > 0` for 1 hour

Severity:
- page

Runbook:
- `docs/ops/07-backup-restore-runbook.md`

---

### G4) Restore failures spike (TICKET)
Condition:
- restore operations failing

Metric basis:
- `trc_restore_failures_total` (if you add it) or reuse backup/agent metrics

Rule:
- ticket if failures > threshold for 30 minutes

Severity:
- ticket

Runbook:
- `docs/ops/07-backup-restore-runbook.md`

---

## H) Secrets and auth

### H1) Secrets delivery failures spike (PAGE)
Condition:
- many instances failing due to secrets injection issues

Metric basis:
- `trc_agent_secrets_delivery_total{result="failed",reason=...}`

Rule:
- page if failure rate > threshold for 10 minutes
- immediate page if reason includes `master_key_unavailable`

Severity:
- page

Runbook:
- `docs/ops/runbooks/control-plane-down.md` and secrets runbook (to be added)

---

### H2) Auth token mint failures (TICKET)
Condition:
- token issuance failing

Metric basis:
- `trc_auth_token_mints_total{result="failed"}`

Rule:
- ticket if failing rate > threshold for 15 minutes

Severity:
- ticket

Runbook:
- auth runbook (to be added)

---

## Alert routing and ownership
- `component=db` goes to DB/oncall
- `component=edge` goes to networking/oncall
- `component=storage` goes to storage/oncall
- everything else goes to platform/oncall

If you have one oncall rotation, route all pages there.

## Runbook requirements
Every page alert must have a runbook link that includes:
- symptoms
- immediate mitigation
- verification steps
- escalation criteria

## Tuning guidance (v1 reality)
Initial thresholds are guesses. The correct approach:
- start with conservative paging (only clear outages and data-loss risks)
- review after first week of operation
- tune thresholds and add missing alerts based on incidents

## Compliance checklist (required)
1) Every page alert maps to a runbook file path.
2) No alert includes forbidden high-cardinality labels.
3) Alerts do not fire on single-sample blips (use `for:` windows).
4) Alerts cover:
- control plane down
- DB down
- projection lag
- scheduler stall
- edge down or cannot reach backends
- node agent down in bulk
- volume pool pressure
- backup staleness
- secrets delivery failures

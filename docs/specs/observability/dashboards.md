# docs/specs/observability/dashboards.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
Define the default Grafana dashboards shipped with the platform:
- which dashboards exist
- what they must show (minimum panels)
- which metrics and log links they rely on
- how dashboards stay stable as the platform evolves

This spec is authoritative for dashboard scope and minimum content, not pixel-perfect layout.

Metrics are defined in:
- `docs/specs/observability/metrics.md`

Alerts are defined in:
- `docs/specs/observability/alerts.md`

## Scope
This spec defines operator-facing dashboards for v1.

Tenant-facing dashboards are not part of v1 unless explicitly productized.

## v1 stance
1) Dashboards must answer the top operational questions quickly.
2) Dashboards must avoid unbounded cardinality and should not rely on per-instance graphs unless bounded.
3) Dashboards must link to logs and events for deeper debugging.
4) Dashboards are shipped as JSON and versioned with the platform.

## Dashboard list (v1 required)
1) **Control Plane Overview**
2) **Event Log and Projections**
3) **Scheduler Health**
4) **Edge Ingress**
5) **Node Fleet Overview**
6) **Node Drilldown**
7) **Storage and Backups**
8) **Secrets and Auth**
9) **Platform SLO Summary** (optional if SLOs are defined in v1)

Each dashboard has required panels below.

---

## 1) Control Plane Overview
Goal:
- Is the API up?
- Is it fast?
- Is it erroring?
- Is DB healthy?

Required panels:
- API request rate: `trc_http_requests_total` (rate)
- API error rate (4xx/5xx): request total filtered by status
- API latency p50/p95/p99: `trc_http_request_duration_seconds_bucket`
- DB connections in use/max: `trc_db_connections_in_use`, `trc_db_connections_max`
- Event append success rate: `trc_event_append_total{result}`
- Command failure counts (if present): `trc_commands_total{result}`

Links:
- control plane logs
- `/v1/orgs/{org}/events` query UI (or CLI instructions)

---

## 2) Event Log and Projections
Goal:
- Are projections keeping up?
- Are we falling behind?
- Which projection is stuck?

Required panels:
- Projection lag events per projection:
  - `trc_projection_lag_events{projection}`
- Last applied event id per projection:
  - `trc_projection_last_applied_event_id{projection}`
- Projection apply duration histogram:
  - `trc_projection_apply_duration_seconds_bucket{projection}`
- Projection error rate:
  - `trc_projection_errors_total{projection}` (rate)
- Events table growth (if exposed):
  - `trc_events_table_rows` or `trc_events_table_size_bytes`

Links:
- projection worker logs
- “rebuild projection” runbook link (docs/ops)

---

## 3) Scheduler Health
Goal:
- Is the scheduler running?
- Is it placing instances?
- Are things unschedulable and why?
- Are rollouts getting stuck?

Required panels:
- reconcile loop runs and duration:
  - `trc_scheduler_reconcile_runs_total`
  - `trc_scheduler_reconcile_duration_seconds_bucket`
- group reconcile outcomes:
  - `trc_scheduler_group_reconciles_total{result}`
- allocations and drains:
  - `trc_scheduler_instance_allocations_total{result}`
  - `trc_scheduler_instance_drains_total{reason}`
- unschedulable reasons:
  - `trc_scheduler_unschedulable_total{reason}` (rate)
- rollout duration and failures:
  - `trc_scheduler_rollout_seconds_bucket{result}`
  - `trc_scheduler_rollout_failures_total{reason}`

Links:
- scheduler logs
- event tail filtered by deploy_id/env_id (CLI or API)

---

## 4) Edge Ingress
Goal:
- Is ingress accepting connections?
- Are we routing correctly?
- Are backends reachable?
- Are SNI sniffs failing?
- Is config apply healthy?

Required panels:
- connection rate and concurrency:
  - `trc_edge_connections_total{listener_port,result}`
  - `trc_edge_concurrent_connections{listener_port}`
- upstream connect failures:
  - `trc_edge_upstream_connect_failures_total{reason}`
- routes loaded:
  - `trc_edge_routes_loaded`
- config apply success and duration:
  - `trc_edge_config_apply_total{result}`
  - `trc_edge_config_apply_seconds_bucket{result}`
- SNI sniff results:
  - `trc_edge_sni_sniffs_total{result}`
  - optional sniff duration
- PROXY v2 injection outcomes:
  - `trc_edge_proxy_v2_injections_total{result}`

Links:
- edge logs
- runbook for edge partial outage

---

## 5) Node Fleet Overview
Goal:
- Are nodes healthy and connected?
- Are agents running?
- What is overall capacity pressure?

Required panels:
- agent heartbeats / reconnects:
  - `trc_agent_heartbeat_total{result}`
  - `trc_agent_controlplane_reconnects_total`
- instances running and ready (fleet totals):
  - `sum(trc_agent_instances_running)` across nodes
  - `sum(trc_agent_instances_ready)` across nodes
- instance boot latency:
  - `trc_agent_instance_boot_seconds_bucket{result}`
- overlay peer health:
  - `trc_agent_overlay_peers`
  - `trc_agent_overlay_handshake_age_seconds_max`
- disk pressure signals:
  - `trc_agent_disk_pressure{kind}`
  - cache bytes by kind: `trc_agent_cache_bytes{kind}`
- volume pool usage:
  - `trc_agent_volume_pool_used_bytes`
  - `trc_agent_volume_pool_free_bytes`

Links:
- node drilldown dashboard (templated by node_id)

---

## 6) Node Drilldown
Goal:
- Investigate one node: why is it failing, full, slow, or partitioned?

Templating:
- `node_id` variable

Required panels:
- instances desired/running/ready on node:
  - `trc_agent_instances_desired{node_id=...}` (if labeled)
  - `trc_agent_instances_running`
  - `trc_agent_instances_ready`
- instance failures by reason:
  - `trc_agent_instance_failures_total{node_id=...,reason}` (rate)
- image pulls and root disk builds:
  - `trc_agent_image_pulls_total{result}`
  - `trc_agent_rootdisk_build_seconds_bucket{result}`
- cache usage and evictions:
  - `trc_agent_cache_bytes{kind}`
  - `trc_agent_cache_evictions_total{kind}`
- disk free and pressure:
  - `trc_agent_disk_free_bytes{mount}`
  - `trc_agent_disk_pressure{kind}`
- overlay health:
  - peer count and handshake age

Links:
- agent logs for node_id
- runbook for host degraded and wireguard partition

---

## 7) Storage and Backups
Goal:
- Are backups happening?
- Are snapshots failing?
- Are restores failing?
- Are we approaching thin pool exhaustion?

Required panels:
- snapshots count and duration:
  - `trc_agent_snapshots_total{result}`
  - `trc_agent_snapshot_seconds_bucket{result}`
- backups count, duration, bytes:
  - `trc_agent_backups_total{result}`
  - `trc_agent_backup_seconds_bucket{result}`
  - `trc_agent_backup_bytes_total{result}`
- time since last successful backup per volume (if you can expose it):
  - v1 may not have per-volume metric due to cardinality, so show:
    - “backup success rate” and “backup failures” and rely on logs/events for per-volume
- volume pool usage:
  - used/free bytes

Links:
- backup restore runbook
- volume corruption runbook
- restore workflow docs

---

## 8) Secrets and Auth
Goal:
- Are auth services healthy?
- Are secrets updates and deliveries succeeding?
- Are we seeing decrypt failures or master key issues?

Required panels:
- token mints and refresh:
  - `trc_auth_token_mints_total{grant_type,result}`
  - `trc_auth_token_refresh_total{result}`
- device flow poll failures:
  - `trc_auth_device_flow_polls_total{result}`
- secrets delivery success/failure:
  - `trc_agent_secrets_delivery_total{result,reason}`
  - `trc_agent_secrets_delivery_seconds_bucket{result}`
- decrypt failures (if tracked separately):
  - can be included as reason label above

Links:
- auth runbook
- secret handling security doc
- incident response doc

---

## 9) Platform SLO Summary (optional)
Goal:
- quick SLO view for operators

Requires SLO definitions in docs/ops.

Panels:
- API availability
- ingress availability
- deploy success rate
- projection lag SLO

This dashboard is optional until SLOs are locked.

## Dashboard packaging
- Dashboards are stored under:
  - `ops/grafana/dashboards/*.json` (suggested path)
- Each dashboard has a stable UID.
- Dashboards are versioned with the platform release.

## Dashboard hygiene rules
- Do not use unbounded labels.
- Use path templates, not raw URLs.
- Provide clear panel titles and units.
- Include links to logs and runbooks.
- Prefer fleet-level views, then drilldown by node_id.

## Compliance checklist (required)
1) All v1 required dashboards exist.
2) Each dashboard loads without missing metric errors.
3) Each panel uses only allowed labels and bounded variables.
4) Each dashboard contains at least one link to logs and one link to a relevant runbook.

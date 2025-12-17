# docs/specs/observability/metrics.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
Define platform metrics:
- scrape model and targets (Prometheus-style)
- required metrics per component (control plane, scheduler, projections, agent, edge)
- label conventions and cardinality rules
- retention and downsampling expectations (operator-facing)
- tenant-facing vs operator-facing separation

This spec is authoritative for metrics behavior and naming.

Related:
- observability architecture: `docs/architecture/09-observability-architecture.md`
- alerts: `docs/specs/observability/alerts.md`

## Scope
This spec defines metrics emitted by platform components.

This spec does not define:
- tracing
- log formats (see logging spec)
- dashboards content (see dashboards spec)

## v1 stance
1) Metrics are Prometheus-compatible: `/metrics` endpoint with text exposition format.
2) Metrics must be safe under multi-tenant operation: no unbounded label cardinality.
3) Metrics are primarily operator-facing in v1. Tenant-facing metrics is optional and limited.
4) Every critical control loop must have:
- latency metrics
- error counters
- backlog/lag gauges
- basic availability signals

## Metric naming conventions
- Prefix all platform metrics with `trc_`.
- Use `_total` suffix for counters.
- Use `_seconds` suffix for durations.
- Use `_bytes` suffix for byte sizes.
- Use `_count` or `_gauge` only when needed (prefer semantic names).

Examples:
- `trc_http_requests_total`
- `trc_event_append_seconds_bucket`
- `trc_projection_lag_events`
- `trc_agent_instances_running`

## Label conventions and cardinality rules (mandatory)
### Allowed bounded labels (v1)
- `component` (small set)
- `node_id` (bounded by cluster size)
- `org_id` (bounded by customer count, but still can be large; use sparingly)
- `env_id` (bounded by org quotas; use sparingly)
- `process_type` (bounded)
- `route_id` (bounded by org quotas; use sparingly)
- `status` (small enum)
- `reason` (small enum)
- `event_type` (bounded by event catalog; use sparingly)
- `http_method`, `http_path_template` (bounded templates, not raw paths)
- `http_status` (bounded)

### Forbidden labels
- raw hostname (unbounded)
- full URL / query string
- instance_id in high-rate metrics (avoid; instance count can be high and churny)
- user id / email
- error message strings
- idempotency keys
- any secret-like values

### Per-org metrics rule
Per-org metrics can explode.
- Prefer global aggregates.
- Only emit per-org metrics for a small, bounded set of “top offenders” via separate reporting, not as raw Prometheus labels.

v1 recommendation:
- do not emit per-org high-frequency metrics.
- keep per-org visibility in logs and event queries.

## Scrape model and targets (v1)
### Scrape protocol
- Prometheus pulls metrics from `/metrics` over HTTP.
- All targets must be reachable from the Prometheus collector network.

### Targets
1) Control plane API service
2) Projection workers (if separate)
3) Scheduler service (if separate)
4) Edge nodes
5) Node agents

If you run multiple replicas:
- scrape each instance and aggregate in Prometheus.

### Authentication
v1 recommendation:
- internal network scrape with network ACLs
- no per-request auth on /metrics, but it must not be publicly reachable
- or use mTLS if you prefer stronger guarantees (optional)

## Required metrics by component

## A) Control plane API
### Availability and request metrics
- `trc_http_requests_total{method,path_template,status}`
- `trc_http_request_duration_seconds_bucket{method,path_template,status}`

### Auth and token issuance
- `trc_auth_token_mints_total{grant_type,result}`
- `trc_auth_token_refresh_total{result}`
- `trc_auth_device_flow_polls_total{result}`

### DB and pool
- `trc_db_connections_in_use`
- `trc_db_connections_max`
- `trc_db_query_duration_seconds_bucket{query_class}` (bounded query class names)

### Idempotency
- `trc_idempotency_hits_total{endpoint}`
- `trc_idempotency_conflicts_total{endpoint}`

## B) Event append and command handling
- `trc_event_append_total{result}`
- `trc_event_append_duration_seconds_bucket{result}`
- `trc_events_table_rows` (gauge, approximate)
- `trc_events_table_size_bytes` (gauge, approximate)

If you classify commands:
- `trc_commands_total{command,result}`
- `trc_command_duration_seconds_bucket{command,result}`

## C) Projection workers
### Lag
- `trc_projection_last_applied_event_id{projection}`
- `trc_projection_lag_events{projection}`
- `trc_projection_apply_duration_seconds_bucket{projection}`

### Failures and restarts
- `trc_projection_errors_total{projection}`
- `trc_projection_rebuilds_total{projection}` (if you support rebuild action)

## D) Scheduler
### Loop health
- `trc_scheduler_reconcile_runs_total`
- `trc_scheduler_reconcile_duration_seconds_bucket`
- `trc_scheduler_group_reconciles_total{result}`
- `trc_scheduler_group_reconcile_duration_seconds_bucket{result}`

### Placement and actions
- `trc_scheduler_instance_allocations_total{result}`
- `trc_scheduler_instance_drains_total{reason}`
- `trc_scheduler_instance_stops_total{reason}`
- `trc_scheduler_unschedulable_total{reason}`

Reasons must be bounded enums, examples:
- `no_capacity_memory`
- `no_nodes_active`
- `volume_locality_no_node`
- `ipam_exhausted`
- `secrets_missing`
- `org_quota_exceeded`

### Rollout
- `trc_scheduler_rollout_seconds_bucket{result}`
- `trc_scheduler_rollout_failures_total{reason}`

## E) Node agent
### Heartbeat and connectivity
- `trc_agent_heartbeat_total{result}`
- `trc_agent_controlplane_reconnects_total`
- `trc_agent_overlay_peers` (gauge)
- `trc_agent_overlay_handshake_age_seconds_max` (gauge)

### Instance lifecycle
- `trc_agent_instances_desired` (gauge)
- `trc_agent_instances_running` (gauge)
- `trc_agent_instances_ready` (gauge)
- `trc_agent_instance_boot_seconds_bucket{result}`
- `trc_agent_instance_failures_total{reason}`

Reason is bounded and matches reason codes:
- `image_pull_failed`
- `rootfs_build_failed`
- `firecracker_start_failed`
- `network_setup_failed`
- `volume_attach_failed`
- `secrets_missing`
- `secrets_injection_failed`
- `healthcheck_failed`
- `oom_killed`
- `crash_loop_backoff`

### Image cache
- `trc_agent_image_pulls_total{result}`
- `trc_agent_image_pull_seconds_bucket{result}`
- `trc_agent_rootdisk_build_seconds_bucket{result}`
- `trc_agent_cache_bytes{kind}` where kind in {oci, rootdisks, tmp}
- `trc_agent_cache_evictions_total{kind}`

### Resource and disk pressure
- `trc_agent_volume_pool_free_bytes`
- `trc_agent_volume_pool_used_bytes`
- `trc_agent_disk_free_bytes{mount}` (bounded mounts only)
- `trc_agent_disk_pressure{kind}` (gauge 0/1)

### Secrets delivery
- `trc_agent_secrets_delivery_total{result,reason}`
- `trc_agent_secrets_delivery_seconds_bucket{result}`

### Snapshots and backups (if agent executes)
- `trc_agent_snapshots_total{result}`
- `trc_agent_snapshot_seconds_bucket{result}`
- `trc_agent_backups_total{result}`
- `trc_agent_backup_seconds_bucket{result}`
- `trc_agent_backup_bytes_total{result}`

## F) Edge ingress
### Listener and connections
- `trc_edge_connections_total{listener_port,result}`
- `trc_edge_concurrent_connections{listener_port}` (gauge)
- `trc_edge_upstream_connect_failures_total{reason}`

### Routing config
- `trc_edge_routes_loaded` (gauge)
- `trc_edge_config_apply_total{result}`
- `trc_edge_config_apply_seconds_bucket{result}`

### Backend health gating
- `trc_edge_backends_active{route_id}` is forbidden due to cardinality if route_id unbounded
Instead:
- `trc_edge_backends_active_total` (gauge)
- `trc_edge_backends_active_by_protocol{protocol_hint}` (gauge)

### SNI and TLS sniffing
- `trc_edge_sni_sniffs_total{result}` where result in {ok, timeout, not_tls, no_sni}
- `trc_edge_sni_sniff_seconds_bucket{result}`

### PROXY v2
- `trc_edge_proxy_v2_enabled_routes` (gauge)
- `trc_edge_proxy_v2_injections_total{result}`

## Tenant-facing metrics (optional, v1 minimal)
v1 recommendation:
- do not expose raw Prometheus metrics per tenant.
- provide tenant-facing “status” via API views:
  - instance states
  - deploy status
  - basic counts

If you later expose tenant metrics:
- create an explicit product surface that is bounded and audited.

## Retention and storage
Prometheus retention is operator-configured.

v1 recommended defaults:
- metrics retention: 14 days
- no long-term storage required in v1
- if long-term storage is needed, integrate a TSDB remote-write later

## Required dashboards (tie-in)
This spec defines what metrics exist; dashboards spec defines what we ship by default.

## Compliance tests (required)
1) Each component exposes /metrics and includes the required metric families.
2) Label cardinality checks:
- ensure forbidden labels are not present
- ensure path_template is used instead of raw paths
3) Scheduler metrics reflect unschedulable reasons deterministically.
4) Agent failure reasons match the bounded enum list.

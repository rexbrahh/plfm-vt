# docs/specs/state/materialized-views.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
This document defines:
- which materialized views (projection tables) exist in the control plane
- what each view represents and which events it consumes
- how views rebuild from the event log
- projection checkpointing and migration rules

Locked decision: state is event log + materialized views. See `docs/adr/0005-state-event-log-plus-materialized-views.md`.  
Storage is Postgres. See `docs/adr/0006-control-plane-db-postgres.md`.

## Scope
This spec defines projection outputs and rebuild mechanics.

This spec does not define:
- event log storage and ordering (see `docs/specs/state/event-log.md`)
- full event catalog (see `docs/specs/state/event-types.md`)
- public API schemas (see `docs/specs/api/openapi.yaml`)

## Projection principles
1) **Views are derived, not authoritative.**  
   The event log is the source of truth.

2) **Projections are replayable.**  
   Every view must be rebuildable by replaying events.

3) **Projections are idempotent and restart-safe.**  
   A projection can apply the same event again without corrupting state.

4) **Versioned evolution.**  
   View schema migrations are explicit. If a migration breaks replay, it is a bug.

## Projection checkpointing (required)
Each projection maintains a durable checkpoint of the last applied global event id.

### Table: `projection_checkpoints`
Required columns:
- `projection_name TEXT PRIMARY KEY`
- `last_applied_event_id BIGINT NOT NULL`
- `updated_at TIMESTAMPTZ NOT NULL`

Rules:
- Projection updates `last_applied_event_id` only after it has fully applied the event(s) durably.
- On startup, the projection reads the checkpoint and resumes from `last_applied_event_id + 1`.

### Exactly-once vs at-least-once
The system provides at-least-once delivery. Projections must be idempotent.

Recommended implementation strategy:
- For each event, run view updates and checkpoint update in one DB transaction:
  - apply changes
  - update checkpoint
This gives exactly-once effects per projection as long as you only commit after both are done.

## View inventory (v1)
Each view table has:
- `resource_version` (int) for optimistic concurrency checks in the API layer
- `created_at`, `updated_at` for operational clarity
- `deleted_at` or `is_deleted` for soft deletes where relevant

### 1) `orgs_view`
Represents:
- org metadata

Primary key:
- `org_id`

Consumes events:
- `org.created`
- `org.updated`

Columns (minimum):
- `org_id`
- `name`
- `created_at`
- `updated_at`

Notes:
- org deletion is not specified in v1. If added later, use soft delete.

---

### 2) `org_members_view`
Represents:
- org membership list and roles

Primary key:
- `member_id`

Unique constraints:
- `(org_id, email)` unique

Consumes events:
- `org_member.added`
- `org_member.role_updated`
- `org_member.removed`

Columns (minimum):
- `member_id`
- `org_id`
- `email`
- `role`
- `created_at`
- `updated_at`
- `is_deleted` (bool)

---

### 3) `service_principals_view`
Represents:
- service principal metadata and allowed scopes

Primary key:
- `service_principal_id`

Consumes events:
- `service_principal.created`
- `service_principal.scopes_updated`
- `service_principal.secret_rotated`
- `service_principal.deleted`

Columns (minimum):
- `service_principal_id`
- `org_id`
- `name`
- `scopes` (array or jsonb)
- `created_at`
- `updated_at`
- `is_deleted` (bool)

Note:
- client secret hashes live outside this view in auth tables.

---

### 4) `apps_view`
Represents:
- apps in an org

Primary key:
- `app_id`

Unique constraints:
- `(org_id, name)` unique for non-deleted apps

Consumes events:
- `app.created`
- `app.updated`
- `app.deleted`

Columns:
- `app_id`
- `org_id`
- `name`
- `description`
- `created_at`
- `updated_at`
- `is_deleted`

---

### 5) `envs_view`
Represents:
- environments for apps

Primary key:
- `env_id`

Unique constraints:
- `(app_id, name)` unique for non-deleted envs

Consumes events:
- `env.created`
- `env.updated`
- `env.deleted`

Columns:
- `env_id`
- `org_id`
- `app_id`
- `name`
- `created_at`
- `updated_at`
- `is_deleted`

---

### 6) `releases_view`
Represents:
- immutable releases per app

Primary key:
- `release_id`

Consumes events:
- `release.created`

Columns:
- `release_id`
- `org_id`
- `app_id`
- `image_ref`
- `index_or_manifest_digest`
- `resolved_digests` (jsonb)
- `manifest_schema_version`
- `manifest_hash`
- `created_at`

Notes:
- releases are immutable; no update events.

---

### 7) `deploys_view`
Represents:
- deploy records and status

Primary key:
- `deploy_id`

Consumes events:
- `deploy.created`
- `deploy.status_changed`

Columns:
- `deploy_id`
- `org_id`
- `app_id`
- `env_id`
- `kind` (deploy or rollback)
- `release_id`
- `process_types` (jsonb or array)
- `status`
- `message`
- `failed_reason`
- `created_at`
- `updated_at`

---

### 8) `env_desired_releases_view`
Represents:
- desired release per `(env_id, process_type)`

Primary key:
- `(env_id, process_type)`

Consumes events:
- `env.desired_release_set`

Columns:
- `env_id`
- `org_id`
- `app_id`
- `process_type`
- `release_id`
- `deploy_id` (correlation)
- `updated_at`

This view is the primary input to scheduler reconciliation for rollouts.

---

### 9) `env_scale_view`
Represents:
- desired replica counts per `(env_id, process_type)`

Primary key:
- `(env_id, process_type)`

Consumes events:
- `env.scale_set`

Columns:
- `env_id`
- `org_id`
- `app_id`
- `process_type`
- `desired_replicas`
- `updated_at`

Rules:
- If a process type has no scale entry, desired defaults to manifest-derived default (see manifest spec).
- The scheduler should treat missing entries as the default rather than requiring explicit rows, but for simplicity the projection can materialize defaults at deploy time.

---

### 10) `env_networking_view`
Represents:
- env-level networking state (IPv4 add-on)

Primary key:
- `env_id`

Consumes events:
- `env.ipv4_addon_enabled`
- `env.ipv4_addon_disabled`

Columns:
- `env_id`
- `org_id`
- `app_id`
- `ipv4_enabled` (bool)
- `ipv4_address` (nullable)
- `ipv4_allocation_id` (nullable)
- `updated_at`

---

### 11) `routes_view`
Represents:
- current routes (hostname bindings and backend targets)

Primary key:
- `route_id`

Unique constraints:
- hostname uniqueness by policy:
  - v1 recommendation: `UNIQUE (hostname)` for non-deleted routes

Consumes events:
- `route.created`
- `route.updated`
- `route.deleted`

Columns:
- `route_id`
- `org_id`
- `app_id`
- `env_id`
- `hostname`
- `listen_port`
- `protocol_hint`
- `backend_process_type`
- `backend_port`
- `proxy_protocol`
- `ipv4_required`
- `created_at`
- `updated_at`
- `is_deleted`

---

### 12) `secret_bundles_view`
Represents:
- env-scoped secret bundle metadata and current version

Primary key:
- `bundle_id`

Unique constraints:
- one bundle per env in v1:
  - `UNIQUE (env_id)` for non-deleted bundles

Consumes events:
- `secret_bundle.created`
- `secret_bundle.version_set`

Columns:
- `bundle_id`
- `org_id`
- `app_id`
- `env_id`
- `format` (platform_env_v1)
- `current_version_id` (nullable until first version)
- `current_data_hash` (nullable)
- `created_at`
- `updated_at`

Important:
- this view does not store secret material.

---

### 13) `volumes_view`
Represents:
- volume metadata

Primary key:
- `volume_id`

Consumes events:
- `volume.created`
- `volume.deleted`

Columns:
- `volume_id`
- `org_id`
- `name` (nullable)
- `size_bytes`
- `filesystem`
- `backup_enabled`
- `created_at`
- `updated_at`
- `is_deleted`

---

### 14) `volume_attachments_view`
Represents:
- volume attachments to env/process type

Primary key:
- `attachment_id`

Unique constraints:
- `(env_id, process_type, mount_path)` unique for non-deleted attachments

Consumes events:
- `volume_attachment.created`
- `volume_attachment.deleted`

Columns:
- `attachment_id`
- `org_id`
- `volume_id`
- `app_id`
- `env_id`
- `process_type`
- `mount_path`
- `read_only`
- `created_at`
- `updated_at`
- `is_deleted`

---

### 15) `snapshots_view`
Represents:
- snapshot requests and status

Primary key:
- `snapshot_id`

Consumes events:
- `snapshot.created`
- `snapshot.status_changed`

Columns:
- `snapshot_id`
- `org_id`
- `volume_id`
- `status`
- `size_bytes`
- `note`
- `created_at`
- `updated_at`
- `failed_reason` (nullable)

---

### 16) `restore_jobs_view`
Represents:
- restore operations and status

Primary key:
- `restore_id`

Consumes events:
- `restore_job.created`
- `restore_job.status_changed`

Columns:
- `restore_id`
- `org_id`
- `snapshot_id`
- `source_volume_id`
- `status`
- `new_volume_id` (nullable until succeeded)
- `created_at`
- `updated_at`
- `failed_reason` (nullable)

---

### 17) `instances_desired_view`
Represents:
- desired instances and assignments (scheduler output)

Primary key:
- `instance_id`

Consumes events:
- `instance.allocated`
- `instance.desired_state_changed`

Columns:
- `instance_id`
- `org_id`
- `app_id`
- `env_id`
- `process_type`
- `node_id`
- `desired_state` (running, draining, stopped)
- `release_id`
- `secrets_version_id` (nullable)
- `overlay_ipv6`
- `resources_snapshot` (jsonb)
- `spec_hash`
- `generation` (int, incremented when spec changes)
- `created_at`
- `updated_at`

Notes:
- This view is the input to building node-scoped plans.

---

### 18) `instances_status_view`
Represents:
- latest reported runtime status per instance

Primary key:
- `instance_id`

Consumes events:
- `instance.status_changed`

Columns:
- `instance_id`
- `org_id`
- `env_id`
- `node_id`
- `status`
- `boot_id`
- `microvm_id`
- `exit_code`
- `reason_code`
- `reason_detail`
- `reported_at`
- `updated_at`

Rules:
- View stores the most recent status by `event_id` (global order).
- It does not attempt to reconstruct full boot attempt history. That can be a separate audit query.

---

### 19) `exec_sessions_view`
Represents:
- exec session metadata for auditing

Primary key:
- `exec_session_id`

Consumes events:
- `exec_session.granted`
- `exec_session.connected`
- `exec_session.ended`

Columns:
- `exec_session_id`
- `org_id`
- `env_id`
- `instance_id`
- `requested_command` (jsonb)
- `tty`
- `status` (granted, connected, ended)
- `expires_at`
- `connected_at`
- `ended_at`
- `exit_code`
- `end_reason`
- `created_at`
- `updated_at`

---

### 20) `nodes_view` (infrastructure)
Represents:
- node metadata and state

Primary key:
- `node_id`

Consumes events:
- `node.enrolled`
- `node.state_changed`
- `node.capacity_updated`

Columns:
- `node_id`
- `state` (active, draining, disabled, degraded, offline)
- `wireguard_public_key`
- `agent_mtls_subject`
- `public_ipv6` (nullable)
- `public_ipv4` (nullable)
- `labels` (jsonb)
- `allocatable` (jsonb)
- `mtu` (nullable)
- `created_at`
- `updated_at`

This view is not tenant-readable by default.

## Derived views (optional but recommended)
These are “helper” views that simplify API and scheduling queries.

### A) `env_status_view`
Represents:
- computed env health summary

Derived from:
- env desired releases
- instances desired and status
- routes and backend readiness

Fields:
- `env_id`
- `status` (healthy, degraded, failing)
- `ready_instances_by_process` (jsonb)
- `last_deploy_id`

This is derived and can be rebuilt anytime.

### B) `route_backends_view`
Represents:
- the set of ready backends per route

Derived from:
- routes_view
- instances_status_view (ready)
- instances_desired_view (node_id and overlay_ipv6)
- process ports (from release manifest, via release metadata if stored)

Fields:
- `route_id`
- `backends` (jsonb array of {overlay_ipv6, port})
- `updated_at`

Edge can consume this derived view directly or via event-driven updates.

## Rebuild rules
### Full rebuild
To rebuild all views:
1) stop writers or run in maintenance mode (optional but recommended)
2) truncate all view tables
3) set all projection checkpoints to 0
4) replay the full event log in ascending `event_id`
5) verify invariants and run end-to-end demo checks

### Partial rebuild
If only one projection is broken:
- truncate only its view tables
- reset only its checkpoint
- replay from 0 for that projection only

Important:
- If projections depend on each other, define a replay order or remove cross-projection dependencies.
- v1 recommendation: projections should be independent and only depend on the event log.

## Migration rules
### Additive schema changes
Allowed:
- add nullable columns
- add new view tables
- add indexes

Projection code can start populating new columns; old data can remain null until replay or backfill.

### Non-additive schema changes
Examples:
- changing column type
- making a column non-null
- changing primary keys

Rules:
- require a migration plan that keeps projections correct during rolling deploys
- may require:
  - dual-write to old and new columns
  - replay into a new table and swap
  - or a controlled downtime window

### Event schema evolution
Projection code must handle multiple event versions.
- v1 requirement: handle current and previous versions during rolling upgrades.

## Consistency guarantees
- Views are eventually consistent with the event log.
- Some API endpoints require read-your-writes:
  - implemented by waiting for projection checkpoints to reach the appended event_id

The list of endpoints that require read-your-writes is in `docs/specs/api/http-api.md`.

## Performance considerations
- Keep view updates O(1) per event where possible.
- Use UPSERT semantics for “latest state” views.
- Avoid heavy joins in projection code; push query complexity into read paths or into derived helper views.

## Open questions
- Whether to store a canonical parsed representation of the manifest (per release) to make some derived views easier. v1 recommendation: store manifest_hash and keep parsed content in release metadata storage, but avoid duplicating full manifest content in many views.

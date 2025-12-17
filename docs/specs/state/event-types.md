# docs/specs/state/event-types.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
This document is the authoritative catalog of control plane event types.

It defines, for each event type:
- event name and version
- aggregate type and aggregate id
- when it is emitted
- payload schema (v1)
- invariants and validations
- primary consumers (projections, scheduler, agents, edge)

This document must be consistent with:
- `docs/specs/state/event-log.md` (storage and ordering contract)
- `docs/specs/state/materialized-views.md` (what state is derived and how)
- `docs/specs/api/openapi.yaml` (public resource model and operations)
- ADRs in `docs/adr/*` and `docs/DECISIONS_LOCKED.md`

## Conventions

### Naming
- `event_type` uses lower-case dot notation: `env.created`, `route.updated`.
- `event_version` is an integer starting at 1 for each `event_type`.

### Aggregate rules
- Every event belongs to exactly one aggregate.
- Aggregates are identified by `(aggregate_type, aggregate_id)`.

### Payload rules
- Payload is JSON object.
- Payload MUST NOT include raw secret material.
- Payload SHOULD include stable ids and minimal derived data needed for debugging and reconstruction.
- Large derived snapshots (like full node plans) are not stored as events in v1. They are derived from state.

### Actor rules
Events record:
- actor_type: `user` | `service_principal` | `system`
- actor_id
- org_id where tenant-scoped

### Correlation fields
All events emitted as part of one user action SHOULD share a `request_id`.
Where useful, set:
- `correlation_id` to group related events (example: deploy_id for all events in one deploy command).

## Aggregate taxonomy (v1)

Tenant-scoped aggregates:
- `org` (aggregate_id = org_id)
- `org_member` (aggregate_id = member_id)
- `service_principal` (aggregate_id = service_principal_id)
- `app` (aggregate_id = app_id)
- `env` (aggregate_id = env_id)
- `release` (aggregate_id = release_id)
- `deploy` (aggregate_id = deploy_id)
- `route` (aggregate_id = route_id)
- `secret_bundle` (aggregate_id = bundle_id)
- `volume` (aggregate_id = volume_id)
- `volume_attachment` (aggregate_id = attachment_id)
- `snapshot` (aggregate_id = snapshot_id)
- `restore_job` (aggregate_id = restore_id)
- `instance` (aggregate_id = instance_id)
- `exec_session` (aggregate_id = exec_session_id)

Infrastructure aggregates (operator-scoped, not tenant-facing by default):
- `node` (aggregate_id = node_id)

## Event catalog

---

## Org and membership

### org.created (v1)
Aggregate:
- type: `org`
- id: `org_id`

Emitted when:
- a new org is created (operator action or product onboarding flow).

Payload:
- `org_id` (string)
- `name` (string)

Invariants:
- org_id is unique.
- org name may be unique by policy (optional).

Consumers:
- org projection (org list, org details)

---

### org.updated (v1)
Aggregate:
- type: `org`
- id: `org_id`

Emitted when:
- org metadata changes (name).

Payload:
- `org_id`
- `name` (optional)
- `billing_email` (optional, if present)

Invariants:
- only org admins can update.

Consumers:
- org projection

---

### org_member.added (v1)
Aggregate:
- type: `org_member`
- id: `member_id`

Emitted when:
- a member is added to an org.

Payload:
- `member_id`
- `org_id`
- `email` (string)
- `role` (enum: `owner`, `admin`, `developer`, `readonly`)

Invariants:
- email must be valid format.
- role must be valid.
- membership for `(org_id, email)` must be unique.

Consumers:
- membership projection
- authz cache (if any)

---

### org_member.role_updated (v1)
Aggregate:
- type: `org_member`
- id: `member_id`

Emitted when:
- an org member role changes.

Payload:
- `member_id`
- `org_id`
- `old_role`
- `new_role`

Invariants:
- only org admins can change roles.
- at least one owner must exist (platform policy).

Consumers:
- membership projection
- authz cache (if any)

---

### org_member.removed (v1)
Aggregate:
- type: `org_member`
- id: `member_id`

Emitted when:
- a member is removed.

Payload:
- `member_id`
- `org_id`
- `email`

Invariants:
- only org admins can remove members.
- at least one owner must remain (platform policy).

Consumers:
- membership projection
- authz cache (if any)

---

## Service principals (automation)

### service_principal.created (v1)
Aggregate:
- type: `service_principal`
- id: `service_principal_id`

Emitted when:
- a service principal is created in an org.

Payload:
- `service_principal_id`
- `org_id`
- `name`
- `scopes` (array of strings)

Invariants:
- only org admins can create.
- scopes must be subset of platform allowed scopes.

Consumers:
- auth projection
- audit tooling

---

### service_principal.scopes_updated (v1)
Aggregate:
- type: `service_principal`
- id: `service_principal_id`

Emitted when:
- allowed scopes for a service principal are updated.

Payload:
- `service_principal_id`
- `org_id`
- `scopes` (array)

Invariants:
- only org admins can update.
- scopes must remain subset of platform allowed scopes.

Consumers:
- auth projection

---

### service_principal.secret_rotated (v1)
Aggregate:
- type: `service_principal`
- id: `service_principal_id`

Emitted when:
- client secret is rotated.

Payload:
- `service_principal_id`
- `org_id`
- `rotation_id` (string)
- `rotated_at` (timestamp string)

Invariants:
- raw secret material is never in payload.
- old secret must be revoked.

Consumers:
- auth projection
- audit tooling

---

### service_principal.deleted (v1)
Aggregate:
- type: `service_principal`
- id: `service_principal_id`

Emitted when:
- a service principal is deleted.

Payload:
- `service_principal_id`
- `org_id`

Invariants:
- deletion revokes credentials.

Consumers:
- auth projection

---

## Apps and environments

### app.created (v1)
Aggregate:
- type: `app`
- id: `app_id`

Emitted when:
- an app is created.

Payload:
- `app_id`
- `org_id`
- `name`
- `description` (optional)

Invariants:
- app name unique per org.

Consumers:
- app projection

---

### app.updated (v1)
Aggregate:
- type: `app`
- id: `app_id`

Emitted when:
- app metadata changes.

Payload:
- `app_id`
- `org_id`
- `name` (optional)
- `description` (optional)

Invariants:
- if name changes, still unique per org.

Consumers:
- app projection

---

### app.deleted (v1)
Aggregate:
- type: `app`
- id: `app_id`

Emitted when:
- app is deleted (soft delete recommended).

Payload:
- `app_id`
- `org_id`

Invariants:
- deletion must handle dependent envs by policy (reject if envs exist, or cascade with explicit deletes).

Consumers:
- app projection

---

### env.created (v1)
Aggregate:
- type: `env`
- id: `env_id`

Emitted when:
- an environment is created.

Payload:
- `env_id`
- `org_id`
- `app_id`
- `name` (example: prod, staging)

Invariants:
- env name unique per app.

Consumers:
- env projection

---

### env.updated (v1)
Aggregate:
- type: `env`
- id: `env_id`

Emitted when:
- env metadata changes.

Payload:
- `env_id`
- `org_id`
- `app_id`
- `name` (optional)

Consumers:
- env projection

---

### env.deleted (v1)
Aggregate:
- type: `env`
- id: `env_id`

Emitted when:
- env is deleted (soft delete recommended).

Payload:
- `env_id`
- `org_id`
- `app_id`

Invariants:
- must detach routes and volumes by policy (either block deletion until cleaned, or cascade with explicit events).

Consumers:
- env projection
- route projection (cleanup)
- scheduler (stop instances)

---

## Releases and deploys

### release.created (v1)
Aggregate:
- type: `release`
- id: `release_id`

Emitted when:
- a new release is created (image digest pinned + manifest).

Payload:
- `release_id`
- `org_id`
- `app_id`
- `image_ref` (string, may include tag for audit)
- `index_or_manifest_digest` (string, sha256)
- `resolved_digests` (array of objects, optional)
  - `os` (string, v1 must be linux)
  - `arch` (string)
  - `digest` (string, sha256)
- `manifest_schema_version` (string, v1 `v1`)
- `manifest_hash` (string)
- `manifest_size_bytes` (int, optional)

Invariants:
- release is immutable.
- image digest is sha256 and pinned at creation time.
- manifest_hash is content hash of canonical TOML (or canonicalized representation).

Consumers:
- release projection
- deploy validation
- workload spec builder (derived)

---

### deploy.created (v1)
Aggregate:
- type: `deploy`
- id: `deploy_id`

Emitted when:
- a deploy is requested (including rollbacks).

Payload:
- `deploy_id`
- `org_id`
- `app_id`
- `env_id`
- `kind` (enum: `deploy`, `rollback`)
- `release_id` (string)
- `process_types` (array of strings, optional, default all)
- `strategy` (enum: `rolling`)
- `initiated_at` (timestamp string)

Invariants:
- release_id must belong to app_id.
- env_id must belong to app_id.
- process_types must exist in the release manifest (if provided).

Consumers:
- deploy projection
- scheduler (trigger reconciliation)

---

### env.desired_release_set (v1)
Aggregate:
- type: `env`
- id: `env_id`

Emitted when:
- desired release for one process type is set as part of a deploy or rollback.

Payload:
- `env_id`
- `org_id`
- `app_id`
- `process_type` (string)
- `release_id` (string)
- `deploy_id` (string, correlation id)

Invariants:
- process_type must exist in the release manifest.
- release_id must belong to app_id.

Consumers:
- env desired state projection
- scheduler (compute desired instances)

---

### deploy.status_changed (v1)
Aggregate:
- type: `deploy`
- id: `deploy_id`

Emitted when:
- deploy progresses or completes.

Payload:
- `deploy_id`
- `org_id`
- `env_id`
- `status` (enum: `queued`, `rolling`, `succeeded`, `failed`)
- `message` (optional string)
- `failed_reason` (optional string)
- `updated_at` (timestamp string)

Invariants:
- status transitions must be monotonic by policy:
  - queued -> rolling -> succeeded|failed
- a failed deploy does not automatically change desired release unless a separate rollback is initiated.

Consumers:
- deploy projection
- user UX (CLI)

---

## Env configuration (scale and IPv4)

### env.scale_set (v1)
Aggregate:
- type: `env`
- id: `env_id`

Emitted when:
- desired replica counts per process type are updated.

Payload:
- `env_id`
- `org_id`
- `app_id`
- `scales` (array of objects)
  - `process_type` (string)
  - `desired` (int, >= 0)

Invariants:
- process_type must exist in currently desired release manifest for the env, or the platform must define behavior for unknown process types (v1 recommendation: reject unknown).
- desired must be bounded by org quotas.

Consumers:
- env scale projection
- scheduler

---

### env.ipv4_addon_enabled (v1)
Aggregate:
- type: `env`
- id: `env_id`

Emitted when:
- dedicated IPv4 add-on is enabled for an environment.

Payload:
- `env_id`
- `org_id`
- `app_id`
- `allocation_id` (string)
- `ipv4_address` (string)
- `enabled_at` (timestamp string)

Invariants:
- allocation_id and ipv4_address are unique and managed by platform.
- env must not already have an active IPv4 allocation.

Consumers:
- env networking projection
- route validation (ipv4_required)
- billing (future)

---

### env.ipv4_addon_disabled (v1)
Aggregate:
- type: `env`
- id: `env_id`

Emitted when:
- dedicated IPv4 add-on is disabled and released.

Payload:
- `env_id`
- `org_id`
- `allocation_id`
- `ipv4_address`
- `disabled_at`

Invariants:
- routes that require ipv4 must be removed or made unreachable by policy (v1 recommendation: reject disable if active ipv4_required routes exist, unless forced with explicit operator override).

Consumers:
- env networking projection
- edge (stop binding ipv4 listeners)

---

## Routes and ingress

### route.created (v1)
Aggregate:
- type: `route`
- id: `route_id`

Emitted when:
- a route is created.

Payload:
- `route_id`
- `org_id`
- `app_id`
- `env_id`
- `hostname` (string)
- `listen_port` (int)
- `protocol_hint` (enum: `tls_passthrough`, `tcp_raw`)
- `backend_process_type` (string)
- `backend_port` (int)
- `proxy_protocol` (enum: `off`, `v2`)
- `backend_expects_proxy_protocol` (bool, required when proxy_protocol is v2)
- `ipv4_required` (bool)

Invariants:
- hostname uniqueness scope must be enforced (v1 recommendation: globally unique across platform).
- backend_process_type must exist in env desired release manifest.
- backend_port must be declared in that process type port declarations.
- if proxy_protocol is v2, backend_expects_proxy_protocol must be true, otherwise reject.
- if ipv4_required is true, env must have ipv4_addon_enabled.

Consumers:
- route projection
- edge config builder

---

### route.updated (v1)
Aggregate:
- type: `route`
- id: `route_id`

Emitted when:
- route fields change.

Payload:
- `route_id`
- `org_id`
- `env_id`
- any of:
  - `backend_process_type`
  - `backend_port`
  - `proxy_protocol`
  - `backend_expects_proxy_protocol`
  - `ipv4_required`

Invariants:
- same validation rules as creation apply for any updated field.

Consumers:
- route projection
- edge config builder

---

### route.deleted (v1)
Aggregate:
- type: `route`
- id: `route_id`

Emitted when:
- route is deleted.

Payload:
- `route_id`
- `org_id`
- `env_id`
- `hostname`

Invariants:
- deletion releases hostname binding.

Consumers:
- route projection
- edge config builder

---

## Secrets

### secret_bundle.created (v1)
Aggregate:
- type: `secret_bundle`
- id: `bundle_id`

Emitted when:
- an environment gets secrets configured for the first time.

Payload:
- `bundle_id`
- `org_id`
- `app_id`
- `env_id`
- `format` (enum: `platform_env_v1`)
- `created_at`

Invariants:
- bundle is env-scoped. One bundle per env in v1.

Consumers:
- secrets projection

---

### secret_bundle.version_set (v1)
Aggregate:
- type: `secret_bundle`
- id: `bundle_id`

Emitted when:
- secrets are updated, creating a new version.

Payload:
- `bundle_id`
- `org_id`
- `env_id`
- `version_id` (string)
- `format` (enum: `platform_env_v1`)
- `data_hash` (string, hash of canonical secrets file content)
- `updated_at`

Invariants:
- raw secret material must not be in payload.
- encrypted secret material is stored outside the event log and referenced by version_id.
- version_id is immutable.

Consumers:
- secrets projection
- scheduler trigger (rotation triggers rollout restart by creating new instances with new version)

---

## Volumes, attachments, snapshots, restore

### volume.created (v1)
Aggregate:
- type: `volume`
- id: `volume_id`

Emitted when:
- a volume is created.

Payload:
- `volume_id`
- `org_id`
- `name` (optional)
- `size_bytes` (int)
- `filesystem` (enum: `ext4`)
- `backup_enabled` (bool)

Invariants:
- size_bytes >= 1Gi.
- filesystem must be supported.

Consumers:
- volume projection
- scheduler (locality constraints depend on placement state, stored elsewhere)

---

### volume.deleted (v1)
Aggregate:
- type: `volume`
- id: `volume_id`

Emitted when:
- a volume is deleted.

Payload:
- `volume_id`
- `org_id`

Invariants:
- volume must not be attached unless forced by operator policy.

Consumers:
- volume projection

---

### volume_attachment.created (v1)
Aggregate:
- type: `volume_attachment`
- id: `attachment_id`

Emitted when:
- a volume attachment is created for an env and process type.

Payload:
- `attachment_id`
- `org_id`
- `volume_id`
- `app_id`
- `env_id`
- `process_type`
- `mount_path`
- `read_only` (bool)

Invariants:
- mount_path must be absolute and not under reserved system paths.
- a given `(env_id, process_type, mount_path)` must be unique.
- volume must exist and be owned by org.
- attachment implies locality constraint for scheduling.

Consumers:
- volume attachment projection
- workload spec builder (derived mounts)

---

### volume_attachment.deleted (v1)
Aggregate:
- type: `volume_attachment`
- id: `attachment_id`

Emitted when:
- an attachment is removed.

Payload:
- `attachment_id`
- `org_id`
- `volume_id`
- `env_id`
- `process_type`

Invariants:
- if attachment is in use, platform policy defines whether to drain first or reject.

Consumers:
- volume attachment projection
- scheduler (may need to drain instances that required the mount)

---

### snapshot.created (v1)
Aggregate:
- type: `snapshot`
- id: `snapshot_id`

Emitted when:
- a snapshot is requested.

Payload:
- `snapshot_id`
- `org_id`
- `volume_id`
- `status` (enum: `queued`)
- `note` (optional)

Invariants:
- volume must exist and be owned by org.

Consumers:
- snapshot projection
- backup pipeline (agent or control plane worker)

---

### snapshot.status_changed (v1)
Aggregate:
- type: `snapshot`
- id: `snapshot_id`

Emitted when:
- snapshot progresses.

Payload:
- `snapshot_id`
- `org_id`
- `volume_id`
- `status` (enum: `running`, `succeeded`, `failed`)
- `size_bytes` (optional)
- `failed_reason` (optional string)

Invariants:
- status transitions are monotonic: queued -> running -> succeeded|failed.

Consumers:
- snapshot projection
- user UX

---

### restore_job.created (v1)
Aggregate:
- type: `restore_job`
- id: `restore_id`

Emitted when:
- restore is requested.

Payload:
- `restore_id`
- `org_id`
- `snapshot_id`
- `source_volume_id`
- `new_volume_name` (optional)
- `status` (enum: `queued`)

Consumers:
- restore projection
- storage worker

---

### restore_job.status_changed (v1)
Aggregate:
- type: `restore_job`
- id: `restore_id`

Emitted when:
- restore progresses or completes.

Payload:
- `restore_id`
- `org_id`
- `status` (enum: `running`, `succeeded`, `failed`)
- `new_volume_id` (required when succeeded)
- `failed_reason` (optional)

Invariants:
- on succeeded, a `volume.created` event for new_volume_id must exist (same request_id or causation linkage).

Consumers:
- restore projection
- volume projection

---

## Scheduling and runtime instances

### instance.allocated (v1)
Aggregate:
- type: `instance`
- id: `instance_id`

Emitted when:
- scheduler allocates a new instance to a node.

Payload:
- `instance_id`
- `org_id`
- `app_id`
- `env_id`
- `process_type`
- `node_id`
- `desired_state` (enum: `running`)
- `release_id`
- `secrets_version_id` (optional, but required if env has secrets)
- `overlay_ipv6` (string, /128)
- `resources_snapshot` (object)
  - `cpu_request` (float)
  - `memory_limit_bytes` (int)
  - `ephemeral_disk_bytes` (int)
- `spec_hash` (string, hash of resolved WorkloadSpec inputs)

Invariants:
- node_id must reference an active node.
- overlay_ipv6 must be unique across instances.
- memory_limit_bytes must respect node allocatable budgets.
- if secrets are required for process type, secrets_version_id must be present.

Consumers:
- instance desired state projection
- scheduler progress tracking
- host agent (node-scoped filter by node_id)

---

### instance.desired_state_changed (v1)
Aggregate:
- type: `instance`
- id: `instance_id`

Emitted when:
- scheduler changes desired state to draining or stopped.

Payload:
- `instance_id`
- `org_id`
- `env_id`
- `desired_state` (enum: `draining`, `stopped`)
- `drain_grace_seconds` (optional, default 10)
- `reason` (string, optional)

Invariants:
- running -> draining -> stopped is the intended progression.
- direct running -> stopped is allowed for emergency stop.

Consumers:
- instance desired state projection
- host agent

---

### instance.status_changed (v1)
Aggregate:
- type: `instance`
- id: `instance_id`

Emitted when:
- host agent reports a lifecycle transition.

Payload:
- `instance_id`
- `org_id`
- `env_id`
- `node_id`
- `status` (enum: `booting`, `ready`, `draining`, `stopped`, `failed`)
- `boot_id` (string, optional)
- `microvm_id` (string, optional)
- `exit_code` (int, optional)
- `reason_code` (string, optional)
- `reason_detail` (string, optional)
- `reported_at` (timestamp string)

Reason codes (v1 allowed set, must match `docs/specs/workload-spec.md`):
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
- `terminated_by_operator`
- `node_draining`

Invariants:
- status transitions should be monotonic per boot attempt, but multiple boot attempts may occur under same instance_id.
- if status is failed, reason_code must be present.

Consumers:
- instance status projection
- edge backend selection (ready gating)
- deploy progress (derived)

---

## Exec sessions

### exec_session.granted (v1)
Aggregate:
- type: `exec_session`
- id: `exec_session_id`

Emitted when:
- an exec grant is issued.

Payload:
- `exec_session_id`
- `org_id`
- `app_id`
- `env_id`
- `instance_id`
- `requested_command` (array of strings)
- `tty` (bool)
- `expires_at` (timestamp string)

Invariants:
- exec requires explicit permission scope.
- command must be validated (bounded length, safe encoding).
- this event must not include secrets.

Consumers:
- exec projection
- audit tooling
- host agent (to accept a grant)

---

### exec_session.connected (v1)
Aggregate:
- type: `exec_session`
- id: `exec_session_id`

Emitted when:
- client connects and session begins.

Payload:
- `exec_session_id`
- `org_id`
- `instance_id`
- `connected_at`

Consumers:
- exec projection
- audit tooling

---

### exec_session.ended (v1)
Aggregate:
- type: `exec_session`
- id: `exec_session_id`

Emitted when:
- session ends.

Payload:
- `exec_session_id`
- `org_id`
- `instance_id`
- `ended_at`
- `exit_code` (optional)
- `end_reason` (string, optional)

Consumers:
- exec projection
- audit tooling

---

## Nodes (infrastructure)

### node.enrolled (v1)
Aggregate:
- type: `node`
- id: `node_id`

Emitted when:
- a node is enrolled into the cluster.

Payload:
- `node_id`
- `cluster_id` (optional)
- `public_ipv6` (optional)
- `public_ipv4` (optional)
- `wireguard_public_key`
- `agent_mtls_subject` (string)
- `labels` (object, optional)
- `enrolled_at`

Invariants:
- node identity is unique.
- keys must be valid and stored securely.

Consumers:
- node projection
- overlay membership distribution

---

### node.state_changed (v1)
Aggregate:
- type: `node`
- id: `node_id`

Emitted when:
- node state changes (operator action or system detection).

Payload:
- `node_id`
- `state` (enum: `active`, `draining`, `disabled`, `degraded`, `offline`)
- `reason` (optional)
- `changed_at`

Invariants:
- disabled nodes are not schedulable.
- draining nodes accept no new instances.

Consumers:
- scheduler
- ops tooling

---

### node.capacity_updated (v1)
Aggregate:
- type: `node`
- id: `node_id`

Emitted when:
- allocatable capacity or limits change (rare event, not heartbeat).

Payload:
- `node_id`
- `allocatable` (object)
  - `cpu_cores` (float)
  - `memory_bytes` (int)
  - `disk_bytes` (int, optional)
- `reserved` (object, optional)
  - `memory_bytes`
- `mtu` (int, optional)
- `updated_at`

Consumers:
- scheduler
- ops tooling

---

## Cross-cutting notes

### Event emission rules for multi-step commands
Some API calls result in multiple events. Example: deploy.
- The command handler MUST append all events in one transaction when possible.
- Events SHOULD share `request_id`.
- Events SHOULD set `correlation_id` to deploy_id.

### Which events are tenant-readable
Tenant-readable (org-scoped) events include all tenant aggregates:
- org, org_member, service_principal, app, env, release, deploy, route, secret_bundle, volume, volume_attachment, snapshot, restore_job, instance, exec_session.

Tenant-readable events do not include infrastructure node internals by default unless explicitly exposed.

### What is intentionally not an event
- periodic node heartbeats
- high-rate runtime metrics
- raw logs

Those belong in telemetry and observability systems, not in the control plane event log.

## Open questions (intentionally deferred)
- Some domain expansions may require new aggregates:
  - shared IPv4 tier
  - L7 ingress mode
  - multi-region routing
- These must be introduced via new event types and potentially new ADRs.

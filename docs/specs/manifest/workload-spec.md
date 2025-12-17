
```md
# docs/specs/workload-spec.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
This document defines the scheduler-to-host-agent contract.

The host agent consumes desired instance assignments and converges the node’s actual state to match. This contract must be stable and versioned because it is the coupling point between control plane scheduling and data plane execution.

This spec is authoritative for:
- identity and invariants for desired instances
- the fields an agent needs to boot and supervise microVMs
- update and rollout rules
- compatibility expectations across versions

## Scope
This spec defines:
- the data model for desired instances (what the agent must run)
- invariants and lifecycle semantics

This spec does not define:
- control plane API endpoints (see `docs/specs/api/*`)
- event types, though it must map cleanly to them (see `docs/specs/state/event-types.md`)
- full Firecracker configuration, though it references required runtime parameters (see `docs/specs/runtime/*`)

## Versioning and compatibility
### WorkloadSpec version
Every record includes:
- `spec_version` (string)
  - v1 value: `"v1"`

Compatibility rules (v1):
- Additive changes are allowed:
  - new optional fields may be added with defaults
- Breaking changes require:
  - new `spec_version`
  - control plane and agent coordination

Agent behavior rules:
- Agents must ignore unknown fields if possible.
- Agents must treat missing optional fields as defaults defined in this document.
- If an agent cannot honor a field that is required for correctness, it must fail the instance with a clear reason code.

## Identity model
There are three related identities.

### 1) Desired Instance
A desired instance is a slot that should be running on a node.

Fields:
- `instance_id` (UUID or ULID, required)
- `org_id` (required)
- `app_id` (required)
- `env_id` (required)
- `process_type` (string, required)

Properties:
- `instance_id` is stable across restarts of the same desired slot.
- A crash and restart does not change `instance_id`.

### 2) Boot attempt (optional)
A boot attempt is one concrete attempt to realize a desired instance.

Fields (reported by agent, not required in desired spec):
- `boot_id` (UUID)
- `started_at`
- `ended_at`
- `exit_reason`

Purpose:
- allows separating “desired slot” from “this VM attempt”.

### 3) Allocation / Assignment
An assignment binds a desired instance to a specific node.

Fields:
- `node_id` (required)
- `assignment_id` (required)
- `generation` (int, required)

Properties:
- `generation` increments when the desired spec for that instance changes.
- The agent uses generation to decide if it must replace or restart.

## High-level message model
The agent consumes a node-scoped plan, not global state.

### `NodePlan` (conceptual)
A plan contains the complete desired set for a node.

Fields:
- `spec_version = "v1"`
- `node_id`
- `plan_id` (opaque string or UUID)
- `created_at`
- `cursor_event_id` (the event id the plan reflects)
- `instances` (array of `DesiredInstanceAssignment`)

Plan semantics:
- The plan is a full snapshot of desired instances for the node.
- The agent must stop instances that exist locally but are not present in the latest plan, after drain rules.
- Plans are replaceable. The latest plan wins.

### `DesiredInstanceAssignment`
Fields:
- `assignment_id` (required)
- `node_id` (required)
- `instance_id` (required)
- `generation` (required)
- `desired_state` (required)
  - `"running"`
  - `"draining"`
  - `"stopped"`
- `drain_grace_seconds` (optional, default 10)
- `workload` (required when desired_state is running or draining)
  - `WorkloadSpec`

Semantics:
- `"running"`: instance must be present and healthy
- `"draining"`: instance should stop accepting new traffic, then terminate after grace period
- `"stopped"`: instance must not be running

## `WorkloadSpec` (v1)
WorkloadSpec is the resolved runtime configuration for a desired instance.

### Fields

#### Identity
- `spec_version = "v1"`
- `org_id`
- `app_id`
- `env_id`
- `process_type`
- `instance_id`
- `generation`

#### Release reference
- `release_id` (required)
- `image` (required)
  - `ref` (string, optional)
  - `digest` (string, required, sha256)
  - `index_digest` (string, optional, sha256, if multi-arch)
  - `resolved_digest` (string, required, sha256, the exact manifest used on this node arch)
  - `os` (string, required, v1 must be `"linux"`)
  - `arch` (string, required, example `"amd64"` or `"arm64"`)
- `manifest_hash` (string, required)
- `command` (array of strings, required)
  - fully resolved entrypoint
- `workdir` (string, optional)
- `env_vars` (map string -> string, optional)

Rules:
- Agents must pull by `resolved_digest`, never by tag.
- `command` must be an executable path available inside the guest after rootfs mount.

#### Resources
- `resources` (required)
  - `cpu_request` (float, required)
  - `memory_limit_bytes` (int, required)
  - `ephemeral_disk_bytes` (int, optional, default 4Gi)
  - `vcpu_count` (int, optional)
  - `cpu_weight` (int, optional)

Recommended mapping rules (v1):
- If `vcpu_count` is not provided:
  - `vcpu_count = max(1, ceil(cpu_request))`
- If `cpu_weight` is not provided:
  - set proportional to cpu_request in a stable way (agent-defined mapping)

Hard rules:
- memory limit must be enforced as a hard cap (cgroup v2) at the host boundary.
- cpu is soft and is enforced via weights or quotas, not strict reservation.

#### Networking
- `network` (required)
  - `overlay_ipv6` (string, required, /128)
  - `gateway_ipv6` (string, required)
  - `mtu` (int, optional, default 1420)
  - `dns` (array of IPv6 addresses, optional)
  - `ports` (array of `PortSpec`, optional)

`PortSpec`:
- `name` (string)
- `port` (int)
- `protocol` (string, v1 `"tcp"`)

Rules:
- `overlay_ipv6` is the identity used for east-west routing and for edge-to-backend routing.
- Port list must correspond to ports declared in manifest for that process type.
- Routes bind to these ports, but route objects are not embedded in WorkloadSpec in v1.

#### Health
- `health` (optional)
  - `type` (`"tcp"` or `"http"`)
  - `port` (int, required)
  - `path` (string, required for http)
  - `interval_seconds` (default 10)
  - `timeout_seconds` (default 2)
  - `grace_period_seconds` (default 10)
  - `success_threshold` (default 1)
  - `failure_threshold` (default 3)

Agent responsibilities:
- Run health checks locally (inside the node context) to determine readiness.
- Report readiness transitions to control plane.
- Apply grace periods to avoid flapping during boot.

#### Secrets
- `secrets` (optional)
  - `required` (bool, default false)
  - `secret_version_id` (string, optional)
  - `mount_path` (string, required if secrets present)
  - `mode` (int, optional, default 0400)
  - `uid` (int, optional, default 0)
  - `gid` (int, optional, default 0)

v1 fixed rule:
- `mount_path` must be `/run/secrets/platform.env`

Rotation semantics:
- A secret version change is represented as a new generation and is rolled out by creating new desired instances (or by incrementing generation for the slot, depending on rollout model). v1 recommendation is create new desired slots for rolling changes.

#### Volumes and mounts
- `mounts` (array, optional)

`MountSpec`:
- `volume_id` (string, required)
- `mount_path` (string, required)
- `read_only` (bool, default false)
- `filesystem` (string, v1 `"ext4"`)
- `device_hint` (string, optional, for agent internal use)

Rules:
- Volumes are local and constrain placement. Scheduler must only assign an instance to a node that can satisfy the mounts.
- Agent must attach the volume device to the microVM and mount at `mount_path`.

#### Lifecycle and stop behavior
- `lifecycle` (optional)
  - `termination_grace_seconds` (default 10)
  - `restart_policy` (default `"always"`)
  - `max_retries` (default 3 when on-failure)

Rules:
- On transition to draining or stopped, agent must send termination signal and wait for grace.
- After grace, agent must force terminate.

## Update and rollout rules
The platform must avoid in-place mutation ambiguity.

v1 rule:
- A change in release digest, command, env vars, mounts, secrets version, or health config must cause a generation change.
- The scheduler should prefer rolling updates by creating new desired instances and draining old ones.
- The agent must treat generation changes as requiring replacement if the spec hash differs.

Recommended mechanism:
- include a deterministic `spec_hash` in WorkloadSpec (optional field)
- agent compares spec_hash to decide if an instance must be replaced

## Agent reconciliation rules (normative)
Given a NodePlan, the agent must:
1) Ensure every desired running instance exists and is healthy.
2) For draining instances:
   - stop advertising readiness
   - terminate after grace
3) For stopped instances:
   - ensure they are not running
4) Garbage collect orphaned artifacts:
   - old Firecracker sockets
   - scratch disks not referenced by any instance
   - cached root disks by policy (must not delete those in use)

The agent must be safe under:
- duplicate plans
- out-of-order plan delivery (use cursor_event_id and plan_id ordering)
- restarts and partial failures

## Failure reporting (required reason codes)
When an instance cannot reach ready state, the agent must report a structured reason, including one of:

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

These reason codes must map to event types in `docs/specs/state/event-types.md`.

## Security and trust boundaries
- Plans and specs are trusted inputs, delivered over authenticated channels.
- Workloads are untrusted code.
- Agents must never accept tenant input directly for WorkloadSpec fields.

## Notes for implementers
- Keep WorkloadSpec small and resolved. Agents should not need to re-interpret full manifests.
- Keep everything idempotent. A plan can be applied repeatedly without drift.
- Make “diffing desired vs actual” explicit and testable.

## Next specs that depend on this
- `docs/specs/runtime/*` (how to boot and wire the microVM)
- `docs/specs/networking/ipam.md` (how overlay_ipv6 is allocated)
- `docs/specs/scheduler/placement.md` (how node_id is selected)
- `docs/specs/state/event-types.md` (status and failure event catalog)

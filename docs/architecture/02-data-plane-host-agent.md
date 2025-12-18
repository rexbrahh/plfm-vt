# docs/architecture/02-data-plane-host-agent.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
This document describes the host agent (node agent) and how the data plane converges actual runtime state to the control plane’s desired state.

This is narrative. The authoritative contracts are:
- `docs/specs/manifest/workload-spec.md`
- `docs/specs/runtime/*`
- `docs/specs/networking/*`
- `docs/specs/storage/*`
- `docs/specs/secrets/*`
- ADRs 0001, 0003, 0004, 0010, 0011, 0012

## What the host agent is
A daemon running on every host (node) that:
- enrolls into the platform
- receives desired state targeting that node
- runs and supervises Firecracker microVM instances
- configures networking and attaches to the overlay
- attaches local volumes
- injects secrets
- reports status and health back to the control plane
- streams logs for CLI consumption

The host agent is the primary owner of “making desired state real” on a host.

## What the host agent is not
- Not a general-purpose container runtime.
- Not a place where user workloads can gain privileged host access.
- Not a multi-tenant API surface. It is controlled by the control plane.
- Not a scheduler. It does not decide placement; it only converges for allocations assigned to it.

## High-level responsibilities
### 1) Enrollment and identity
- Generate or load node identity material.
- Join the platform using a short-lived enrollment token or operator-approved workflow.
- Maintain:
  - an mTLS identity for control plane RPC
  - WireGuard keys for overlay membership
- Support rotation and revocation.

### 2) Desired state consumption
- Maintain an event cursor.
- Consume only relevant events:
  - allocations for this node
  - volume attach/detach intents for this node
  - secret version bindings for instances on this node
  - node configuration updates (MTU, reserved memory, etc)

### 3) Reconciliation loop
The agent runs a continuous reconciliation loop:
- Observe desired allocations (what should be running here).
- Observe actual state (what microVMs exist, their status, attached volumes, applied config).
- Perform actions to converge.

Critical properties:
- idempotent (safe to retry)
- restart-safe (agent can restart and continue)
- at-least-once tolerant (duplicate events do not corrupt state)
- bounded (does not spin or thrash under failure)

### 4) Runtime lifecycle management (Firecracker)
For each desired instance:
- prepare root filesystem artifacts
- create microVM configuration
- attach block devices (root disk, scratch disk, optional volumes)
- set resource limits (CPU shares/weights, memory hard cap)
- configure networking (tap, routes, overlay connectivity)
- boot microVM
- track readiness and health
- terminate and garbage collect when no longer desired

### 5) OCI image fetch and cache
- Pull OCI images by digest.
- Cache by content address.
- Verify digests and expected platform constraints.
- Convert OCI layers into a runtime root filesystem form compatible with Firecracker.

Recommended strategy (v1 stance):
- Build a read-only ext4 root disk per release digest.
- Attach a per-instance writable scratch disk.
- In guest init, overlay read-only root with scratch upperdir.

### 6) Secrets injection
- Fetch or receive the secret bundle version metadata for this instance.
- Materialize the secrets file into the microVM at a fixed mount point.
- Enforce strict permissions.
- Ensure the instance sees secrets consistent with its env binding.

Rotation default:
- new secrets version triggers a restart rollout rather than hot reload.

### 7) Volume attachment (local volumes)
- Ensure locality constraints are respected by scheduler (agent assumes desired placement is valid, but must still protect itself).
- Attach local block device to microVM.
- Mount inside microVM at a fixed mount point.
- Report attachment status.
- Participate in snapshot or backup operations if agent owns the pipeline.

### 8) Logging and telemetry
- Collect workload logs using a standard channel (serial console, vsock, or defined log device).
- Provide streaming logs to the control plane or log sink.
- Emit metrics:
  - microVM count, state, boot latencies
  - image cache hit rates
  - resource usage per instance
  - reconciliation loop outcomes and error counts
  - overlay link health

### 9) Exec and interactive sessions
- Support an exec mechanism authorized by control plane.
- Provide a secure channel into a running microVM.
- Enforce:
  - short-lived grants
  - audit logging
  - least privilege (exec is not a privileged host shell)

Implementation detail lives in runtime specs, but the agent owns execution.

## Agent internal model
### Local state store
The agent should maintain a small local state store (disk-backed) to survive restarts:
- instance id -> microVM id, socket paths, disks, current state
- last applied event cursor
- cache metadata for images and root disks
- volume mapping metadata
- last known node configuration

### Actual state observation
The agent must be able to reconstruct actual state from:
- persisted agent state
- scanning its microVM directories and Firecracker sockets
- querying Firecracker for VM status where possible

It must not assume in-memory state is the truth.

## Interfaces
### Agent <-> Control plane
- Authenticated and authorized via mTLS.
- Control plane sends desired allocations and grants.
- Agent reports:
  - heartbeats and capacity
  - instance lifecycle transitions
  - health status
  - logs and exec session metadata
  - volume attachment status

Exact message shapes are specified in `docs/specs/manifest/workload-spec.md` and API specs.

### Agent <-> Firecracker
- Firecracker API socket per microVM.
- Jailer integration for sandboxing.
- Seccomp profile selection.
- Device attachment is explicit and minimal.

### Agent <-> Host OS
- cgroup v2 management
- filesystem layout for:
  - cached image artifacts
  - microVM runtime directories
  - volume devices and mounts
- networking:
  - create tap devices
  - set routes and firewall rules (nftables)
  - configure WireGuard interface (or consume pre-configured interface managed by OS)

## Host OS assumptions
The architecture assumes hosts are configured in a reproducible way.
We intend to use NixOS for determinism, but the agent should not hardcode NixOS specifics.

Minimum requirements:
- Linux with KVM support
- cgroup v2 enabled
- ability to run Firecracker and jailer
- ability to create tap devices and configure routes
- WireGuard support
- a storage substrate for local volumes (recommended: LVM thin pool)

## Resource model enforcement
### CPU (soft)
- CPU requests map to cgroup weights (fair sharing).
- Scheduler may oversubscribe CPU at placement time.
- Agent enforces fairness, not strict reservation.

### Memory (hard)
- Memory limit is mandatory per instance.
- Agent enforces memory cap at the host boundary.
- If exceeded, the instance should fail locally and visibly (OOM/termination) without destabilizing host.

### Reserved capacity
The agent must budget host overhead:
- Firecracker overhead per microVM
- page cache behavior
- node services and control plane sidecars (if any)

Allocatable capacity is reported to the scheduler.

## Failure handling
### Agent restart
- Must resume reconciliation with no manual intervention.
- Must not leak microVMs, disks, or routes.

### Firecracker failure
- Agent detects failure, records reason, and restarts instance per policy.
- Repeated failures trigger backoff and clear reporting.

### Overlay partition
- Agent continues running workloads locally.
- Agent reports degraded connectivity.
- The platform should define what “ready” means when the node cannot reach edge or control plane.

### Disk pressure
- Cache eviction policy must exist.
- Volume storage must not be impacted by image cache eviction.
- Agent must surface “disk full” as a first-class alert.

## Security posture
Threat assumptions:
- Workloads are untrusted.
- The agent is trusted infrastructure code.

Security requirements:
- Strict filesystem permissions for microVM artifacts and secrets.
- No host filesystem mounts into guest except explicit volume mounts.
- Jailer usage and seccomp profiles for Firecracker.
- mTLS for control plane communications.
- Audit logging for exec and sensitive operations.

## Next document
- `docs/architecture/03-edge-ingress-egress.md`

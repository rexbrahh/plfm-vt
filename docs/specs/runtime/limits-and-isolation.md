# docs/specs/runtime/limits-and-isolation.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
Define the isolation and resource enforcement requirements for running workloads as Firecracker microVMs:
- cgroup v2 resource limits
- Firecracker jailer configuration (process containment)
- seccomp policy for the VMM
- host filesystem constraints and directory layout
- device access constraints for the jailed VMM process
- required failure reporting and reason codes

This spec is normative for the host agent implementation.

Locked decisions:
- microVM isolation boundary: `docs/ADRs/0001-isolation-microvm-per-instance.md`
- Firecracker runtime: `docs/ADRs/0003-runtime-firecracker.md`
- CPU soft, memory hard cap: `docs/ADRs/0012-scheduling-cpu-oversubscribe-mem-hardcap.md`

## Scope
This spec defines host-side enforcement. It does not define:
- guest init behavior (see `docs/specs/runtime/firecracker-boot.md`)
- image fetching and caching (see `docs/specs/runtime/image-fetch-and-cache.md`)
- overlay networking (see `docs/specs/networking/*`)

## Definitions
- **VMM process**: the Firecracker process (and its jailer wrapper) that runs a microVM.
- **Instance**: one running microVM for a specific `(env, process_type)` slot.
- **Hard cap**: a limit that must not be exceeded, and should fail the instance rather than destabilize the host.
- **Soft share**: a fairness target that can be exceeded or contended, with throttling under load.

## Core invariants
1) Workloads must not be able to access the host filesystem except via explicit attached volumes.
2) Workloads must not be able to access host devices.
3) The VMM process must run with minimal privileges, inside a jail, with seccomp enabled.
4) Memory must be enforced as a hard cap per instance at the host boundary.
5) CPU is enforced as a fairness share, not a strict reservation, unless explicitly configured later.
6) Secrets must not be written to persistent host storage in plaintext.

## cgroup v2 requirements (mandatory)
The host agent must place each instance VMM process into a dedicated cgroup v2.

### cgroup naming
- Path: `/sys/fs/cgroup/trc/instances/<instance_id>/`
- The host agent may add additional hierarchy, but instance_id must appear in the path.

### Memory enforcement (hard)
WorkloadSpec provides `memory_limit_bytes` as the guest memory size and the primary budgeting unit.

The host agent must enforce:
- `memory.max = memory_limit_bytes + vmm_overhead_bytes`
- `memory.swap.max = 0`

Where:
- `vmm_overhead_bytes` is an operator-configured constant that budgets VMM overhead (default recommendation: 64Mi).
- Scheduler budgeting should account for this overhead as part of allocatable memory, but the agent must enforce it even if scheduler is wrong.

OOM behavior:
- If the cgroup OOM kills the VMM process, the agent reports the instance as failed with reason `oom_killed`.
- The platform must prefer failing the instance over destabilizing the host.

### CPU enforcement (soft)
WorkloadSpec provides `cpu_request` as a soft share target.

The host agent must enforce CPU fairness using cgroup v2 weights:
- `cpu.weight = clamp(1, 10000, round(cpu_request * 100))`

Examples:
- cpu_request 1.0 -> weight 100
- cpu_request 0.5 -> weight 50
- cpu_request 2.0 -> weight 200

v1 rule:
- Do not enforce `cpu.max` by default.
- The platform may introduce a separate `cpu_limit` later if needed.

### Optional controls (allowed, not required in v1)
- `io.weight` for disk fairness
- `pids.max` to cap process count of the VMM wrapper and helpers

If `pids.max` is used, it must not block normal Firecracker operation.

## Firecracker memory sizing (guest vs host)
The host agent must set Firecracker guest memory to exactly `memory_limit_bytes` from WorkloadSpec.

Reason:
- Predictable behavior for tenants.
- Aligns with scheduler accounting.

The cgroup memory.max includes overhead to avoid killing the VMM due to metadata and housekeeping.

## Jailer configuration (mandatory)
The host agent must run Firecracker via a jailer (Firecracker jailer or equivalent) to provide:
- chroot into a per-instance directory
- privilege dropping (unprivileged uid/gid)
- namespace isolation where feasible
- controlled device access inside the jail
- no_new_privs

### Per-instance sandbox directory layout
The host agent must create a per-instance sandbox directory:
- `/var/lib/trc-agent/instances/<instance_id>/`

Inside it, at minimum:
- `rootfs/` (mount point, not required to persist)
- `drives/`
  - `root.ext4` (read-only root disk file or bind reference)
  - `scratch.ext4` (read-write scratch disk file)
- `firecracker/`
  - `api.sock`
  - `metrics.fifo` (optional)
  - `log.fifo` (optional)
- `dev/` (device nodes or bind mounts, see below)

Constraints:
- Directory permissions must prevent other users from reading or writing:
  - sandbox dir mode 0700
- Files must be owned by the jailer user.

### Privilege dropping
The VMM process must not run as root.

v1 recommendation:
- Run jailer as root (or a tightly-capabilitied service) only for setup.
- Drop to a dedicated unprivileged user, example `trc-fc`, for the Firecracker process.

The Firecracker process must have:
- no ambient capabilities
- no setuid binaries accessible in its jail
- `PR_SET_NO_NEW_PRIVS` enabled

### Namespaces
Minimum required:
- mount namespace (isolate filesystem view)
- PID namespace (avoid host PID visibility)

Optional, if operationally safe:
- user namespace (for further privilege isolation)

v1 note:
- network namespace isolation for Firecracker is optional. If used, ensure the tap device remains usable.

## Seccomp policy (mandatory)
The Firecracker VMM process must run with seccomp enabled.

### Policy source
v1 recommendation:
- Use Firecracker’s default seccomp filter set appropriate for the host architecture, with minimal modifications.

### Enforcement rules
- Seccomp must be enabled for all instances.
- If seccomp cannot be applied, instance start must fail.

Failure reporting:
- reason code: `firecracker_start_failed`
- reason detail: `seccomp_apply_failed`

## Device access inside jail (mandatory)
The jailed VMM process must have access only to the minimum host devices needed.

Required devices (typical Firecracker needs):
- `/dev/kvm`
- `/dev/net/tun` (for tap interface)
- `/dev/vhost-vsock` (for vsock)
- `/dev/urandom` (or `/dev/random` where required)

Rules:
- Provide these via bind mounts or device nodes inside the jail.
- Do not expose block devices other than the explicit drives and attached volumes.
- Do not expose `/dev/mem`, `/dev/sda`, or similar host devices.

If the platform supports additional devices later, it requires a security review and likely an ADR.

## Filesystem constraints (mandatory)
### No host filesystem mounts
A workload must not see the host filesystem.

Allowed attachments into microVM:
- root disk (read-only, built from image)
- scratch disk (per instance)
- explicit volume block devices (local volumes)

Disallowed in v1:
- bind-mounting arbitrary host paths into the guest
- virtiofs mounts
- exposing host sockets into guest

### Reserved mount paths inside guest
Guest init enforces and the platform validates:
- mounts cannot target `/proc`, `/sys`, `/dev`, `/run`, `/run/secrets`, `/tmp`, or `/`

See `docs/specs/runtime/volume-mounts.md` and `docs/specs/runtime/firecracker-boot.md`.

### Symlink and path traversal safety
When preparing sandbox files (root disk paths, scratch disk paths, sockets):
- host agent must open files with `O_NOFOLLOW` where possible
- host agent must resolve and validate that all paths are under the sandbox directory
- do not follow symlinks that escape the sandbox

## Secrets handling constraints (mandatory)
Secrets are delivered to the guest as a file at:
- `/run/secrets/platform.env`

v1 delivery intent:
- secret material should be transferred via vsock and written by guest init to tmpfs.

Host-side rules:
- secret material must not be persisted on disk in plaintext.
- if a temporary file is unavoidable, it must be:
  - stored on tmpfs
  - mode 0600
  - deleted immediately after use
- host agent logs must never include secret material.

Failure reporting:
- missing secrets when required -> `secrets_missing`
- injection failures -> `secrets_injection_failed`

## Network exposure constraints (mandatory)
- The VMM process must not listen on public interfaces for control APIs.
- Firecracker API socket must be a unix socket inside the sandbox directory and not world-readable.

The only intended external connectivity for workloads is via:
- the virtio-net interface inside the guest
- L4 routing at the edge

## Rate limiting and denial-of-service considerations
v1 minimum:
- enforce per-instance memory hard cap
- enforce CPU fairness
- cap disk usage via scratch disk size
- avoid unbounded file growth in sandbox directories

Recommended additional controls:
- cap log buffer sizes per instance
- cap maximum concurrent exec sessions per org

## Failure reporting (normative)
When isolation or limit enforcement fails, the host agent must report `instance.status_changed` events with clear reason codes.

Required reason codes (subset relevant to this spec):
- `firecracker_start_failed`
- `network_setup_failed`
- `oom_killed`
- `crash_loop_backoff`
- `secrets_injection_failed`
- `volume_attach_failed`

Recommended reason_detail values for this spec:
- `seccomp_apply_failed`
- `jailer_setup_failed`
- `device_access_denied`
- `cgroup_setup_failed`
- `memory_limit_exceeded`
- `sandbox_path_invalid`

## Compliance tests (required)
Automated tests must validate:
1) Each instance VMM runs as non-root and has no ambient capabilities.
2) cgroup v2 limits are applied:
   - memory.max set and swap disabled
   - cpu.weight set deterministically
3) seccomp is enabled for the VMM process.
4) The Firecracker API socket is not reachable outside the sandbox directory.
5) The sandbox directory contains only the expected files, and permissions prevent cross-instance access.
6) A forced memory stress inside the guest triggers OOM behavior without destabilizing the host, and reason `oom_killed` is reported.

## Open questions (future)
- Whether to enforce additional syscall restrictions inside the guest (beyond virtualization) using guest kernel lockdown features.
- Whether to support optional stricter egress policies per org or env.
- Whether to support optional cpu.max limits for certain tiers.

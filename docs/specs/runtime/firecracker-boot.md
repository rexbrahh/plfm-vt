# docs/specs/runtime/firecracker-boot.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
Define the runtime boot contract for workload instances running as Firecracker microVMs:
- kernel selection and boot args
- root filesystem strategy
- guest init responsibilities (PID 1)
- vsock usage and control channels
- required mount points and reserved paths

This spec is normative for the host agent and the guest init implementation.

Locked decisions:
- MicroVM per environment (per instance): `docs/adr/0001-isolation-microvm-per-env.md`
- Firecracker runtime: `docs/adr/0003-runtime-firecracker.md`
- OCI image + manifest: `docs/adr/0002-artifact-oci-image-plus-manifest.md`
- Secrets delivered as a fixed-format file: `docs/adr/0010-secrets-delivery-file-format.md`

## Scope
This spec defines the boot contract only.

This spec does not define:
- image pulling and caching (`docs/specs/runtime/image-fetch-and-cache.md`)
- networking inside the VM details (`docs/specs/runtime/networking-inside-vm.md`)
- volume attach semantics (`docs/specs/runtime/volume-mounts.md`)
- cgroups, seccomp, jailer, filesystem constraints (`docs/specs/runtime/limits-and-isolation.md`)

## Definitions
- **Instance**: one running replica of a process type, implemented as one microVM.
- **Root disk**: read-only block device derived from an OCI image digest.
- **Scratch disk**: writable per-instance block device used for overlay upperdir and workdir.
- **Guest init**: platform-provided PID 1 inside the microVM, responsible for preparing the runtime environment and launching the workload entrypoint.
- **WorkloadSpec**: resolved runtime config produced by control plane and delivered to the host agent (`docs/specs/workload-spec.md`).

## High-level boot architecture (v1)
Each instance microVM is configured with:

1) A platform-selected Linux kernel (no user-supplied kernel in v1).
2) A platform-provided guest init (PID 1).
3) Block devices:
   - `vda`: root disk (read-only ext4)
   - `vdb`: scratch disk (read-write ext4)
   - `vdc...`: optional volume devices (read-write ext4 by default)
4) One virtio-net device (eth0).
5) One virtio-vsock device for control plane to guest coordination (config handshake, exec plumbing).

The guest init performs:
- mount and pivot into overlay root
- network config
- secrets file materialization at a fixed path
- volume mounts
- launching the user entrypoint
- signal forwarding and process supervision

## Kernel requirements
### Supported OS and architectures
- OS: Linux only
- Arch: amd64 and arm64 (as hosts are added)

### Kernel configuration requirements
Kernel must include:
- virtio block, virtio net, virtio rng
- virtio vsock (AF_VSOCK)
- ext4
- overlayfs
- tmpfs
- procfs and sysfs
- basic IPv6 stack
- nftables is optional inside guest (not required in v1)

Kernel must not require:
- initramfs (allowed, but not required)
- systemd

### Kernel versioning
- Kernel version is controlled by the platform.
- Upgrades are operator-driven and must be staged.
- The platform maintains a compatibility matrix for kernels vs guest init versions.

## Kernel command line (boot args)
Constraints:
- Kernel cmdline must remain small and stable.
- Do not encode full WorkloadSpec into cmdline.

Recommended cmdline (illustrative):
- `console=ttyS0`
- `panic=1`
- `pci=off`
- `reboot=k`
- `ipv6.disable=0`

Optional:
- `init=/sbin/trc-init`

Notes:
- Root filesystem is not mounted by the kernel. The guest init mounts disks and pivots root.

## Root filesystem strategy (v1)
### Decision
Use an overlay root:
- lowerdir from a read-only root disk (per release digest)
- upperdir and workdir from a per-instance scratch disk

Rationale:
- Cache efficiency: one root disk per digest, reused across instances.
- Correctness: workloads can write to filesystem paths without mutating shared base.
- Simplicity: works on ext4 consistently.

### Disk layout
- `vda` (root disk): ext4, mounted read-only at `/mnt/lower`
- `vdb` (scratch disk): ext4, mounted read-write at `/mnt/scratch`

The guest init creates:
- `/mnt/scratch/upper`
- `/mnt/scratch/work`

Then mounts:
- overlayfs at `/mnt/newroot` with:
  - `lowerdir=/mnt/lower`
  - `upperdir=/mnt/scratch/upper`
  - `workdir=/mnt/scratch/work`

Then `pivot_root` into `/mnt/newroot`.

### Runtime tmp directories
Within the new root, guest init mounts:
- `tmpfs` at `/run`
- `tmpfs` at `/tmp`

This prevents `/run` and `/tmp` from depending on scratch disk I/O and avoids filling scratch due to transient files.

## Guest init (PID 1) contract
### Responsibilities (normative)
Guest init must:

1) Mount minimal pseudo-filesystems:
- `/proc`
- `/sys`
- `/dev` (devtmpfs if available)
- `/run` (tmpfs)
- `/tmp` (tmpfs)

2) Prepare overlay root and pivot into it (as described above).

3) Configure networking for eth0:
- set MTU
- assign overlay IPv6 address
- set default route via configured gateway
- write DNS config if provided

4) Materialize secrets file at a fixed path:
- Path: `/run/secrets/platform.env`
- Permissions: `0400` by default
- Owner: root by default
- Contents: platform secrets format (v1), as defined in `docs/specs/secrets/format.md`

5) Mount volumes for this instance:
- For each mount in WorkloadSpec, mount the corresponding block device to the configured mount_path.
- Enforce read_only flag when requested.
- Refuse mounts to reserved paths (same constraints as manifest validation).

6) Apply environment variables and working directory:
- Compose env vars from WorkloadSpec `env_vars`
- Set working directory if provided

7) Execute the workload entrypoint:
- Exec `command` (argv array)
- Forward signals
- Reap zombies
- Return the workload exit code as the microVM exit status (reported by agent)

8) Emit boot and failure diagnostics:
- Write critical lifecycle logs to `stdout`/serial console.
- Do not emit secret material.

### What guest init must not do (v1)
- Must not run an SSH daemon.
- Must not expose a metadata HTTP service.
- Must not accept inbound control connections from the public network.
- Must not attempt to auto-configure itself by calling external endpoints.

### Process supervision model
- Guest init remains PID 1 for the life of the microVM.
- The workload process is a direct child of PID 1.
- PID 1 forwards SIGTERM and SIGINT to the workload on shutdown.
- If workload exits, PID 1 exits with the same code (unless overridden by policy).

Restart policy is enforced by the host agent, not inside the guest.

## Workload configuration delivery to the guest
### Requirement
Guest init needs a minimal set of inputs:
- command (argv)
- env vars
- workdir
- network config (IPv6, gateway, MTU, DNS)
- volume mount list
- secrets presence and secret version binding

### v1 mechanism (normative): vsock config handshake
The platform uses vsock to deliver a resolved configuration blob from host agent to guest init at boot.

Contract:
- Guest init MUST initiate a vsock request for its configuration.
- Host agent MUST respond with the full resolved config for that instance.

This avoids:
- encoding large config in kernel cmdline
- shipping a separate config disk in v1
- relying on an HTTP metadata service

The exact message schema is implementation-defined but must include a version field:
- `config_version = "v1"`

Minimum required fields in the config response:
- `instance_id`
- `generation`
- `command`
- `env_vars`
- `workdir`
- `network` (overlay_ipv6, gateway_ipv6, mtu, dns)
- `mounts`
- `secrets` (required flag, secret_version_id or equivalent reference)

Security requirements:
- The host agent must only deliver config to the correct microVM.
- The vsock endpoint must not be reachable from outside the host.

Failure behavior:
- If config handshake fails or times out, guest init must exit non-zero and write a clear error to console.

## Vsock usage (v1)
Vsock is reserved for platform control channels:
1) Boot-time config handshake (required)
2) Optional exec sessions (future, but reserved)

Vsock must not be used for:
- general workload networking
- arbitrary tenant-provided protocols

Recommended vsock allocation:
- One vsock device per microVM.
- Guest CID is set per microVM by the host agent.
- Guest init uses a fixed port for config requests.

The concrete CID/port numbers are implementation-defined, but must be consistent across agent and init.

## Secrets delivery semantics inside the guest
- The end result must be a file at `/run/secrets/platform.env`.
- The file content must be exactly the platform secrets format (v1).
- Rotation semantics are restart-based by default:
  - the platform rolls instances so new instances boot with the new secrets version

The guest init must not attempt hot-reload by default.

## Volume attachment mapping
Block devices for volumes are attached by the host agent.
Guest init mounts them according to WorkloadSpec.

v1 assumptions:
- Filesystem: ext4
- Each mount refers to exactly one block device
- No shared multi-writer semantics

Mount order:
- root overlay prepared first
- secrets prepared next
- volume mounts prepared next
- workload exec last

If a required volume mount fails:
- guest init must exit non-zero
- agent reports `volume_attach_failed`

## Readiness signaling (v1)
Readiness is determined by the host agent via health checks defined in WorkloadSpec.

Guest init helps by:
- writing a clear log line when it has:
  - configured networking
  - mounted secrets and volumes
  - started the workload process

Agent starts the health check grace period when it sees the microVM is running (and optionally after a vsock “config applied” ack if implemented).

## Error handling and reason mapping
Guest init failure modes must map to agent reason codes:
- config handshake failure -> `network_setup_failed` or a dedicated `config_fetch_failed` (if we add it)
- overlay mount failure -> `rootfs_build_failed` (if disk malformed) or `firecracker_start_failed` (if cannot boot at all)
- secrets required but missing -> `secrets_missing`
- secrets file write failure -> `secrets_injection_failed`
- volume mount failure -> `volume_attach_failed`

Guest init must never print secret contents in logs.

## Reserved paths inside the guest (v1)
These paths are platform-reserved and must not be used as mount targets:
- `/proc`
- `/sys`
- `/dev`
- `/run` (tmpfs)
- `/run/secrets` (platform-owned)
- `/tmp` (tmpfs)

Secrets file fixed path:
- `/run/secrets/platform.env`

## Compliance tests (required)
The runtime implementation must ship automated tests that:
1) Boot a microVM from a known root disk and scratch disk.
2) Verify overlay root works (writes do not mutate base root disk).
3) Verify secrets file exists at `/run/secrets/platform.env` with correct permissions.
4) Verify a volume block device mounts to the requested path.
5) Verify the workload command runs and exits, and exit codes are observable.
6) Verify vsock config handshake failure produces a clear error and fails fast.

## Open questions (explicitly deferred to other runtime specs)
- Image fetch and cache behavior: `docs/specs/runtime/image-fetch-and-cache.md`
- Detailed guest networking config and IPAM integration: `docs/specs/runtime/networking-inside-vm.md`
- Volume device discovery conventions: `docs/specs/runtime/volume-mounts.md`
- Host hardening: `docs/specs/runtime/limits-and-isolation.md`

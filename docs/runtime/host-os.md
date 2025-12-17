# Host OS: NixOS

This document defines the required behavior of the bare metal host OS for libghostty-vt PaaS worker nodes, and the canonical NixOS implementation. It is the companion to ADR-013 (Target NixOS as the Bare Metal Host OS).

## Scope

This document covers:

- What runs on the host vs what runs in microVMs
- The host interface contract (filesystem, networking, isolation, observability)
- NixOS specific implementation constraints (pinning, upgrades, rollback, GC)
- Security guardrails (especially secrets and binary cache trust)
- Operational runbooks for node lifecycle (bootstrap, drain, upgrade, decommission)

Out of scope:

- Control plane deployment (covered elsewhere)
- Guest OS requirements (covered in the microVM guest contract docs)
- Product and UX surfaces (CLI, console)

## Definitions

- **Host**: The bare metal machine running NixOS.
- **VMM**: The microVM monitor process (Firecracker) running on the host and using KVM.
- **MicroVM**: The guest VM running a workload instance.
- **Node agent**: Host daemon that reconciles node state (microVM lifecycle, networking, secrets materialization, telemetry).

## Responsibilities split

### Host responsibilities
- Run the node agent and supporting host daemons
- Provide KVM, cgroup v2 isolation, and stable resource accounting per microVM
- Provide IPv6 first networking, WireGuard overlay, and L4 ingress plumbing
- Provide local caching for OCI images and other runtime artifacts
- Provide volume attachment plumbing for persistent storage
- Materialize secrets into runtime paths without ever putting secrets in the Nix store
- Emit host and workload telemetry (logs, metrics, events)

### MicroVM responsibilities
- Run the customer workload process tree
- Consume secrets from the fixed, reconciled format and path exposed to the guest
- Emit workload logs to stdout and stderr and optionally to a structured sink
- Follow the guest contract for networking, health, and shutdown

## Host interface contract

This section is the contract that other docs rely on. It is implemented with NixOS in v1.

### Hardware and kernel requirements
Minimum requirements for production nodes:

- CPU supports hardware virtualization (Intel VT-x or AMD-V)
- KVM available and enabled
- Sufficient RAM for expected microVM density plus host overhead
- Reliable local storage (NVMe preferred) for /nix plus runtime caches
- IPv6 connectivity is strongly preferred and assumed by default for endpoints

Kernel and OS requirements:

- Linux with KVM support
- cgroup v2 enabled (unified hierarchy)
- nftables available for firewalling and policy
- WireGuard available (kernel module preferred)
- systemd as the service manager (NixOS uses systemd)

### Identity and time
- Each node has a stable **node ID** (not derived from hostname).
- Time sync must be enabled (chrony or systemd-timesyncd).
- Node agent must reject large clock skews.

### Filesystem layout

The host must provide stable, non-secret paths for runtime state. Canonical layout:

- `/nix`  
  Nix store and system profile generations. Do not place runtime state here.

- `/var/lib/vt/`  
  Persistent host runtime state.
  - `/var/lib/vt/images/` OCI image cache and unpacked layers
  - `/var/lib/vt/volumes/` volume attachment staging and metadata
  - `/var/lib/vt/vm/` microVM rootfs artifacts, ephemeral disk templates, metadata
  - `/var/lib/vt/state/` node agent durable state (non-secret)

- `/run/vt/`  
  Runtime ephemeral state (tmpfs).
  - `/run/vt/secrets/` runtime secret materialization (host side)
  - `/run/vt/guests/<guest-id>/` per guest runtime data, sockets, taps, vsock
  - `/run/vt/events/` transient event buffers if needed

- `/var/log/`  
  Host logs are primarily in journald, but some components may write structured logs here.
  Prefer journald for all host services.

Principles:
- Secrets must not be written to `/var/lib/vt` unless explicitly encrypted at rest and justified.
- Anything under `/run` is assumed ephemeral and cleared on reboot.
- Node replacement should be possible by reprovisioning and reattaching volumes.

### Networking

#### Baseline
The host must support:

- IPv6 first addressing and routing
- WireGuard overlay (node-to-node private network)
- L4 ingress that does not terminate TCP by default
- Dedicated IPv4 as an optional add-on (where available)

#### Interfaces
Canonical host interfaces (names are illustrative but should be stable):

- `wg-vt0`  
  WireGuard overlay interface. Used for control plane connectivity and east-west traffic.

- `br-vt0`  
  Bridge for microVM tap interfaces (or equivalent wiring). Hosts attach taps per guest.

- `tap-vt-<guest-id>`  
  Per microVM tap. Owned and managed by the node agent.

#### Firewall and policy
- nftables is the canonical firewall layer.
- Default deny inbound on the public interface except explicitly configured ingress ports.
- Allow required egress for:
  - control plane connectivity
  - image pulls (configurable)
  - time sync
  - observability shipping (if off-host)

Ingress responsibilities:
- L4 ingress should preserve end-to-end semantics.
- Proxy Protocol v2 may be enabled per endpoint where required.
- IPv6 is the default external connectivity. IPv4 may be provisioned per endpoint via the IPv4 add-on.

### Isolation and resource control

- cgroup v2 is mandatory.
- Each microVM is represented by a cgroup subtree that contains:
  - the Firecracker VMM process
  - any helper processes needed for the VM
- CPU, memory, IO limits are applied at the cgroup level and are the source of truth for enforcement.
- The node agent must expose per microVM accounting (CPU time, RSS, throttling, OOMs).

### Secrets handling

Hard rules:
- No secrets in the Nix store.
- No secrets in Nix derivations, module option values that end up in derivations, or build inputs.
- Host services that need secrets must consume them from runtime paths such as `/run/vt/secrets`.

Secrets delivery model:
- The control plane reconciles desired secret bundles for each workload instance.
- The node agent materializes those secrets into a fixed file format and location that the guest can read.
- The guest must not assume immediate convergence. It must handle eventual consistency.

Canonical guest exposure approaches (choose and standardize in the secrets delivery docs):
- A read-only block device attached to the microVM containing the secrets file(s)
- A tmpfs or ramdisk provided at boot with the secrets payload
- A vsock based fetch protocol with an on-guest agent that writes the fixed format

Regardless of mechanism:
- Secret materialization on the host must use strict permissions.
- Secrets must have explicit rotation behavior and a clear “current vs desired” reconciliation state.

### Observability

Host requirements:
- journald is authoritative for host service logs.
- Node agent emits events for state transitions (start, stop, crash, health changes, reconcile actions).
- Metrics exporter on the host exposes:
  - node level CPU, memory, disk, network
  - per microVM resource stats
  - node agent reconcile loop health and queue depth

Workload logs:
- Workloads should emit logs to stdout and stderr.
- The node agent collects and streams logs, and provides a tail mechanism.

## NixOS implementation

### Pinning strategy
- All host nodes are built from a pinned NixOS configuration.
- The pin must be explicit (flake lock or pinned nixpkgs).
- Rolling forward the pin is an intentional change with a documented cadence.

### Service topology
Canonical host services (names illustrative):

- `vt-node-agent.service`  
  Main reconciler for node state and microVM lifecycle.

- `vt-netd.service`  
  Optional helper for interface wiring, nftables rules, IPAM hooks.

- `vt-imaged.service`  
  OCI image pull, verify, unpack, cache management.

- `vt-volumed.service`  
  Volume attachment orchestration, mount plumbing, snapshot hooks.

- `vt-telemetry.service`  
  Metrics exporter and optional log shipper.

The NixOS module should define these as systemd units with:
- explicit dependencies
- restart policies
- resource limits
- hardening settings

### systemd hardening baseline
For all host daemons, enable a baseline set of systemd protections where compatible:

- NoNewPrivileges
- ProtectSystem
- ProtectHome
- PrivateTmp
- RestrictAddressFamilies (allow only what is needed)
- CapabilityBoundingSet (minimal)
- SystemCallFilter (where practical)

Do not enable protections that break needed KVM, tap, or netlink operations. Prefer to harden incrementally and test.

### Nix store and garbage collection
- Define a GC policy explicitly. Do not rely on ad hoc cleanup.
- Prefer scheduled GC with safeguards:
  - maintain a minimum free space threshold
  - never GC while in the middle of a rolling upgrade
- Separate disk budgeting:
  - /nix store growth
  - image cache growth
  - volume data and metadata
  - logs

The node agent must surface disk pressure signals as events and should refuse scheduling new workloads if host storage is below safe thresholds.

### Binary caches and substituters
- Production nodes must use an explicit allowlist of substituters.
- Trusted keys must be pinned and rotated via a documented procedure.
- Disable “accept everything” cache behavior by default.

## Operational runbooks

### Bootstrap a new node
1. Provision NixOS with the worker node role configuration.
2. Ensure required kernel features are enabled (KVM, cgroup v2, WireGuard).
3. Bring up `wg-vt0` and verify overlay connectivity to the control plane.
4. Start host services:
   - vt-node-agent
   - vt-imaged
   - vt-telemetry
5. Enroll the node with the control plane using a bootstrap token.
6. Verify node reports healthy and is schedulable.

### Drain a node
1. Mark node as draining in the control plane (no new placements).
2. Node agent begins eviction:
   - stop accepting new workloads
   - migrate or reschedule workloads according to policy
3. Confirm all workload instances are stopped or moved.
4. Mark node unschedulable and proceed with maintenance.

### Upgrade a node
1. Select a canary node and drain it.
2. Apply the new pinned NixOS config.
3. Restart required services and validate:
   - overlay connectivity
   - ingress rules
   - image pulls
   - microVM launch
4. Un-drain and observe stability.
5. Roll out in small batches.

### Roll back a node
1. Drain the node.
2. Roll back to the previous system generation.
3. Restart services and validate health.
4. Un-drain once stable.

### Decommission a node
1. Drain and evacuate workloads.
2. Detach or migrate volumes per the storage policy.
3. Revoke node credentials and overlay keys.
4. Remove from control plane inventory.

## Validation checklist

Before a node is considered production ready:

- KVM works and Firecracker can boot a microVM
- cgroup v2 is enabled and limits apply correctly
- WireGuard overlay stable and reachable
- nftables policy applied and survives reboot
- IPv6 ingress works end-to-end
- Secrets are delivered without touching the Nix store
- Logs and metrics are visible from the control plane
- Upgrade and rollback procedure verified on a canary

## Portability notes

Although v1 targets NixOS, the rest of the system should rely on the host contract, not NixOS internals.

To preserve future portability:
- Keep host assumptions documented in this file, not scattered
- Avoid leaking NixOS specific paths into guest contracts
- Treat NixOS as the reference implementation of the host contract

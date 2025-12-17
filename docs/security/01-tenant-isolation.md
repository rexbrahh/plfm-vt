# Tenant isolation

Last updated: 2025-12-17

This document defines the isolation model for running untrusted customer workloads in a multi tenant PaaS using Firecracker microVMs. It focuses on preventing tenant to tenant access and tenant to platform compromise.

## Isolation goals

- Confidentiality: a tenant cannot read another tenant's data or secrets.
- Integrity: a tenant cannot influence another tenant's runtime, configuration, or networking.
- Availability: a tenant cannot materially degrade another tenant beyond defined quotas and shared component limits.
- Containment: a compromised workload should have a small blast radius and clear forensics signals.

## Isolation boundaries

Primary boundaries, from strongest to weakest.

1. MicroVM boundary: guest kernel and process space separated from host.
2. Host boundary: host kernel and host agent processes separated from control plane by identity and ACLs.
3. Tenant resource boundary: volumes, endpoints, secrets, and logs are scoped and enforced by immutable ids.
4. Network boundary: overlay network and edge enforce identity and prevent spoofing.

## Compute isolation

### Firecracker microVM hardening

- Firecracker runs with a minimal device model and a restrictive seccomp profile.
- Host kernels are treated as a critical dependency with rapid patching and reboot policies.
- Guest images should be minimal and updated frequently. Prefer distroless or minimal base images.

Controls:
- Dedicated unprivileged user for Firecracker processes.
- No extra Linux capabilities for the Firecracker process beyond what is required.
- cgroups enforced limits for CPU, memory, and IO.
- Disable or restrict high risk kernel features where feasible (for example unprivileged user namespaces).
- Host level syscall filters for host agent and VM launcher components.

### Resource isolation and fairness

Threats:
- CPU starvation, memory pressure, disk IO saturation, inode exhaustion.
- Connection storms and packet floods impacting shared network resources.
- Host agent overload due to rapid lifecycle churn.

Controls:
- Per VM quotas and limits enforced by cgroups.
- Per tenant and per org quotas in the scheduler.
- Rate limits on lifecycle operations (start, stop, scale, release).
- Backpressure and bounded queues for host agent control channels.

## Storage isolation

### Volume identity and attachment

Threats:
- Wrong volume attached to wrong VM due to id confusion.
- Snapshot restore binds a tenant to a different tenant's data.
- Stale host metadata leads to reuse of a previous tenant's mount path.

Controls:
- Volumes, snapshots, and backups use stable immutable ids.
- Attachment operations require both the volume id and the target workload id, with server side verification.
- Host agent keeps no authoritative mapping without control plane signed instructions.
- Strong ownership checks at every step: org, project, app, env, workload.

### Data at rest

Controls:
- Encrypt volumes, snapshots, and backups at rest (per tenant or per env keys).
- Secure wipe policies for ephemeral disks and scratch space used during image unpack and volume staging.
- Never reuse ephemeral storage without a wipe when tenant identity changes.

### File permissions and mounts

Controls:
- Secrets and sensitive runtime files should be on tmpfs where possible.
- Mount options should restrict execution where feasible (`noexec`, `nodev`, `nosuid`) for data volumes.
- Host mount namespaces isolate per VM mount operations.

## Network isolation

### Overlay network and spoofing prevention

Threats:
- Tenant spoofs source IP or identity metadata.
- Tenant attempts lateral movement to another tenant over the overlay.
- Tenant attempts to reach host services or control plane internal services.

Controls:
- Default deny network policy between tenant namespaces.
- Strong egress controls from tenants to control plane endpoints (only needed endpoints).
- Host firewall rules prevent guests from reaching host network namespaces or metadata services.
- Overlay tunnels are authenticated (WireGuard keys) and bound to node identity.
- Packet filters enforce that a tenant can only send traffic with its assigned IPs.

### Ingress and Proxy Protocol v2

Threats:
- Untrusted Proxy Protocol headers used to bypass allowlists and rate limits.
- Protocol confusion where a tenant sends proxy headers directly to an app port.

Controls:
- Proxy Protocol is only enabled on explicit endpoints and dedicated listeners.
- Only accept Proxy Protocol from trusted upstream proxies by IP allowlist and mutual auth.
- Expose original remote address to tenants as informational, never as an auth input.

### Egress controls

Threats:
- Workload used as a botnet node or to scan internal networks.
- Exfiltration of data at scale.

Controls:
- Optional egress policies per env and per app.
- Baseline protections: block access to known internal IP ranges and platform control plane subnets unless required.
- Rate limits and anomaly detection for unusual egress patterns.

## Metadata and credentials boundaries

Avoid a traditional instance metadata service that hands out platform credentials. If any metadata endpoint exists, it must only serve non sensitive runtime information and must be locked to the VM boundary.

Controls:
- No long lived platform credentials inside guests.
- Any per workload identity token is short lived, scoped, and audience restricted.
- Debug endpoints are not accessible from tenant networks by default.

## Side channels

We treat side channels as reduced but not eliminated risk.

Threats:
- CPU cache side channels, speculative execution class issues.
- Cross VM timing side channels via shared resources.

Controls:
- Keep host kernels and microcode patched.
- Consider scheduling policies that reduce co residence of high risk tenants on shared hosts.
- Provide a higher isolation tier option in the future (dedicated host or dedicated cores).

## Detection and response signals

Minimum signals to collect for tenant isolation incidents.

- Host level alerts for abnormal syscalls or seccomp violations in Firecracker processes.
- Unexpected network flows between tenant segments.
- Volume attach mismatches and repeated mount failures.
- Spikes in denied authz events that suggest probing.
- Integrity checks for host agent binaries and config.

## Testing

- Automated cross tenant network reachability tests (should fail by default).
- Volume misbinding tests and snapshot restore scoping tests.
- Load tests for noisy neighbor scenarios and lifecycle churn.
- Periodic review of Firecracker and kernel CVEs and mitigation status.

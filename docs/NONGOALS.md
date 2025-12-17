# docs/NONGOALS.md

Status: reviewed  
Owner: TBD  
Last reviewed: 2025-12-16

This file exists to prevent scope creep. If something is listed here, it is not part of v1 unless a new ADR or explicit decision removes it.

## Not a goal in v1 (explicitly deferred)

### Product scope
- A Kubernetes-compatible platform (no CRDs, no Helm, no “just run k8s workloads” promise).
- A generic VM hosting product where users manage their own guest OS.
- A serverless platform (no per-request microVMs, no function runtime).
- A full web UI as the primary control surface. CLI is the product in v1.
- Multi-service “app bundles” (no docker-compose, no Helm charts, no multi-app deploy units).

### Build and artifact
- Buildpacks or platform-managed builds.
- “Git push to deploy” as the primary workflow.
- Accepting non-OCI artifacts (zip uploads, tarballs) as first-class deploy units.

### Networking
- L7-first ingress, default TLS termination, WAF, CDN, caching.
- Guaranteed support for clients that do not send SNI, unless the user buys dedicated address-based routing.
- Shared IPv4 by default. IPv4 requires explicit add-on allocation.
- Advanced multi-region routing and global anycast in v1.
- Complex SDN stacks or Kubernetes-centric networking requirements.

### Runtime and isolation
- User-supplied kernels or arbitrary VM images.
- Nested virtualization.
- Privileged host access from workloads.
- User-defined sidecars as a platform feature.

### Storage
- A distributed filesystem or shared network storage as the default.
- Multi-writer shared volumes.
- Synchronous volume replication or zero-downtime failover for stateful workloads.
- Transparent live migration of stateful workloads between hosts.

### Control plane
- Multi-region active-active writes.
- External event store dependency as a hard requirement.
- Heavy business logic in database stored procedures.

### Secrets
- Environment variables as the default secrets delivery mechanism.
- Mandatory integration with Vault or a cloud KMS in v1.
- Hot-reloaded secrets as the default (rotation triggers restarts in v1).

### Observability
- A full “managed observability” product with complex query UI and billing.
- Automatic, perfect distributed tracing for every workload with no user effort.

## Not a goal at any time (out of scope by design)
- Providing anonymity services or bypassing network policy for users.
- Running workloads that require privileged kernel modules or direct hardware access as a default supported path.
- Becoming a general-purpose public compute marketplace.

## What to do when someone asks for a non-goal
- If it is a real customer requirement, propose a new ADR or a product milestone.
- If it conflicts with locked decisions (microVM boundary, IPv6-first, L4-first), it needs an ADR-level discussion, not an implementation shortcut.

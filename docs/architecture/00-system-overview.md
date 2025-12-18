# docs/architecture/00-system-overview.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
This document is the top-level narrative of how the platform works. It is intentionally high level. It explains components, responsibilities, and the main end-to-end flows.

Authoritative contracts live in `docs/specs/**`. Irreversible choices live in `docs/ADRs/**`.

## What the platform is
A developer-focused PaaS where users deploy **OCI images** plus a small **platform manifest**. The primary user surface is a **CLI**. The platform runs workloads as **microVM instances** on shared hosts and provides **L4-first ingress**.

## Why it exists
- Give individual developers and small teams a reliable way to run services without inheriting Kubernetes operational complexity.
- Provide strong isolation by default (microVM boundary).
- Provide a networking story that is IPv6-native and still supports raw TCP.

## What it is not
See `docs/NONGOALS.md`. In short:
- Not Kubernetes.
- Not a generic VM hosting panel.
- Not serverless.
- Not L7-first by default.
- Not distributed shared storage by default.

## Locked decisions that shape the architecture
See `docs/DECISIONS-LOCKED.md` and ADRs:
- MicroVM isolation per instance: `docs/ADRs/0001-isolation-microvm-per-instance.md`
- OCI image + manifest: `docs/ADRs/0002-artifact-oci-image-plus-manifest.md`
- Firecracker runtime: `docs/ADRs/0003-runtime-firecracker.md`
- WireGuard full mesh overlay (v1): `docs/ADRs/0004-overlay-wireguard-full-mesh.md`
- Event log + materialized views: `docs/ADRs/0005-state-event-log-plus-materialized-views.md`
- Control plane DB is Postgres: `docs/ADRs/0006-control-plane-db-postgres.md`
- IPv6-first, IPv4 is paid add-on: `docs/ADRs/0007-network-ipv6-first-ipv4-paid.md`
- Ingress is L4-first, SNI passthrough default: `docs/ADRs/0008-ingress-l4-sni-passthrough-first.md`
- PROXY protocol v2 is supported (opt-in): `docs/ADRs/0009-proxy-protocol-v2-client-ip.md`
- Secrets delivered as a fixed-format file: `docs/ADRs/0010-secrets-delivery-file-format.md`
- Storage is local volumes + async backups: `docs/ADRs/0011-storage-local-volumes-async-backups.md`
- CPU is soft, memory is hard-capped: `docs/ADRs/0012-scheduling-cpu-oversubscribe-mem-hardcap.md`

## Core concepts
These definitions must match `docs/GLOSSARY.md`.

- Org (tenant): top-level ownership boundary.
- App: a named service inside an org.
- Environment (env): deploy target like `prod`, `staging`. Env scopes config, secrets, routes, and releases.
- Release: immutable tuple of image digest plus manifest hash.
- Process type: named entrypoint within an env, like `web`, `worker`.
- Instance: one running replica of a process type. One instance equals one microVM.
- Route: hostname and listener binding that maps to an env and process type.

Default stance:
- One process type per microVM instance. Multiple process types can exist per env, but they scale independently into separate microVM sets.

## System components
### CLI (primary product surface)
Responsibilities:
- Auth and context selection (org, app, env).
- Manifest validation and normalization.
- Deploy flow (push image, create release, promote).
- Operability commands (logs, exec, status, rollback, scale).
- Secrets management (create/update, render local file format).
- Route management (create, list, bind hostname, enable PROXY v2).
- Volume management (create, attach, snapshot, restore).

Key requirement:
- CLI behavior must be scriptable and stable (exit codes, errors, idempotency).

### Control plane
The control plane is the source of truth for desired state.

Subcomponents:
- HTTP API and auth subsystem
- Command handlers that validate and append events
- Event log storage in Postgres
- Projection workers that maintain materialized views
- Scheduler and reconcilers that create desired allocations
- Change stream delivery to agents and edge

The control plane does not directly run workloads. It declares desired state and orchestrates convergence.

### Event log and materialized views
- All meaningful state transitions are recorded as immutable events.
- “Current state” is served from materialized views (projection outputs).
- Projections are replayable and idempotent.

This model is the backbone for auditability and debugging.

### Scheduler and reconcilers
Responsibilities:
- Compute placements for `(env, process_type)` into instances on hosts.
- Enforce resource constraints:
  - CPU is oversubscribable (soft).
  - Memory is a hard cap and must not be oversubscribed.
- Respect locality constraints for volumes (local volumes).
- Emit allocation events that drive agents to converge.

### Host agent (node agent)
A daemon on every host that converges actual state to desired state.

Responsibilities:
- Node enrollment and secure identity (control plane-issued membership).
- OCI image fetch, verification, and caching.
- Build or acquire per-release root filesystem artifacts for Firecracker.
- Firecracker lifecycle management (create, start, stop, garbage collect).
- Apply cgroup limits and jailer restrictions.
- Setup VM networking and attach to overlay.
- Mount local volumes into microVMs.
- Inject secrets file into microVMs.
- Stream logs and health back to control plane.

The agent must be idempotent and restart-safe.

### Firecracker microVM (workload runtime)
Each instance runs inside a dedicated Firecracker microVM.

Key properties:
- Boots a platform-selected kernel.
- Runs a minimal platform init (PID 1) that:
  - mounts required pseudo-filesystems
  - mounts secrets and volumes
  - starts the workload entrypoint for the process type
  - handles signals and exit status

The guest is not a general-purpose VM product in v1. Users do not supply kernels.

### Edge ingress (L4-first)
The edge accepts inbound connections and routes them to the correct backend instance.

Default routing:
- TLS passthrough with SNI inspection for routing.
- No TLS termination by default.
- Routing decisions are per-connection.

Raw TCP:
- First-class support via explicit port allocation.
- Dedicated IPv4 is a paid add-on for users who require IPv4 reachability.

Client IP propagation:
- PROXY protocol v2 is supported and opt-in per Route.
- If enabled, edge prepends PROXY v2 header to upstream connection.

### Networking overlay (WireGuard full mesh)
- Nodes are connected by a WireGuard overlay.
- IPv6 is the default addressing for hosts and workloads.
- Control plane allocates overlay addresses (IPAM).

The overlay is for platform internal connectivity, not a customer VPN product.

### Secrets subsystem
- Secrets are scoped to `(org, app, env)`.
- Delivery to workloads is a mounted file in a fixed platform format.
- Rotation default is restart semantics (rotation triggers rollout restart), not hot reload.

### Storage subsystem
- Persistent volumes are local to a host.
- Scheduler respects volume locality.
- Backups are asynchronous, out-of-band, and restorable.
- Restores create new volumes, not in-place overwrite.

### Observability
Baseline requirements:
- Logs: per instance, streamable via CLI.
- Metrics: host agent and edge emit platform metrics.
- Tracing: control plane internal tracing is enabled; workload tracing is optional.

The platform must emit enough signals to debug:
- “what is desired”
- “what is running”
- “why is it not converging”

### Web terminal frontend (optional surface)
There may be a web terminal UI (libghostty-vt or other) used for:
- onboarding demos
- remote exec sessions
- curated terminal experiences

It is not required for the control plane to function and must not dictate control plane contracts.

## End-to-end flows
### 1) User deploy
1. User runs `platform deploy`.
2. CLI validates manifest, pushes OCI image, pins digest.
3. Control plane appends release events.
4. Scheduler creates desired instances for the target process types.
5. Agents pull image, boot microVMs, report readiness.
6. Edge receives updated routing desired state and starts routing traffic.

### 2) Route creation and propagation
1. User creates a Route binding (hostname + listener port) to `(env, process_type, backend_port)`.
2. Control plane validates ownership and conflicts.
3. Control plane emits route events.
4. Edge consumes route events, applies L4 config (SNI routing rules).
5. Health checks determine whether route is active.

### 3) Logs
1. Agent captures workload output (console or defined log channel).
2. Agent streams logs to control plane or a platform log sink.
3. CLI tails logs by `(env, process_type, instance)`.

### 4) Exec
1. CLI requests exec for a specific instance.
2. Control plane authorizes and issues a short-lived exec grant.
3. Agent opens an exec channel into the microVM (implementation detail defined in runtime specs).
4. CLI attaches to the session.

### 5) Secrets update
1. User updates secret bundle for an env.
2. Control plane emits secret version event.
3. Control plane marks affected workloads as requiring restart.
4. Scheduler rolls instances (same release digest, new secrets version).
5. New instances boot with updated secrets file.

### 6) Volume attach
1. User creates a volume and attaches it to an env/process type.
2. Scheduler enforces locality and placement constraints.
3. Agent attaches block device and mounts inside microVM at a known mount point.
4. Backup pipeline snapshots volume asynchronously.

## Trust boundaries (high level)
- The control plane is trusted and holds tenant state, including encrypted secrets.
- The host agent is trusted infrastructure software.
- Workloads are treated as untrusted tenant code.
- The edge is trusted to enforce routing rules and optional PROXY v2 injection.

## Versioning and compatibility
- Manifest has a schema version and strict validation. Unknown fields are rejected by default.
- Events are versioned. We do not rewrite event history.
- Projections are rebuildable from the event log.

## Next documents
- `docs/architecture/01-control-plane.md`
- `docs/architecture/02-data-plane-host-agent.md`
- `docs/architecture/03-edge-ingress-egress.md`
- `docs/architecture/04-state-model-and-reconciliation.md`

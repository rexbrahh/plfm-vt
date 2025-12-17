# docs/DECISIONS_LOCKED.md

Status: reviewed  
Owner: TBD  
Last reviewed: 2025-12-16

This is the current list of hard decisions. The purpose is to stop re-litigating fundamentals.

If you want to change one of these, write a new ADR that supersedes the old one and update this file.

## Core architecture decisions (ADRs)

### Isolation and runtime
- **MicroVM is the isolation boundary, scoped per environment.**  
  ADR: `docs/adr/0001-isolation-microvm-per-env.md`

- **Runtime is Firecracker.**  
  ADR: `docs/adr/0003-runtime-firecracker.md`

### Artifact and deployment contract
- **Release artifact is OCI image (digest-pinned) plus a platform manifest.**  
  ADR: `docs/adr/0002-artifact-oci-image-plus-manifest.md`

### Control plane state model
- **Source of truth is an append-only event log plus materialized views.**  
  ADR: `docs/adr/0005-state-event-log-plus-materialized-views.md`

- **Control plane database is Postgres.**  
  ADR: `docs/adr/0006-control-plane-db-postgres.md`

### Networking
- **IPv6-first internally and externally.**  
  ADR: `docs/adr/0007-network-ipv6-first-ipv4-paid.md`

- **IPv4 is a paid add-on (dedicated allocation), especially for raw TCP reachability.**  
  ADR: `docs/adr/0007-network-ipv6-first-ipv4-paid.md`

- **Overlay network is WireGuard full mesh in v1.**  
  ADR: `docs/adr/0004-overlay-wireguard-full-mesh.md`

- **Ingress is L4-first with SNI passthrough by default. L7 is optional and kept separate.**  
  ADR: `docs/adr/0008-ingress-l4-sni-passthrough-first.md`

- **Client source identity propagation uses PROXY Protocol v2 (opt-in per route).**  
  ADR: `docs/adr/0009-proxy-protocol-v2-client-ip.md`

### Secrets
- **Secrets are delivered as a platform-managed file with a fixed format (and CLI renders the same format).**  
  ADR: `docs/adr/0010-secrets-delivery-file-format.md`

### Storage
- **Persistent storage is local volumes with asynchronous backups.**  
  ADR: `docs/adr/0011-storage-local-volumes-async-backups.md`

### Scheduling and resource model
- **CPU is a soft resource (oversubscribable). Memory is a hard cap.**  
  ADR: `docs/adr/0012-scheduling-cpu-oversubscribe-mem-hardcap.md`

## Default stances resolved during ADR discussions
These are the current defaults that should be reflected in specs unless a future ADR changes them.

- **One process type per microVM instance.** An environment can have multiple process types, but each instance runs exactly one entrypoint.  
  Source: ADR 0001 open-questions resolution

- **Manifest authoring format: TOML.** (CLI may also emit normalized JSON for tooling.)  
  Source: ADR 0002 open-questions resolution

- **Multi-arch images:** accept OCI indexes, but record and execute resolved per-arch digests deterministically.  
  Source: ADR 0002 open-questions resolution

- **WireGuard MTU:** standardize on a conservative MTU (example 1420) and do not break ICMPv6 Packet Too Big.  
  Source: ADR 0004 open-questions resolution

- **IPv4 allocation unit:** dedicated IPv4 is allocated per environment in v1.  
  Source: ADR 0007 open-questions resolution

- **Ingress routing model:** `Route` is a first-class control plane object binding hostname and listener port to an environment and process type.  
  Source: ADR 0008 open-questions resolution

- **Secrets rotation:** default semantics are “rotate triggers rollout restart”, not hot reload.  
  Source: ADR 0010 open-questions resolution

- **Volume snapshots:** prefer block-level snapshots (example: LVM thin snapshots) with encrypted async backups to object storage.  
  Source: ADR 0011 open-questions resolution

- **CPU UX:** expose CPU as a soft request (vCPU fractions) mapped to cgroup weights; memory is a single required hard cap.  
  Source: ADR 0012 open-questions resolution

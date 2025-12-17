# Threat model

Last updated: 2025-12-17

This document describes the attacker model, assets, trust boundaries, primary attack surfaces, and the defensive posture for the libghostty-vt PaaS.

## Security goals

- Prevent tenant to tenant data access and influence.
- Prevent tenant to platform (host and control plane) compromise.
- Keep customer secrets confidential at rest, in transit, and at runtime.
- Ensure all state mutations are authenticated, authorized, and auditable.
- Make supply chain compromises materially harder and easier to detect.
- Degrade safely under abuse (rate limits, isolation, blast radius control).

## Non goals

- Defending a customer workload against vulnerabilities inside their own app code beyond basic isolation and platform guardrails.
- Absolute protection against hardware side channels. We reduce risk and blast radius, and we patch quickly.

## System overview

High level components and typical trust boundaries.

- Customer devices
  - `ghosttyctl` (primary interface)
  - Web console and web terminal (libghostty-vt via WASM)
- Edge
  - L4 ingress and routing (IPv6 default, paid dedicated IPv4 add-on)
  - Optional Proxy Protocol v2 support
- Control plane
  - Authn and authz
  - API for resources: org, project, app, env, release, workload/instances, endpoint, volume, secret bundle, events/logs
  - Scheduler and reconciliation loop
  - Materialized views for desired vs current state
- Host plane
  - Host agent that launches and supervises Firecracker microVMs
  - Network overlay (WireGuard based) and local packet filtering
  - Image fetch and cache
  - Volume attach and mount
- Data stores
  - Control plane state store
  - Audit log store
  - Artifact and image registry (internal and external registries)

## Assets

Protect these first.

- Customer secrets (API keys, database credentials, TLS private keys).
- Customer data inside volumes, snapshots, backups, and logs.
- Customer control over releases and runtime configuration.
- Platform credentials and keys (KMS roots, signing keys, database credentials).
- Host integrity (kernel, Firecracker process, host agent).
- Control plane integrity (authz decisions, scheduler, reconciliation).
- Audit logs (forensics value).
- Network routing and endpoint mappings (availability and integrity).

## Threat actors

- External attacker with no legitimate credentials.
- Attacker with stolen customer credentials (API token, browser session).
- Malicious tenant with legitimate access attempting lateral movement or escape.
- Insider or compromised CI system pushing malicious changes.
- Supply chain attacker compromising dependencies, base images, or registries.
- Network attacker between customer and edge, or edge and control plane (MITM attempts).
- Abuse oriented attackers (DDoS, resource exhaustion, fraud).

## Trust boundaries

Primary boundaries we treat as hostile crossings.

1. Internet boundary: customer device to edge and API.
2. Identity boundary: authentication to authorization decision.
3. Control plane boundary: API and scheduler to host agents.
4. Host boundary: host OS and microVM boundary.
5. Tenant boundary: one microVM and its attached resources vs another tenant.
6. Logging boundary: sensitive runtime data to log pipelines.

## Primary attack surfaces

### Customer interfaces

- CLI auth flows and token storage.
- Web console authentication, session handling, and XSS risk.
- Web terminal (WASM) sandboxing and origin isolation.
- API endpoints for resource management, logs, exec, and secrets.

### Edge and networking

- L4 ingress and routing configuration integrity.
- Protocol confusion and Proxy Protocol v2 abuse if enabled.
- Volumetric and state exhaustion attacks (SYN floods, connection storms).
- Tenant spoofing attempts (source IP and identity metadata).

### Control plane

- Authz bypass and confused deputy (cross org, cross env).
- Unsafe defaults in manifest and release creation.
- Eventual consistency gaps: stale reads used for security decisions.
- Internal service to service auth (mTLS and/or signed tokens).
- Deserialization bugs, request smuggling, and SSRF from control plane services.

### Host plane

- MicroVM escape attempts (Firecracker, kernel, device emulation).
- Host agent API misuse and privilege escalation.
- Image cache poisoning and path traversal.
- Volume mount and filesystem boundary bugs.
- Overlay networking bugs leading to tenant spoofing or cross tenant access.

### Supply chain

- Untrusted OCI images and tags (mutable tags like `latest`).
- Dependency typosquatting and compromised upstream packages.
- Compromised build pipeline producing malicious artifacts.
- Unsigned or unverifiable CLI binaries or update channels.

## Threat analysis (STRIDE style)

### Spoofing

- Stolen API tokens used to impersonate users.
- Tenant spoofing in overlay networks.
- Proxy Protocol header spoofing to fake source IP.

Controls:
- Short lived tokens, refresh tokens, and device binding where feasible.
- Strict per hop identity propagation, never trust caller supplied identity fields.
- Proxy Protocol only from allowlisted trusted L4 proxies and on dedicated listener ports.
- Mutual authentication for control plane to host plane communications.

### Tampering

- API request tampering and replay.
- Tampering with desired state in control plane store.
- Tampering with images in cache or registry.

Controls:
- TLS everywhere and request ids with replay detection on sensitive endpoints.
- Strong authz at every mutation point, plus audit logs.
- Image verification by digest and signature, and cache keyed by digest.
- Immutable release artifacts where possible.

### Repudiation

- Users deny sensitive actions like secret changes or endpoint updates.

Controls:
- Audit logs that record actor, scope, request id, resource, and decision.
- Tamper evidence for audit logs (hash chaining, WORM storage options).

### Information disclosure

- Secrets leaked via logs, crash dumps, or debug endpoints.
- Cross tenant volume read or snapshot exposure.
- Misconfigured endpoints exposing internal services.

Controls:
- Secret handling rules (no secret values in logs, strict redaction).
- Encryption at rest for secret bundles, volumes, snapshots, backups.
- Strong tenant isolation and network policy defaults deny.
- Secure defaults and explicit opt in for exposure (public endpoints).

### Denial of service

- L4 floods, connection storms, and abusive workloads.
- Control plane rate exhaustion or database contention.
- Host resource exhaustion (CPU, memory, disk, inode).

Controls:
- Edge rate limiting and connection quotas per tenant and per endpoint.
- Control plane API quotas per org and per token.
- Host cgroups, VM limits, and per tenant fairness.
- Backpressure and circuit breakers in internal services.

### Elevation of privilege

- Authz bypass leading to admin capabilities.
- Host agent compromise leading to fleet takeover.
- MicroVM escape leading to host compromise.

Controls:
- Default deny policy model, explicit scopes, and defense in depth checks.
- Minimal host OS, aggressive patching, and hardening (seccomp, no unnecessary capabilities).
- Separation of duties: host agents cannot grant themselves new permissions.
- Continuous monitoring for privilege escalation signals.

## Top attack paths to prioritize

1. Authz confusion between org, project, app, and env boundaries.
2. Web console XSS stealing session tokens or API tokens.
3. Proxy Protocol v2 misuse leading to spoofed source IP and policy bypass.
4. Image supply chain compromise and running a malicious image.
5. Secret leakage through logs, crash dumps, or debug tooling.
6. Host agent RCE leading to host compromise.
7. MicroVM escape using kernel or Firecracker vulnerabilities.
8. Volume snapshot misbinding (wrong tenant, wrong env).
9. Eventual consistency race leading to stale authz decisions.
10. DDoS and abuse causing degraded service or cross tenant impact.

## Security requirements

Minimum required properties for v1.

- Every API mutation is authenticated and authorized, with audit logs.
- Resource identifiers are stable and authorization uses immutable ids, not names.
- Secrets are never returned after creation, and never logged.
- Deploy only by digest, with optional signature enforcement.
- MicroVMs run with strict resource limits and no host level privileges.
- Network policy defaults deny lateral traffic between tenants.
- All internal service calls are authenticated and authorized.

## Validation plan

- Unit and property tests for authz policy evaluation.
- Integration tests for cross tenant isolation (network, volumes, logs).
- Fuzzing of edge protocol parsing and API request parsing.
- Red team style exercises focused on authz bypass, secret exfil, and host agent compromise.
- Continuous dependency scanning and patch validation in staging.

## Residual risks and roadmap

- Hardware and microarchitectural side channels: mitigate with patching, scheduling strategies, and blast radius control.
- Zero day escapes: mitigate with layered controls, detection, and rapid rollback.
- Customer misconfiguration: mitigate with secure defaults, warnings, and preflight checks in CLI.

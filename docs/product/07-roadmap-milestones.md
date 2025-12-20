# Roadmap milestones

This roadmap is organized around shipping a coherent, usable product at each stage. The goal is to avoid half-features that create support load.

Dates are intentionally omitted here. Milestones are capability-based.

## Milestone A: Developer preview (single region)

### Product
- CLI onboarding flow
- Create org, project, app, env
- Deploy OCI image + manifest
- Basic endpoint creation (IPv6)
- Tail logs and events
- Exec into instances
- Rollback to prior release
- Basic volumes

### UX
- Terminal-first experience
- Web console terminal (libghostty-vt via WASM) that can run the same CLI
- Clear receipts and status views

## Milestone B: v1 (production-ready core)

### Reliability and operability
- Strong desired vs current state introspection
- Stable IDs and machine-readable output across commands
- Quotas and rate limits with transparent errors
- Log retention and event retention policies

### Networking
- Endpoint management hardened
- IPv4 add-on available and billable
- Proxy Protocol v2 option (where required)

### Product
- Templates and examples for common stacks
- Clear documentation for “what v1 is” and “what is deferred”

## Milestone C: v1.1 (developer quality of life)

- Better diff tooling (what will change on deploy)
- More introspection commands (placement, volume attach constraints)
- Improved log querying (time ranges, filters)
- Safer secret management workflows (rotate, preview, validate)

## Milestone D: L7 expansion (optional, post-v1)

Only if it can be delivered without compromising L4 clarity.

- Hostname-based routing
- Managed TLS termination
- HTTP-focused developer ergonomics (zero-downtime restarts, health probes)

## Milestone E: Multi-region (explicit, not magical)

- Multi-region deploy with explicit user choice
- Region-aware volumes (and clear constraints)
- Failover story documented and visible

## Milestone F: Managed services (selective)

- Start with one managed data service that is operationally justifiable
- Keep it optional, do not turn the platform into an add-ons marketplace first

## Milestone G: Enterprise readiness (only after product fit)

- Advanced RBAC and audit exports
- SSO and org policy controls
- Compliance workflows

## Roadmap guardrails

- Every new feature must have:
  - CLI surface area
  - introspection surfaces (describe, events, logs where relevant)
  - a clear billing story (if it costs us money)
- If a feature cannot be debugged, it is not ready to ship.

## Milestone mapping to engineering plan

This table maps product milestones (capability-based) to engineering milestones (implementation-based). See `docs/engineering/implementation-plan.md` for detailed engineering scope.

| Product Milestone | Engineering Milestones | Summary |
|-------------------|------------------------|---------|
| **A: Developer preview** | M0 + M1 + M2 + M3 + M4 + M5 + M6 + M7 + M8 | Full end-to-end platform for single region |
| **B: v1** | Post-M8 hardening + quotas + IPv4 add-on fulfillment | Production-ready core with billing |
| **C: v1.1** | Incremental DX improvements | Diff tooling, log querying, secret workflows |
| **D: L7 expansion** | New ingress capabilities | Managed TLS, HTTP routing (post-v1) |
| **E: Multi-region** | Region-aware scheduler + volumes | Explicit multi-region deploy |
| **F: Managed services** | New service types | Selective managed data services |
| **G: Enterprise** | RBAC + SSO + compliance | Enterprise readiness (post product-market fit) |

### Engineering milestones detail (for Milestone A)

| Eng Milestone | Owner | Key Deliverables |
|---------------|-------|------------------|
| M0: Repo foundation | Team DX | `just` targets, CI, schema validation |
| M1: Control-plane core | Team Control | Org/project/app/env resources, event model |
| M2: Manifest workflow | Team Control + DX | Manifest v1 schema, `vt deploy` with receipts |
| M3: Node agent runtime | Team Runtime | VM lifecycle, image fetch, workload start/stop |
| M4: L4 ingress IPv6 | Team Edge | Endpoint resource, L4 proxy, IPv6 default |
| M5: Secrets E2E | Team Control + Runtime | Encrypted storage, file delivery, CLI gate |
| M6: Releases + rollout | Team Control | Release resource, rollout tracking, wait semantics |
| M7: Observability | Shared | Event tailing, log streaming, metrics hooks |
| M8: Frontend | Team Frontend | Console, web terminal, log streaming UX |

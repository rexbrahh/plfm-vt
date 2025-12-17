Here is a doc set that, if you actually keep it updated, will let you work on this platform for months without losing the plot. It is organized as a repo-friendly file tree where each area gets its own file, and specs stay separate from narrative docs.

You can drop this under `docs/` in the main repo, or keep it as a standalone `platform-docs` repo.

---

## docs index and operating rules

- `docs/README.md`  
    How to navigate docs, what is authoritative, how to update, doc maturity levels.
    
- `docs/INDEX.md`  
    Link map to every doc, plus “owner” and “last reviewed” fields.
    
- `docs/GLOSSARY.md`  
    Terms: app, environment, workload, host, edge, overlay, port allocation, event log, reconciler, etc.
    
- `docs/NONGOALS.md`  
    Explicit “we are not doing X in v1” to prevent scope creep.
    
- `docs/DECISIONS_LOCKED.md`  
    Current hard decisions (microVM, OCI-only v1, Firecracker, WireGuard mesh, event log + Postgres views, IPv6-first, L4 SNI passthrough, secret files, local volumes + async backups, CPU soft, memory hard).
    

---

## ADRs (Architecture Decision Records)

Put every irreversible choice here so you do not re-litigate it later.

- `docs/adr/0001-isolation-microvm-per-env.md`
    
- `docs/adr/0002-artifact-oci-image-plus-manifest.md`
    
- `docs/adr/0003-runtime-firecracker.md`
    
- `docs/adr/0004-overlay-wireguard-full-mesh.md`
    
- `docs/adr/0005-state-event-log-plus-materialized-views.md`
    
- `docs/adr/0006-control-plane-db-postgres.md`
    
- `docs/adr/0007-network-ipv6-first-ipv4-paid.md`
    
- `docs/adr/0008-ingress-l4-sni-passthrough-first.md`
    
- `docs/adr/0009-proxy-protocol-v2-client-ip.md`
    
- `docs/adr/0010-secrets-delivery-file-format.md`
    
- `docs/adr/0011-storage-local-volumes-async-backups.md`
    
- `docs/adr/0012-scheduling-cpu-oversubscribe-mem-hardcap.md`
    

---

## Product and UX

- `docs/product/01-problem-statement.md`
    
- `docs/product/02-target-users-and-use-cases.md`
    
- `docs/product/03-core-user-flows.md`  
    Onboarding, deploy, logs, exec, rollback, scale, expose ports, attach volumes.
    
- `docs/product/04-surface-area-v1.md`  
    What exists in v1, what is explicitly deferred.
    
- `docs/product/05-service-tiers-and-limits.md`  
    Quotas, fair use, free vs paid, IPv4 add-on policy.
    
- `docs/product/06-pricing-and-billing-model.md`
    
- `docs/product/07-roadmap-milestones.md`
    

---

## System architecture (narrative)

- `docs/architecture/00-system-overview.md`  
    Control plane, data plane, edge, CLI, web terminal.
    
- `docs/architecture/01-control-plane.md`
    
- `docs/architecture/02-data-plane-host-agent.md`
    
- `docs/architecture/03-edge-ingress-egress.md`
    
- `docs/architecture/04-state-model-and-reconciliation.md`
    
- `docs/architecture/05-multi-tenancy-and-identity.md`
    
- `docs/architecture/06-failure-model-and-degraded-modes.md`
    
- `docs/architecture/07-scaling-plan-multi-host.md`
    
- `docs/architecture/08-security-architecture.md`
    
- `docs/architecture/09-observability-architecture.md`
    

Add diagrams alongside:

- `docs/diagrams/system-context.svg`
    
- `docs/diagrams/component-architecture.svg`
    
- `docs/diagrams/state-flow-events.svg`
    
- `docs/diagrams/network-overlay-ingress.svg`
    

---

## Core specs (authoritative)

### Manifest and workload contract

- `docs/specs/manifest/manifest-schema.md`  
    `<platform>.toml` schema, validation rules, defaults.
    
- `docs/specs/workload-spec.md`  
    The scheduler-to-host-agent contract. Fields, invariants, compatibility rules.
    

### Control plane API

- `docs/specs/api/auth.md`  
    Tokens, device flow, expiry, scopes.
    
- `docs/specs/api/http-api.md`  
    Endpoint list, request/response, pagination, errors.
    
- `docs/specs/api/openapi.yaml`  
    Source of truth for clients.
    

### State and events

- `docs/specs/state/event-log.md`  
    Ordering, idempotency keys, retention, replay rules.
    
- `docs/specs/state/event-types.md`  
    Full event catalog.
    
- `docs/specs/state/materialized-views.md`  
    What views exist, how they rebuild, migration rules.
    

### Runtime (Firecracker)

- `docs/specs/runtime/firecracker-boot.md`  
    Kernel, rootfs strategy, init, vsock usage.
    
- `docs/specs/runtime/image-fetch-and-cache.md`
    
- `docs/specs/runtime/networking-inside-vm.md`
    
- `docs/specs/runtime/volume-mounts.md`
    
- `docs/specs/runtime/limits-and-isolation.md`  
    cgroups, seccomp, jailer config, filesystem constraints.
    

### Networking

- `docs/specs/networking/overlay-wireguard.md`  
    Peer config, allowed IPs, key rotation, control-plane distribution.
    
- `docs/specs/networking/ipam.md`  
    IPv6 allocation strategy for hosts and workloads.
    
- `docs/specs/networking/ingress-l4.md`  
    TCP routing rules, SNI sniffing constraints, port allocation, health checks.
    
- `docs/specs/networking/proxy-protocol-v2.md`
    
- `docs/specs/networking/ingress-l7.md`  
    Optional and later, keep separate so it does not contaminate v1.
    
- `docs/specs/networking/ipv4-addon.md`  
    Shared IPv4 behavior and dedicated IPv4 paid behavior.
    

### Storage

- `docs/specs/storage/volumes.md`  
    Lifecycle, attach rules, locality constraints.
    
- `docs/specs/storage/snapshots.md`
    
- `docs/specs/storage/backups.md`
    
- `docs/specs/storage/restore-and-migration.md`
    

### Scheduler

- `docs/specs/scheduler/placement.md`
    
- `docs/specs/scheduler/reconciliation-loop.md`
    
- `docs/specs/scheduler/drain-evict-reschedule.md`
    
- `docs/specs/scheduler/quotas-and-fairness.md`
    

### Secrets

- `docs/specs/secrets/format.md`  
    Exact file format (recommend JSON), versioning, permissions.
    
- `docs/specs/secrets/delivery.md`  
    Where it is mounted, when it updates, restart semantics.
    
- `docs/specs/secrets/encryption-at-rest.md`  
    Key management assumptions, rotation, audit.
    

### Observability

- `docs/specs/observability/logging.md`  
    Log format, shipping pipeline, retention.
    
- `docs/specs/observability/metrics.md`  
    Prometheus scrape targets, labels, cardinality rules.
    
- `docs/specs/observability/dashboards.md`  
    Grafana dashboards you ship by default.
    
- `docs/specs/observability/alerts.md`  
    Alert rules, paging thresholds.
    

---

## CLI and developer surface

- `docs/cli/00-principles.md`  
    CLI as the product, ergonomics, scripting guarantees.
    
- `docs/cli/01-command-map.md`
    
- `docs/cli/02-manifest-workflow.md`
    
- `docs/cli/03-auth-and-context.md`
    
- `docs/cli/04-errors-and-exit-codes.md`
    
- `docs/cli/05-templates-and-examples.md`
    
- `docs/cli/06-debug-tooling.md`  
    Introspection commands, event tailing, workload describe, secrets render.
    

---

## Web terminal frontend (libghostty-vt)

- `docs/frontend/web-terminal/00-overview.md`
    
- `docs/frontend/web-terminal/01-terminal-protocol.md`
    
- `docs/frontend/web-terminal/02-pty-bridge.md`
    
- `docs/frontend/web-terminal/03-security-model.md`
    
- `docs/frontend/web-terminal/04-performance-latency-budget.md`
    
- `docs/frontend/web-terminal/05-failure-modes.md`
    

---

## Operations and reliability (runbooks are mandatory)

- `docs/ops/00-sre-principles.md`
    
- `docs/ops/01-slos-slis.md`
    
- `docs/ops/02-capacity-planning.md`
    
- `docs/ops/03-monitoring-and-oncall.md`
    
- `docs/ops/04-incident-response.md`
    
- `docs/ops/05-postmortem-template.md`
    
- `docs/ops/06-disaster-recovery.md`
    
- `docs/ops/07-backup-restore-runbook.md`
    
- `docs/ops/runbooks/control-plane-down.md`
    
- `docs/ops/runbooks/host-degraded.md`
    
- `docs/ops/runbooks/edge-partial-outage.md`
    
- `docs/ops/runbooks/postgres-failover.md`
    
- `docs/ops/runbooks/wireguard-partition.md`
    
- `docs/ops/runbooks/firecracker-failure.md`
    
- `docs/ops/runbooks/volume-corruption.md`
    

---

## Security

- `docs/security/00-threat-model.md`
    
- `docs/security/01-tenant-isolation.md`
    
- `docs/security/02-authz-and-scopes.md`
    
- `docs/security/03-secret-handling.md`
    
- `docs/security/04-audit-logging.md`
    
- `docs/security/05-vulnerability-management.md`
    
- `docs/security/06-supply-chain-and-signing.md`  
    Image signing and verification, dependency policies.
    

---

## Implementation guides (for you and future contributors)

- `docs/engineering/repo-layout.md`
    
- `docs/engineering/dev-environment.md`
    
- `docs/engineering/build-and-release.md`
    
- `docs/engineering/coding-standards.md`
    
- `docs/engineering/testing-strategy.md`
    
- `docs/engineering/performance-testing.md`
    
- `docs/engineering/compatibility-and-versioning.md`  
    Especially for WorkloadSpec and manifest evolution.
    

---

## Business, legal, and policy (keep it minimal but present)

- `docs/policy/data-retention.md`
    
- `docs/policy/abuse-and-rate-limits.md`
    
- `docs/policy/support-policy.md`
    
- `docs/legal/terms.md`
    
- `docs/legal/privacy.md`
    

---

If you want, next I can take this list and turn it into a “docs skeleton” that includes templates for each file (headings, required sections, checklists), so you can start filling them in without reinventing structure every time.
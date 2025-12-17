# docs/INDEX.md

Status: reviewed  
Owner: TBD  
Last reviewed: 2025-12-16

This is the link map to every document in `docs/`. Each entry includes an owner and the last review date.

## Start here
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/README.md | reviewed | TBD | 2025-12-16 | How to use docs and what is authoritative |
| docs/INDEX.md | reviewed | TBD | 2025-12-16 | This file |
| docs/GLOSSARY.md | reviewed | TBD | 2025-12-16 | Shared vocabulary |
| docs/NONGOALS.md | reviewed | TBD | 2025-12-16 | What we are not building (especially in v1) |
| docs/DECISIONS_LOCKED.md | reviewed | TBD | 2025-12-16 | Current hard decisions with ADR links |

## Product and UX
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/product/01-problem-statement.md | planned | TBD | N/A | Why the platform exists |
| docs/product/02-target-users-and-use-cases.md | planned | TBD | N/A | Who it is for |
| docs/product/03-core-user-flows.md | planned | TBD | N/A | Onboarding, deploy, logs, exec, rollback, scale, ports, volumes |
| docs/product/04-surface-area-v1.md | planned | TBD | N/A | What exists in v1 vs deferred |
| docs/product/05-service-tiers-and-limits.md | planned | TBD | N/A | Quotas, fair use, free vs paid, IPv4 add-on policy |
| docs/product/06-pricing-and-billing-model.md | planned | TBD | N/A | Pricing model |
| docs/product/07-roadmap-milestones.md | planned | TBD | N/A | Milestones and sequencing |

## System architecture narrative
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/architecture/00-system-overview.md | planned | TBD | N/A | Control plane, data plane, edge, CLI, web terminal |
| docs/architecture/01-control-plane.md | planned | TBD | N/A | APIs, scheduler, state, auth |
| docs/architecture/02-data-plane-host-agent.md | planned | TBD | N/A | Node agent responsibilities, reconciliation |
| docs/architecture/03-edge-ingress-egress.md | planned | TBD | N/A | L4 ingress, egress posture |
| docs/architecture/04-state-model-and-reconciliation.md | planned | TBD | N/A | Event log, projections, reconcilers |
| docs/architecture/05-multi-tenancy-and-identity.md | planned | TBD | N/A | Org, project, app, env, scopes |
| docs/architecture/06-failure-model-and-degraded-modes.md | planned | TBD | N/A | What fails and how we degrade |
| docs/architecture/07-scaling-plan-multi-host.md | planned | TBD | N/A | Multi-node growth plan |
| docs/architecture/08-security-architecture.md | planned | TBD | N/A | Threat model mapping to components |
| docs/architecture/09-observability-architecture.md | planned | TBD | N/A | Logs, metrics, traces, alerting |

## Diagrams
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/diagrams/system-context.svg | planned | TBD | N/A | System context diagram |
| docs/diagrams/component-architecture.svg | planned | TBD | N/A | Component diagram |
| docs/diagrams/state-flow-events.svg | planned | TBD | N/A | State and events flow |
| docs/diagrams/network-overlay-ingress.svg | planned | TBD | N/A | Overlay and ingress path |

## Core specs (authoritative)
### Manifest and workload contract
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/specs/manifest/manifest-schema.md | planned | TBD | N/A | `<platform>.toml` schema, defaults, validation |
| docs/specs/workload-spec.md | planned | TBD | N/A | Scheduler to host-agent contract |

### Control plane API
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/specs/api/auth.md | planned | TBD | N/A | Tokens, device flow, expiry, scopes |
| docs/specs/api/http-api.md | planned | TBD | N/A | Endpoints, requests, errors, pagination |
| docs/specs/api/openapi.yaml | planned | TBD | N/A | Source of truth for clients |

### State and events
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/specs/state/event-log.md | planned | TBD | N/A | Ordering, idempotency keys, retention, replay rules |
| docs/specs/state/event-types.md | planned | TBD | N/A | Full event catalog |
| docs/specs/state/materialized-views.md | planned | TBD | N/A | Views, rebuild rules, migrations |

### Runtime (Firecracker)
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/specs/runtime/firecracker-boot.md | planned | TBD | N/A | Kernel, rootfs strategy, init, vsock usage |
| docs/specs/runtime/image-fetch-and-cache.md | planned | TBD | N/A | OCI pulls, caching, verification |
| docs/specs/runtime/networking-inside-vm.md | planned | TBD | N/A | Guest networking contract |
| docs/specs/runtime/volume-mounts.md | planned | TBD | N/A | Attach/mount mechanics |
| docs/specs/runtime/limits-and-isolation.md | planned | TBD | N/A | cgroups, seccomp, jailer, fs constraints |

### Networking
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/specs/networking/overlay-wireguard.md | planned | TBD | N/A | Peer config, allowed IPs, rotation, distribution |
| docs/specs/networking/ipam.md | planned | TBD | N/A | IPv6 allocation for hosts and workloads |
| docs/specs/networking/ingress-l4.md | planned | TBD | N/A | TCP routing rules, SNI constraints, health checks |
| docs/specs/networking/proxy-protocol-v2.md | planned | TBD | N/A | PROXY v2 usage and constraints |
| docs/specs/networking/ingress-l7.md | planned | TBD | N/A | Optional later, kept separate from v1 |
| docs/specs/networking/ipv4-addon.md | planned | TBD | N/A | Dedicated IPv4 paid behavior, port policy |

### Storage
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/specs/storage/volumes.md | planned | TBD | N/A | Lifecycle, attach rules, locality constraints |
| docs/specs/storage/snapshots.md | planned | TBD | N/A | Snapshot mechanics |
| docs/specs/storage/backups.md | planned | TBD | N/A | Backup pipeline, encryption, retention |
| docs/specs/storage/restore-and-migration.md | planned | TBD | N/A | Restore, migration constraints |

### Scheduler
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/specs/scheduler/placement.md | planned | TBD | N/A | Placement rules |
| docs/specs/scheduler/reconciliation-loop.md | planned | TBD | N/A | Desired vs actual convergence |
| docs/specs/scheduler/drain-evict-reschedule.md | planned | TBD | N/A | Maintenance and node loss behavior |
| docs/specs/scheduler/quotas-and-fairness.md | planned | TBD | N/A | Quotas, fairness guarantees |

### Secrets
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/specs/secrets/format.md | planned | TBD | N/A | Exact file format, versioning, permissions |
| docs/specs/secrets/delivery.md | planned | TBD | N/A | Mount location, update, restart semantics |
| docs/specs/secrets/encryption-at-rest.md | planned | TBD | N/A | Key management assumptions, rotation, audit |

### Observability
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/specs/observability/logging.md | planned | TBD | N/A | Log format, shipping, retention |
| docs/specs/observability/metrics.md | planned | TBD | N/A | Prometheus targets, labels, cardinality rules |
| docs/specs/observability/dashboards.md | planned | TBD | N/A | Grafana dashboards shipped by default |
| docs/specs/observability/alerts.md | planned | TBD | N/A | Alert rules and paging thresholds |

## CLI and developer surface
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/cli/00-principles.md | planned | TBD | N/A | CLI ergonomics and scripting guarantees |
| docs/cli/01-command-map.md | planned | TBD | N/A | Command inventory and nouns |
| docs/cli/02-manifest-workflow.md | planned | TBD | N/A | How manifests are created, validated, deployed |
| docs/cli/03-auth-and-context.md | planned | TBD | N/A | Auth, org selection, env context |
| docs/cli/04-errors-and-exit-codes.md | planned | TBD | N/A | Stable exit codes for scripting |
| docs/cli/05-templates-and-examples.md | planned | TBD | N/A | Example projects and manifests |
| docs/cli/06-debug-tooling.md | planned | TBD | N/A | Introspection, event tailing, workload describe, secrets render |

## Web terminal frontend (libghostty-vt)
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/frontend/web-terminal/00-overview.md | planned | TBD | N/A | What the web terminal is and is not |
| docs/frontend/web-terminal/01-terminal-protocol.md | planned | TBD | N/A | Session protocol and message types |
| docs/frontend/web-terminal/02-pty-bridge.md | planned | TBD | N/A | PTY bridge design |
| docs/frontend/web-terminal/03-security-model.md | planned | TBD | N/A | Threats and mitigations |
| docs/frontend/web-terminal/04-performance-latency-budget.md | planned | TBD | N/A | Perf targets and profiling |
| docs/frontend/web-terminal/05-failure-modes.md | planned | TBD | N/A | Failure handling |

## Operations and reliability (runbooks are mandatory)
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/ops/00-sre-principles.md | planned | TBD | N/A | Ops philosophy |
| docs/ops/01-slos-slis.md | planned | TBD | N/A | SLOs and SLIs |
| docs/ops/02-capacity-planning.md | planned | TBD | N/A | Capacity rules of thumb |
| docs/ops/03-monitoring-and-oncall.md | planned | TBD | N/A | On-call setup |
| docs/ops/04-incident-response.md | planned | TBD | N/A | Incident process |
| docs/ops/05-postmortem-template.md | planned | TBD | N/A | Postmortem template |
| docs/ops/06-disaster-recovery.md | planned | TBD | N/A | DR planning |
| docs/ops/07-backup-restore-runbook.md | planned | TBD | N/A | Backup and restore steps |
| docs/ops/runbooks/control-plane-down.md | planned | TBD | N/A | Runbook |
| docs/ops/runbooks/host-degraded.md | planned | TBD | N/A | Runbook |
| docs/ops/runbooks/edge-partial-outage.md | planned | TBD | N/A | Runbook |
| docs/ops/runbooks/postgres-failover.md | planned | TBD | N/A | Runbook |
| docs/ops/runbooks/wireguard-partition.md | planned | TBD | N/A | Runbook |
| docs/ops/runbooks/firecracker-failure.md | planned | TBD | N/A | Runbook |
| docs/ops/runbooks/volume-corruption.md | planned | TBD | N/A | Runbook |

## Security
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/security/00-threat-model.md | planned | TBD | N/A | Threat model |
| docs/security/01-tenant-isolation.md | planned | TBD | N/A | Isolation boundaries |
| docs/security/02-authz-and-scopes.md | planned | TBD | N/A | Authorization model |
| docs/security/03-secret-handling.md | planned | TBD | N/A | Secret lifecycle |
| docs/security/04-audit-logging.md | planned | TBD | N/A | Audit requirements |
| docs/security/05-vulnerability-management.md | planned | TBD | N/A | Patch policy |
| docs/security/06-supply-chain-and-signing.md | planned | TBD | N/A | Image signing and verification |

## Implementation guides
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/engineering/repo-layout.md | planned | TBD | N/A | Repo structure |
| docs/engineering/dev-environment.md | planned | TBD | N/A | Local dev setup |
| docs/engineering/build-and-release.md | planned | TBD | N/A | Release process |
| docs/engineering/coding-standards.md | planned | TBD | N/A | Standards |
| docs/engineering/testing-strategy.md | planned | TBD | N/A | Test levels |
| docs/engineering/performance-testing.md | planned | TBD | N/A | Benchmarks |
| docs/engineering/compatibility-and-versioning.md | planned | TBD | N/A | Manifest and WorkloadSpec evolution |

## Business, legal, policy
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/policy/data-retention.md | planned | TBD | N/A | Retention rules |
| docs/policy/abuse-and-rate-limits.md | planned | TBD | N/A | Abuse policy |
| docs/policy/support-policy.md | planned | TBD | N/A | Support expectations |
| docs/legal/terms.md | planned | TBD | N/A | Terms |

## ADRs (Architecture Decision Records)
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/adr/0001-isolation-microvm-per-env.md | planned | TBD | N/A | MicroVM isolation boundary |
| docs/adr/0002-artifact-oci-image-plus-manifest.md | planned | TBD | N/A | OCI + manifest |
| docs/adr/0003-runtime-firecracker.md | planned | TBD | N/A | Firecracker runtime |
| docs/adr/0004-overlay-wireguard-full-mesh.md | planned | TBD | N/A | WireGuard mesh |
| docs/adr/0005-state-event-log-plus-materialized-views.md | planned | TBD | N/A | Event log + projections |
| docs/adr/0006-control-plane-db-postgres.md | planned | TBD | N/A | Postgres |
| docs/adr/0007-network-ipv6-first-ipv4-paid.md | planned | TBD | N/A | IPv6-first, IPv4 paid |
| docs/adr/0008-ingress-l4-sni-passthrough-first.md | planned | TBD | N/A | L4 ingress, SNI passthrough |
| docs/adr/0009-proxy-protocol-v2-client-ip.md | planned | TBD | N/A | Client IP propagation |
| docs/adr/0010-secrets-delivery-file-format.md | planned | TBD | N/A | Secrets file delivery |
| docs/adr/0011-storage-local-volumes-async-backups.md | planned | TBD | N/A | Local volumes + async backups |
| docs/adr/0012-scheduling-cpu-oversubscribe-mem-hardcap.md | planned | TBD | N/A | CPU soft, memory hard |

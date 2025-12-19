# docs/INDEX.md

Status: reviewed  
Owner: TBD  
Last reviewed: 2025-12-17

This is the link map to every document in `docs/`. Each entry includes an owner and the last review date.

Status key:
- **reviewed**: Content complete and reviewed
- **draft**: Content exists, needs review
- **stub**: File exists with minimal content
- **planned**: File does not exist yet

## Start here
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/README.md | reviewed | TBD | 2025-12-16 | How to use docs and what is authoritative |
| docs/engineering/staff-handoff.md | draft | Platform Eng | 2025-12-18 | Staff handoff packet + 4-week execution plan |
| docs/INDEX.md | reviewed | TBD | 2025-12-17 | This file |
| docs/GLOSSARY.md | reviewed | TBD | 2025-12-16 | Shared vocabulary |
| docs/NONGOALS.md | reviewed | TBD | 2025-12-16 | What we are not building (especially in v1) |
| docs/DECISIONS-LOCKED.md | reviewed | TBD | 2025-12-17 | Current hard decisions with ADR links |
| docs/ADVERSARIAL-REVIEW.md | reviewed | TBD | 2025-12-17 | Documentation suite review and gaps |

## Product and UX
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/product/01-problem-statement.md | draft | TBD | 2025-12-16 | Why the platform exists |
| docs/product/02-target-users-and-use-cases.md | draft | TBD | 2025-12-16 | Who it is for |
| docs/product/03-core-user-flows.md | draft | TBD | 2025-12-16 | Onboarding, deploy, logs, exec, rollback, scale, ports, volumes |
| docs/product/04-surface-area-v1.md | draft | TBD | 2025-12-16 | What exists in v1 vs deferred |
| docs/product/05-service-tiers-and-limits.md | draft | TBD | 2025-12-16 | Quotas, fair use, free vs paid, IPv4 add-on policy |
| docs/product/06-pricing-and-billing-model.md | draft | TBD | 2025-12-16 | Pricing model |
| docs/product/07-roadmap-milestones.md | draft | TBD | 2025-12-16 | Milestones and sequencing |

## System architecture narrative
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/architecture/00-system-overview.md | draft | TBD | 2025-12-16 | Control plane, data plane, edge, CLI, web terminal |
| docs/architecture/01-control-plane.md | draft | TBD | 2025-12-16 | APIs, scheduler, state, auth |
| docs/architecture/02-data-plane-host-agent.md | draft | TBD | 2025-12-16 | Node agent responsibilities, reconciliation |
| docs/architecture/03-edge-ingress-egress.md | draft | TBD | 2025-12-16 | L4 ingress, egress posture |
| docs/architecture/04-state-model-and-reconciliation.md | draft | TBD | 2025-12-16 | Event log, projections, reconcilers |
| docs/architecture/05-multi-tenancy-and-identity.md | draft | TBD | 2025-12-16 | Org, project, app, env, scopes |
| docs/architecture/06-failure-model-and-degraded-modes.md | draft | TBD | 2025-12-16 | What fails and how we degrade |
| docs/architecture/07-actors-and-supervision.md | draft | TBD | 2025-12-19 | Actor model and supervision trees for reconciliation |
| docs/architecture/07-scaling-plan-multi-host.md | draft | TBD | 2025-12-16 | Multi-node growth plan |
| docs/architecture/08-security-architecture.md | draft | TBD | 2025-12-16 | Threat model mapping to components |
| docs/architecture/09-observability-architecture.md | draft | TBD | 2025-12-16 | Logs, metrics, traces, alerting |

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
| docs/specs/manifest/manifest-schema.md | draft | TBD | 2025-12-17 | `<platform>.toml` schema, defaults, validation |
| docs/specs/manifest/workload-spec.md | draft | TBD | 2025-12-16 | Scheduler to host-agent contract |
| docs/specs/manifest/v1-rejections.md | draft | TBD | 2025-12-16 | Manifest validation rejections |
| docs/specs/manifest/example-worker.toml | draft | TBD | 2025-12-16 | Example manifest |
| docs/specs/manifest/example-volume_usage.toml | draft | TBD | 2025-12-16 | Example manifest with volumes |

### Control plane API
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/specs/api/auth.md | draft | TBD | 2025-12-16 | Tokens, device flow, expiry, scopes |
| docs/specs/api/http-api.md | draft | TBD | 2025-12-16 | Endpoints, requests, errors, pagination |
| docs/specs/api/openapi.yaml | draft | TBD | 2025-12-16 | Source of truth for clients |

### State and events
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/specs/state/event-log.md | draft | TBD | 2025-12-16 | Ordering, idempotency keys, retention, replay rules |
| docs/specs/state/event-types.md | draft | TBD | 2025-12-16 | Full event catalog |
| docs/specs/state/materialized-views.md | draft | TBD | 2025-12-16 | Views, rebuild rules, migrations |

### Runtime (Firecracker)
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/specs/runtime/firecracker-boot.md | draft | TBD | 2025-12-17 | Kernel, rootfs strategy, init, vsock usage |
| docs/specs/runtime/guest-init.md | **reviewed** | TBD | 2025-12-17 | Guest init PID 1 contract and handshake protocol |
| docs/specs/runtime/guest-init-delivery.md | **reviewed** | TBD | 2025-12-17 | Initramfs delivery mechanism |
| docs/specs/runtime/exec-sessions.md | **reviewed** | TBD | 2025-12-17 | Interactive exec protocol and auth |
| docs/specs/runtime/image-fetch-and-cache.md | draft | TBD | 2025-12-17 | OCI pulls, caching, verification, limits |
| docs/specs/runtime/networking-inside-vm.md | draft | TBD | 2025-12-16 | Guest networking contract |
| docs/specs/runtime/volume-mounts.md | draft | TBD | 2025-12-16 | Attach/mount mechanics |
| docs/specs/runtime/limits-and-isolation.md | draft | TBD | 2025-12-16 | cgroups, seccomp, jailer, fs constraints |
| docs/specs/runtime/agent-actors.md | draft | TBD | 2025-12-19 | Actor message schemas and lifecycle events |

### Networking
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/specs/networking/overlay-wireguard.md | draft | TBD | 2025-12-16 | Peer config, allowed IPs, rotation, distribution |
| docs/specs/networking/node-enrollment.md | **reviewed** | TBD | 2025-12-17 | Host enrollment security ceremony |
| docs/specs/networking/ipam.md | draft | TBD | 2025-12-17 | IPv6 allocation, failure semantics |
| docs/specs/networking/ingress-l4.md | draft | TBD | 2025-12-16 | TCP routing rules, SNI constraints, health checks |
| docs/specs/networking/proxy-protocol-v2.md | draft | TBD | 2025-12-16 | PROXY v2 usage and constraints |
| docs/specs/networking/ingress-l7.md | draft | TBD | 2025-12-16 | Optional later, kept separate from v1 |
| docs/specs/networking/ipv4-addon.md | draft | TBD | 2025-12-16 | Dedicated IPv4 paid behavior, port policy |

### Storage
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/specs/storage/volumes.md | draft | TBD | 2025-12-16 | Lifecycle, attach rules, locality constraints |
| docs/specs/storage/snapshots.md | draft | TBD | 2025-12-16 | Snapshot mechanics |
| docs/specs/storage/backups.md | draft | TBD | 2025-12-16 | Backup pipeline, encryption, retention |
| docs/specs/storage/restore-and-migration.md | draft | TBD | 2025-12-16 | Restore, migration constraints |

### Scheduler
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/specs/scheduler/placement.md | draft | TBD | 2025-12-17 | Placement rules, volume constraints |
| docs/specs/scheduler/reconciliation-loop.md | draft | TBD | 2025-12-16 | Desired vs actual convergence |
| docs/specs/scheduler/drain-evict-reschedule.md | draft | TBD | 2025-12-16 | Maintenance and node loss behavior |
| docs/specs/scheduler/quotas-and-fairness.md | draft | TBD | 2025-12-16 | Quotas, fairness guarantees |

### Secrets
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/specs/secrets/format.md | draft | TBD | 2025-12-16 | Exact file format, versioning, permissions |
| docs/specs/secrets/delivery.md | draft | TBD | 2025-12-16 | Mount location, update, restart semantics |
| docs/specs/secrets/encryption-at-rest.md | draft | TBD | 2025-12-16 | Key management assumptions, rotation, audit |

### Observability
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/specs/observability/logging.md | draft | TBD | 2025-12-16 | Log format, shipping, retention |
| docs/specs/observability/metrics.md | draft | TBD | 2025-12-16 | Prometheus targets, labels, cardinality rules |
| docs/specs/observability/dashboards.md | draft | TBD | 2025-12-16 | Grafana dashboards shipped by default |
| docs/specs/observability/alerts.md | draft | TBD | 2025-12-16 | Alert rules and paging thresholds |

## CLI and developer surface
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/cli/00-principles.md | draft | TBD | 2025-12-16 | CLI ergonomics and scripting guarantees |
| docs/cli/01-command-map.md | draft | TBD | 2025-12-16 | Command inventory and nouns |
| docs/cli/02-manifest-workflow.md | draft | TBD | 2025-12-16 | How manifests are created, validated, deployed |
| docs/cli/03-auth-and-context.md | draft | TBD | 2025-12-16 | Auth, org selection, env context |
| docs/cli/04-errors-and-exit-codes.md | draft | TBD | 2025-12-16 | Stable exit codes for scripting |
| docs/cli/05-templates-and-examples.md | draft | TBD | 2025-12-16 | Example projects and manifests |
| docs/cli/06-debug-tooling.md | draft | TBD | 2025-12-16 | Introspection, event tailing, workload describe, secrets render |
| docs/cli/07-TUI-workbench-v1.md | draft | TBD | 2025-12-16 | TUI workbench design |

## Web terminal frontend
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/frontend/README.md | draft | TBD | 2025-12-16 | Frontend overview |
| docs/frontend/00-overview.md | draft | TBD | 2025-12-16 | What the web terminal is and is not |
| docs/frontend/01-terminal-protocol.md | draft | TBD | 2025-12-16 | Session protocol and message types |
| docs/frontend/02-pty-bridge.md | draft | TBD | 2025-12-16 | PTY bridge design |
| docs/frontend/03-security-model.md | draft | TBD | 2025-12-16 | Threats and mitigations |
| docs/frontend/04-performance-latency-budget.md | draft | TBD | 2025-12-16 | Perf targets and profiling |
| docs/frontend/05-failure-modes.md | draft | TBD | 2025-12-16 | Failure handling |
| docs/frontend/06-wasm-component-contract.md | draft | TBD | 2025-12-16 | WASM component interface |
| docs/frontend/07-worker-message-schema.md | draft | TBD | 2025-12-16 | Worker message schema |
| docs/frontend/08-render-loop.md | draft | TBD | 2025-12-16 | Render loop design |

## Operations and reliability
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/ops/00-sre-principles.md | draft | TBD | 2025-12-16 | Ops philosophy |
| docs/ops/01-slos-slis.md | draft | TBD | 2025-12-16 | SLOs and SLIs |
| docs/ops/02-capacity-planning.md | draft | TBD | 2025-12-16 | Capacity rules of thumb |
| docs/ops/03-monitoring-and-oncall.md | draft | TBD | 2025-12-16 | On-call setup |
| docs/ops/04-incident-response.md | draft | TBD | 2025-12-16 | Incident process |
| docs/ops/05-postmortem-template.md | draft | TBD | 2025-12-16 | Postmortem template |
| docs/ops/06-disaster-recovery.md | draft | TBD | 2025-12-16 | DR planning |
| docs/ops/07-backup-restore-runbook.md | draft | TBD | 2025-12-16 | Backup and restore steps |
| docs/ops/runbooks/control-plane-down.md | draft | TBD | 2025-12-16 | Runbook |
| docs/ops/runbooks/host-degraded.md | draft | TBD | 2025-12-16 | Runbook |
| docs/ops/runbooks/edge-partial-outage.md | draft | TBD | 2025-12-16 | Runbook |
| docs/ops/runbooks/postgres-failover.md | draft | TBD | 2025-12-16 | Runbook |
| docs/ops/runbooks/wireguard-partition.md | draft | TBD | 2025-12-16 | Runbook |
| docs/ops/runbooks/firecracker-failure.md | draft | TBD | 2025-12-16 | Runbook |
| docs/ops/runbooks/volume-corruption.md | draft | TBD | 2025-12-16 | Runbook |
| docs/ops/runbooks/secrets-key-ceremony.md | **reviewed** | TBD | 2025-12-17 | Master key operations |
| docs/ops/runbooks/volume-node-failure.md | **reviewed** | TBD | 2025-12-17 | Volume recovery runbook |

## Security
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/security/00-threat-model.md | draft | TBD | 2025-12-16 | Threat model |
| docs/security/01-tenant-isolation.md | draft | TBD | 2025-12-16 | Isolation boundaries |
| docs/security/02-authz-and-scopes.md | draft | TBD | 2025-12-16 | Authorization model |
| docs/security/03-secret-handling.md | draft | TBD | 2025-12-16 | Secret lifecycle |
| docs/security/04-audit-logging.md | draft | TBD | 2025-12-16 | Audit requirements |
| docs/security/05-vulnerability-management.md | draft | TBD | 2025-12-16 | Patch policy |
| docs/security/06-supply-chain-and-signing.md | draft | TBD | 2025-12-16 | Image signing and verification |

## Implementation guides
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/engineering/README.md | draft | TBD | 2025-12-16 | Engineering overview |
| docs/engineering/repo-layout.md | draft | TBD | 2025-12-16 | Repo structure |
| docs/engineering/dev-environment.md | draft | TBD | 2025-12-16 | Local dev setup |
| docs/engineering/build-and-release.md | draft | TBD | 2025-12-16 | Release process |
| docs/engineering/coding-standards.md | draft | TBD | 2025-12-16 | Standards |
| docs/engineering/testing-strategy.md | draft | TBD | 2025-12-16 | Test levels |
| docs/engineering/performance-testing.md | draft | TBD | 2025-12-16 | Benchmarks |
| docs/engineering/compatibility-and-versioning.md | draft | TBD | 2025-12-16 | Manifest and WorkloadSpec evolution |
| docs/engineering/implementation-plan.md | draft | TBD | 2025-12-16 | Milestone plan |
| docs/engineering/implementing-a-new-resource.md | draft | TBD | 2025-12-16 | Resource implementation checklist |
| docs/engineering/core-loop-demo.md | draft | TBD | 2025-12-19 | End-to-end core loop demo script |

## Runtime
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/runtime/host-os.md | draft | TBD | 2025-12-16 | NixOS host configuration |

## ADRs (Architecture Decision Records)
| Path | Status | Owner | Last reviewed | Notes |
|---|---|---:|---:|---|
| docs/ADRs/0001-isolation-microvm-per-instance.md | **locked** | TBD | 2025-12-17 | MicroVM isolation boundary (per instance) |
| docs/ADRs/0002-artifact-oci-image-plus-manifest.md | **locked** | TBD | 2025-12-16 | OCI + manifest |
| docs/ADRs/0003-runtime-firecracker.md | **locked** | TBD | 2025-12-16 | Firecracker runtime |
| docs/ADRs/0004-overlay-wireguard-full-mesh.md | **locked** | TBD | 2025-12-16 | WireGuard mesh |
| docs/ADRs/0005-state-event-log-plus-materialized-views.md | **locked** | TBD | 2025-12-16 | Event log + projections |
| docs/ADRs/0006-control-plane-db-postgres.md | **locked** | TBD | 2025-12-16 | Postgres |
| docs/ADRs/0007-network-ipv6-first-ipv4-paid.md | **locked** | TBD | 2025-12-16 | IPv6-first, IPv4 paid |
| docs/ADRs/0008-ingress-l4-sni-passthrough-first.md | **locked** | TBD | 2025-12-16 | L4 ingress, SNI passthrough |
| docs/ADRs/0009-proxy-protocol-v2-client-ip.md | **locked** | TBD | 2025-12-16 | Client IP propagation |
| docs/ADRs/0010-secrets-delivery-file-format.md | **locked** | TBD | 2025-12-16 | Secrets file delivery |
| docs/ADRs/0011-storage-local-volumes-async-backups.md | **locked** | TBD | 2025-12-16 | Local volumes + async backups |
| docs/ADRs/0012-scheduling-cpu-oversubscribe-mem-hardcap.md | **locked** | TBD | 2025-12-16 | CPU soft, memory hard |
| docs/ADRs/0013-nixos-as-host-os.md | **locked** | TBD | 2025-12-16 | NixOS as host OS |
| docs/ADRs/(0001-4)-open-questions-decisions.md | reviewed | TBD | 2025-12-16 | Open questions resolution |
| docs/ADRs/(0005-8)-open-questions-decisions.md | reviewed | TBD | 2025-12-16 | Open questions resolution |
| docs/ADRs/(0009-12)-open-questions-decisions.md | reviewed | TBD | 2025-12-16 | Open questions resolution |

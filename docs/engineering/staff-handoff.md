# Staff Handoff: plfm-vt (CLI-first microVM PaaS)

Status: draft  
Owner: Platform Eng (incoming Staff)  
Last reviewed: 2025-12-18

## Why this document exists
I’m going to be away for several weeks. This document is the “single packet” you can use to:
- onboard a new staff engineer into the architecture and decisions quickly
- split work across multiple teams (seniors + juniors) for ~4 weeks without needing tribal knowledge
- keep implementation aligned to the locked decisions, contracts, and quality bars

If anything in this doc conflicts with a spec or ADR, treat this doc as wrong and update it.

## TL;DR: what we are building
plfm-vt is a developer-focused PaaS where users deploy **OCI images + a small manifest**, and workloads run as **Firecracker microVMs** on bare metal. The primary product surface is a **CLI**. Networking is **IPv6-first** with a **WireGuard overlay**, and ingress is **L4-first** (TLS passthrough + SNI routing) with **PROXY protocol v2** as an opt-in for client IP preservation.

Start with:
- `docs/README.md` (how docs work)
- `docs/DECISIONS-LOCKED.md` (what we will not re-litigate)
- `docs/architecture/00-system-overview.md` (system narrative)
- `docs/engineering/implementation-plan.md` (milestones / team split)

## What is “locked” vs “negotiable”
### Locked (requires a new ADR to change)
See `docs/DECISIONS-LOCKED.md` and `docs/ADRs/*`. The most architecture-shaping locks:
- microVM is the isolation boundary (one microVM per instance)
- OCI image + manifest is the release artifact
- event log + materialized views is the state model
- Postgres is the control plane DB (v1)
- WireGuard full mesh overlay (v1)
- IPv6-first; dedicated IPv4 is an add-on
- ingress is L4-first with SNI passthrough; PROXY v2 is opt-in
- secrets delivered via a fixed-format file
- storage is local volumes + async backups
- CPU is oversubscribable (soft), memory is hard-capped
- NixOS is the host OS

### Negotiable (expected iteration)
- exact API endpoint shapes (must track OpenAPI + compatibility rules)
- internal service boundaries (monolith vs split) as long as contracts hold
- reconciliation implementation details (as long as desired/current remains explicit)
- developer ergonomics (CLI UX, receipts, output formatting) within determinism rules

## Architecture: the 1-page mental model
### Components
- **CLI (`vt`)**: primary UX; validates manifest; calls control plane; prints deterministic receipts.
- **Control plane (`services/control-plane`)**:
  - accepts commands (writes) and appends immutable **events**
  - runs **projections** to produce materialized views (current state)
  - runs **scheduler/reconcilers** to produce desired allocations
  - serves read APIs from views (and provides read-your-writes by waiting on projection checkpoints)
- **Node agent (`services/node-agent`)**:
  - consumes desired allocations for its node
  - converges actual runtime state to desired (idempotent, restart-safe)
  - manages VM lifecycle (Firecracker), networking, volumes, secrets, logs, exec plumbing
- **Ingress (`services/ingress`)**:
  - consumes route/backend desired state
  - applies L4 routing (SNI passthrough), optional PROXY v2, health-gated backends

### The canonical data flow (deploy → schedule → run)
1. CLI calls control plane (create org/app/env, create release, create deploy, set scale, etc).
2. Control plane validates + appends events in a DB transaction.
3. Projection workers update view tables from the event log.
4. Scheduler reconciler reads desired release+scale and emits instance allocation events.
5. Node agent pulls desired instances for its node and converges: “desired → running microVMs”.
6. Node agent reports instance status back (events and/or status APIs).
7. CLI surfaces state by reading from control plane views (`describe`, `instances`, `logs`, etc).

This loop must remain explicit and explainable:
- “what is desired”
- “what is running”
- “why are they different”

## Repo map (what exists today)
### Language and workspace
- Rust workspace: `Cargo.toml` at repo root.
- Primary crates:
  - libs: `libs/id`, `libs/events`, `libs/reconcile`, `libs/secrets-format`, `libs/networking`, `libs/testing`
  - services: `services/control-plane` (`plfm-control-plane`), `services/node-agent` (`plfm-node-agent`), `services/ingress` (`plfm-ingress`, currently stubby)
  - CLI: `cli/ghostctl` (crate `ghostctl`, binary name `vt`)

### Contracts
- OpenAPI: `api/openapi/openapi.yaml`
- JSON Schemas: `api/schemas/*.json` (events, manifest, workload spec, node plan)
- Authoritative specs: `docs/specs/**`

### Dev stack (local)
- Dev compose: `deploy/environments/dev/docker-compose.yml` (currently Postgres-focused)
- Task runner: `justfile` (some build/lint targets are placeholders; dev targets are real)

## Current state snapshot (as of 2025-12-18)
This is intentionally blunt so you can plan without guessing.

- Control plane: event log + projection worker + scheduler loop are wired; core resource APIs exist (org/app/env/release/deploy/node/instance), and `instances_desired_view` includes `deploy_id` for traceability.
- Node agent: reconciliation loop + instance manager exist, but full Firecracker runtime work is still gated on Linux/KVM and the remaining runtime integration tasks.
- CLI (`vt`): foundational commands exist for orgs/apps/envs/releases/deploys/nodes/instances/scale/logs; output is table-first with a `--format json` option.
- Ingress: crate exists but is still early; treat it as a workstream starter, not a finished component.
- Frontend: directory scaffolding exists, but implementation is not started (intentionally).
- Dev environment: local Postgres via docker compose, migrations run in control-plane dev mode.

## Non-negotiable quality bars (teams must enforce)
These come from `docs/engineering/coding-standards.md` and the project stance:
- **No secrets** in logs, errors, traces, or events (only secret version IDs/metadata).
- **Idempotency** for all mutations (API + CLI); safe to retry.
- **Deterministic output** for CLI: stable ordering; golden tests where possible.
- **Desired vs current** is explicit everywhere; no “hidden imperative mutations”.
- **Receipts** for mutating CLI commands: always tell users what happened and what to do next.

## Getting productive quickly (onboarding checklist)
### First 60–90 minutes: read in this order
1. `docs/README.md`
2. `docs/NONGOALS.md`
3. `docs/DECISIONS-LOCKED.md`
4. `docs/architecture/00-system-overview.md`
5. `docs/architecture/04-state-model-and-reconciliation.md`
6. `docs/specs/state/event-log.md` and `docs/specs/state/materialized-views.md`
7. `docs/specs/manifest/manifest-schema.md`
8. `docs/engineering/README.md` and `docs/engineering/implementation-plan.md`

### Local dev (macOS/Linux)
```bash
just dev-up

# in another terminal
just dev-control-plane

# in another terminal (agent is mostly logical unless you’re on Linux w/ KVM)
just dev-node-agent

# CLI
cargo run -p ghostctl -- --help
```

Notes:
- Full Firecracker runtime work requires Linux + KVM; see `docs/engineering/dev-environment.md` and `services/node-agent/README.md`.
- If you’re on macOS, focus on control-plane + CLI + scheduler/projections; use a Linux host/runner for runtime/ingress integration.

### Debugging primitives
- Database is the truth store: events and view tables (see `services/control-plane/migrations/`).
- “Why isn’t it converging?” usually reduces to:
  - missing/incorrect desired state events
  - projection not applying (checkpoint stuck)
  - scheduler not emitting allocations
  - agent not consuming/applying allocations

## 4-week execution plan (enough for multiple teams)
The plan below is designed so two teams can make progress in parallel without blocking each other.

### Workstream A: Control plane + CLI (Team Control + Team DX/CLI)
Goal: “users can do the core loop via CLI” with strong contracts and idempotency.

Week 1 (stabilize contracts + dev loop)
- Turn `just verify` into a real gate (remove placeholder commands where feasible).
- Add contract tests for:
  - OpenAPI ↔ handlers (request/response shape)
  - JSON schema fixtures for events/manifest/workload-spec
- Implement/verify read-your-writes behavior on create/deploy endpoints (projection checkpoint waits).
- Establish a “demo script” in docs (create org/app/env → release → deploy → instances → logs).

Week 2 (idempotency + receipts + “debuggability”)
- Implement idempotency keys end-to-end (API header → idempotency table → consistent response replay).
- Standardize error codes and receipts (CLI points to `events tail`, `instances list`, `deploys get`).
- Flesh out event correlation (`request_id`, `deploy_id`) and make tracing straightforward.

Week 3 (routes + edge contract, without needing full ingress)
- Control plane: finalize `Route` model in events + views (hostname/port → env/process_type/backend_port, proxy_v2 flag).
- CLI: add `routes` commands with deterministic output and receipts.
- Add a stub “edge sync” consumer that can replay route changes by cursor (even if no real TCP proxy yet).

Week 4 (secrets + volumes interface contracts)
- Control plane: implement secret bundle + version events and views (no raw secret material).
- CLI: implement secrets set/import/list with redaction rules.
- Control plane: implement volume create/attach events and views (even if runtime is stubbed).

Suggested junior-friendly tasks in this workstream:
- Add golden tests for CLI table output ordering.
- Add fixtures + schema validation tests for API payloads.
- Implement CLI `--json` output for one command at a time with stable fields.

### Workstream B: Node agent + runtime path (Team Runtime)
Goal: “desired allocations reliably become running instances” with observable state transitions.

Week 1 (runtime skeleton + local state)
- Ensure agent reconciliation loop is restart-safe (disk-backed state store, clear directory layout).
- Define a strict runtime interface: mock runtime in CI; Firecracker runtime behind a feature flag.
- Implement image cache primitives (content-addressed directories, checksum verification).

Week 2 (guest init + boot pipeline)
- Implement guest init artifact caching and selection per `docs/specs/runtime/guest-init-delivery.md`.
- Implement boot handshake per `docs/specs/runtime/guest-init.md` and `docs/specs/runtime/firecracker-boot.md`.
- Add boot latency measurement hooks (P50/P95/P99) and log-only metrics initially.

Week 3 (exec sessions + logs)
- Implement agent-side exec per `docs/specs/runtime/exec-sessions.md`:
  - vsock connect to guest exec service
  - PTY and resize control messages
  - bounded cleanup on disconnect
- Implement log streaming contract (even if transport is temporary) with strict redaction.

Week 4 (volumes + networking integration)
- Implement volume attach/mount path per `docs/specs/runtime/volume-mounts.md`.
- Implement overlay wiring stubs (WireGuard config consumption) and tap device management scaffolding.

Suggested junior-friendly tasks in this workstream:
- Build unit tests around state machine transitions (allocated → booting → running → stopped).
- Add fuzz tests for manifest/WorkloadSpec parsing in libs (no network required).
- Add structured logging fields (`request_id`, `resource_id`, `instance_id`) consistently.

### Workstream C: Ingress / Edge (Team Edge)
Goal: “edge can load route/backends and route connections safely” (even if not production-grade yet).

Week 1 (control-plane sync + config model)
- Implement event cursor consumption from control plane (polling OK first).
- Build routing table model: `(listener_addr, port, sni/hostname) -> backend set`.

Week 2 (L4 proxy MVP)
- Implement SNI parse and TCP proxy loop.
- Implement health-gated backend selection (use control-plane readiness first; avoid tight coupling).

Week 3 (PROXY v2 + IPv6-first correctness)
- Implement PROXY v2 injection (opt-in per Route).
- Add IPv6-only integration tests (no implicit IPv4 assumptions).

Week 4 (hardening + safe reload)
- Implement atomic config reload with persistence.
- Add basic metrics counters per route (connections, failures) for debuggability.

Suggested junior-friendly tasks in this workstream:
- Write SNI parser unit tests (happy paths + malformed ClientHello).
- Write PROXY v2 encoding/decoding tests (goldens).
- Add a “routing table diff” printer for debugging (deterministic ordering).

### Optional Workstream D: Frontend console + web terminal (Team Frontend)
This can start in parallel, but should not block core control-plane/runtime work.

Week 1–2
- Scaffold `frontend/console` with auth + context selection aligned to CLI semantics.
- Implement a basic “events tail” + “instances list” view using OpenAPI.

Week 3–4
- Implement a web terminal prototype that can attach to exec sessions defined in `docs/specs/runtime/exec-sessions.md`.
- Focus on security boundaries (token lifetime, origin checks, no secret leakage).

## Interfaces and “ownership boundaries” (how to avoid stepping on each other)
Use this rule: if a PR changes a contract, it must update the contract source of truth and add tests.

| Interface | Source of truth | Owners |
|---|---|---|
| Public API | `api/openapi/openapi.yaml` | Control + CLI |
| Event envelope + event types | `api/schemas/event-envelope.json`, `docs/specs/state/event-types.md` | Control |
| Manifest schema | `api/schemas/manifest.json`, `docs/specs/manifest/manifest-schema.md` | Control + CLI |
| WorkloadSpec / NodePlan | `api/schemas/workload-spec.json`, `api/schemas/node-plan.json` | Control + Runtime |
| Exec sessions | `docs/specs/runtime/exec-sessions.md` | Runtime + Control + Frontend |
| Guest init delivery | `docs/specs/runtime/guest-init-delivery.md` | Runtime |
| L4 ingress + PROXY v2 | `docs/specs/networking/ingress-l4.md`, `docs/specs/networking/proxy-protocol-v2.md` | Edge |

## “Complete prompt” for a new Staff Engineer (copy/paste)
Use this if you want to bootstrap yourself (or an AI assistant) with the full project context quickly.

> You are the incoming Staff Engineer for `plfm-vt`, a CLI-first PaaS that runs workloads as Firecracker microVMs on bare metal. Your job is to ship the v1 core loop: OCI image + manifest → release → deploy → scheduler allocates → node-agent converges → ingress routes (L4/SNI passthrough) → users observe/operate via CLI.  
>
> Non-negotiable invariants: event-sourced state (append-only log + materialized views), explicit desired vs current state, idempotent mutations, deterministic CLI output, secrets never in logs/events/errors, IPv6-first, ingress is L4 passthrough, PROXY v2 is opt-in, local volumes + async backups, CPU soft/memory hard.  
>
> First, read these docs in order and treat them as authoritative:  
> 1) `docs/README.md`  
> 2) `docs/DECISIONS-LOCKED.md` + `docs/ADRs/*`  
> 3) `docs/architecture/00-system-overview.md` and `docs/architecture/04-state-model-and-reconciliation.md`  
> 4) `docs/specs/state/event-log.md`, `docs/specs/state/materialized-views.md`  
> 5) `docs/specs/manifest/manifest-schema.md`  
> 6) `docs/engineering/implementation-plan.md`  
>
> Then, map those contracts to code in: `services/control-plane`, `services/node-agent`, `services/ingress`, `cli/ghostctl`, and workspace libs under `libs/`.  
>
> Finally, produce (a) a 4-week execution plan split across Control/Runtime/Edge/CLI teams, (b) a risk register, and (c) a set of concrete “next PRs” that improve the end-to-end demo loop without changing locked decisions.

## Appendix: “where to start writing code”
- Adding a new resource end-to-end: `docs/engineering/implementing-a-new-resource.md`
- Control plane design: `docs/architecture/01-control-plane.md`
- Host agent design: `docs/architecture/02-data-plane-host-agent.md`
- Ingress design: `docs/architecture/03-edge-ingress-egress.md`
- State model: `docs/specs/state/*`

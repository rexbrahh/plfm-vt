# Implementation plan

This plan translates completed architecture docs and ADRs into an execution framework for multiple teams.

## Guiding constraints

- CLI is the primary customer interface.
- Control-plane is reconciliation-driven with explicit desired vs current state.
- Secrets are delivered by reconciliation into a fixed file format.
- Ingress is L4 by default, IPv6-first, with dedicated IPv4 as an add-on.
- All mutating operations produce receipts and are safe to retry.

## Teams (suggested ownership model)

Team DX
- repo scaffolding, dev environment, CI, tooling, test harness

Team Control
- API, auth, org/project/app/env/release resources, reconciliation orchestration, scheduler core

Team Runtime
- node-agent, VM lifecycle, image fetch/cache, volumes, limits/isolation, secrets materialization

Team Edge
- ingress L4, IPAM integration, IPv6 defaulting, IPv4 add-on, proxy protocol v2

Team Frontend
- console, web terminal (libghostty-vt WASM), session UX, log streaming UX

Team Observability and Security (can be shared)
- event stream, logs, metrics, dashboards, alerts, threat model enforcement hooks

## Milestones (dependency-aware)

Milestone 0: Repo and build foundation (Team DX)
Deliverables:
- `just` targets: fmt/lint/test/build/dev-up
- CI: validate + test + build artifacts
- schema validation pipeline for `api/`
Definition of done:
- new contributor can run `just dev-up` and `just test` successfully

Milestone 1: Control-plane core resources (Team Control)
Scope:
- org, project, app, env resources
- auth + context model
- event model wiring
DoD:
- CLI can create org/project/app/env and list/describe them
- events emitted for create/update flows

Milestone 2: Manifest-first workflow (Team Control + Team DX + CLI owners)
Scope:
- manifest schema v1
- apply semantics (create/update)
- clear diff and preview output
DoD:
- `vt deploy` (alias: `vt apply`) produces deterministic plans and receipts
- schema validation works offline in CLI

Milestone 3: Node agent minimal runtime (Team Runtime)
Scope:
- VM lifecycle skeleton
- image fetch and cache
- basic workload start/stop
DoD:
- control-plane can request a workload
- node-agent converges and reports status
- failure modes are observable via events

Milestone 4: L4 ingress IPv6-first (Team Edge)
Scope:
- endpoint resource mapping
- L4 proxying without termination
- IPv6 default behavior
- IPv4 add-on model (even if fulfillment is stubbed initially)
DoD:
- reachable service on IPv6 end-to-end
- clear status reporting and eventing for edge actions

Milestone 5: Secrets end-to-end (Team Control + Team Runtime)
Scope:
- encrypted secret bundle storage
- delivery via reconciliation to fixed file format
- env/secret gate before release creation in CLI
DoD:
- no secrets in logs
- workload consumes reconciled secret file
- CLI enforces “set/import env or ack none” before release creation

Milestone 6: Releases and rollout semantics (Team Control + CLI owners)
Scope:
- release resource
- rollout tracking via events
- wait semantics and timeouts
DoD:
- `ghostctl release create --wait` works reliably
- receipts point to `ghostctl events tail` and `ghostctl workload describe`

Milestone 7: Observability surfaces (shared)
Scope:
- event tailing
- logs streaming
- basic metrics hooks
DoD:
- first-class introspection commands exist and are stable

Milestone 8: Frontend console + web terminal (Team Frontend)
Scope:
- console shell with auth + context
- web terminal using libghostty-vt WASM
- embedded workflows for logs, events, and “ssh-like” sessions when available
DoD:
- web terminal can run curated workflows and CLI onboarding
- log streaming UX does not require page reloads

## Cross-cutting quality bars

- Compatibility: no breaking schema changes without versioning and tests.
- Determinism: golden tests for CLI output and stable ordering.
- Security: secret redaction and least privilege from day one.
- Performance: baseline measurements established early, regressions tracked.

## Work intake and tracking (simple default)

For each milestone:
- one tracking doc with:
  - scope
  - APIs touched
  - schemas touched
  - test plan
  - rollout plan
  - compatibility notes
- each PR must link the relevant milestone section and update it if scope changes.

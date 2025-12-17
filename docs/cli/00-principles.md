# CLI principles

The CLI is the product. It is the primary interface for authentication, configuration, deployment, operations, and debugging.

This document defines the ergonomics and scripting guarantees of the CLI.

## Scope

The CLI must fully support v1 customer workflows:

- Initialize an app from a local folder with a manifest
- Authenticate and select an org, project, and environment
- Configure runtime environment and secrets
- Create immutable releases and deploy them
- Manage endpoints (L4, IPv6 default, dedicated IPv4 add-on)
- Manage volumes and snapshots
- Observe and debug via status, events, and logs

## Product stance

- The CLI is not a thin wrapper around HTTP APIs.
- The CLI is a guided workflow engine for a reconciliation-based platform.
- Defaults must be safe, explainable, and easy to automate.

## Core mental model

### Manifest first
A manifest must exist for every app. It may be minimal, but it is required.
Whatever the user declares in the manifest represents desired state.

### Releases are immutable
A release is an immutable deployment unit. Users create releases and deploy releases.

### Reconciliation is real
The control plane converges desired state to current state asynchronously.
The CLI must never pretend that convergence is always immediate.

### L4 first networking
Endpoints are L4 by default. IPv6 is the default. Dedicated IPv4 is an explicit add-on.
Proxy Protocol v2 may be enabled per endpoint.

### Secrets are delivered, not injected
The CLI records desired secret state. The system delivers secrets into the runtime as a fixed-format file via reconciliation.

## Ergonomics principles

### 1) Fast path is one command per step
The common path should be:

1. init
2. env set or env import, or env confirm --none
3. release create
4. deploy
5. status, logs tail, events tail

Every step should have a clear default and a single canonical command.

### 2) Make state visible
Every relevant command should be able to show:

- desired state
- current state
- conditions and reasons for the gap
- the object identifiers involved

Prefer explicit names and IDs over inferred behavior.

### 3) Errors must be actionable
Error output must include:

- what failed
- why it failed (if known)
- how to fix it (one concrete command)
- a trace or request id when available

Avoid “unknown error” unless there is truly no context.

### 4) Keep command names predictable
- Verbs for actions: deploy, create, set, attach, expose
- Nouns for resources: apps, releases, workloads, endpoints, volumes, secrets
- describe for deep inspection
- tail for streaming feeds

### 5) Interactive prompts are opt-in
Default behavior must be non-interactive and scriptable.
Interactive prompts are allowed only when:
- the TTY is interactive, and
- the command is not clearly in a scripting context

Provide `--yes` and `--no-input` for deterministic automation.

## Scripting guarantees

### Stability and versioning
- Command names and flags are stable within a major version.
- Machine output is stable and versioned.
- The CLI prints its own version and API compatibility info.

### Deterministic machine output
Every command must support:
- human output by default
- `--json` for structured output

Rules:
- `--json` output is valid JSON, no extra text.
- Fields are additive over time. Removing or renaming fields requires a major version bump.
- Timestamps use RFC 3339.
- Identifiers are stable strings.

### Idempotency
All mutation commands must be safe to retry.
Examples:
- `endpoints expose` re-running does not create duplicates.
- `secrets set` re-running yields the same desired state.
- `deploy` can be retried after partial failure.

When a request cannot be idempotent, the CLI must require an explicit unique token or confirm intent.

### Consistent exit codes
Exit codes are documented and consistent across commands.

Recommended categories:
- 0: success
- 1: generic failure (only when no better category fits)
- 2: invalid usage or validation error
- 3: auth failure or permission denied
- 4: not found
- 5: conflict (already exists, precondition fa

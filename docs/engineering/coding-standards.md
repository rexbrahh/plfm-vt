# Coding standards

These standards exist to keep the platform safe, debuggable, and consistent across teams.

## Global product invariants

1. Desired vs current state is explicit
- Mutations create a desired state.
- Reconciliation converges current state to desired state.
- CLI output must reflect this and never imply synchronous convergence unless it waited and confirmed.

2. Idempotency is default
- API operations should be safe to retry.
- CLI operations should be safe to re-run.
- Avoid “create then fail” patterns without receipts and stable IDs.

3. Deterministic behavior
- Stable ordering in output.
- Stable serialization formats.
- Avoid nondeterministic timestamps in golden-tested output unless explicitly marked.

4. Secrets never leak
- No secrets in logs.
- No secrets in error strings.
- No secrets in tracing attributes.
- Redaction must be enforced centrally.

## Code style

Formatting:
- Use language standard formatters (rustfmt, gofmt, prettier).
- CI must enforce formatter output.

Linting:
- Treat warnings as errors in CI where possible.
- Clippy/staticcheck/eslint should be part of `just lint`.

## Error handling

- Prefer typed errors with context.
- Avoid panics in long-running services.
- CLI exit codes must be consistent and documented (docs/cli and docs/engineering/build-and-release.md).
- Errors must include:
  - what failed
  - which resource ID
  - what the user can do next (where appropriate)

## Logging and tracing

- Structured logs only.
- Correlation IDs:
  - request ID
  - resource ID
  - reconciliation cycle ID (if applicable)
- Log levels:
  - INFO: state transitions, receipts
  - WARN: recoverable problems
  - ERROR: failed operations with actionable context
- Never log secret payloads or decrypted values.

## API and schema conventions

Resources:
- org, project, app, env, release, workload/instances, endpoint, volume, secret bundle, event/log stream

Rules:
- Stable IDs are canonical. Names are labels.
- New fields must be additive and have clear default behavior.
- Fields that may grow should be maps with constraints, not unbounded strings.

## Reconciliation conventions

- Reconciler inputs:
  - desired spec (from control plane)
  - observed state (from node agent and runtime)
- Reconciler outputs:
  - actions to take
  - events emitted
  - status updates
- Every reconcile loop must be safe to run repeatedly.

## Code review checklist (minimum)

- Does this change preserve desired vs current semantics?
- Are outputs deterministic?
- Are secrets redacted and never logged?
- Are contract tests updated if schemas changed?
- Is there a clear rollback path for behavior changes?
- Are error messages actionable and do they avoid leaking sensitive data?

# Testing strategy

This platform is reconciliation-driven and distributed. Tests must reflect that.

## Test layers

1. Unit tests
- Pure functions: parsing, validation, diffing, serialization
- Reconciler decision logic with mocked inputs

2. Component integration tests
- Control-plane: API endpoints + storage + auth (no real runtime)
- Node-agent: VM lifecycle logic with simulated runtime hooks
- Ingress: config generation and state application

3. Contract tests (required for changes to interfaces)
- OpenAPI and schema compatibility tests
- Event type validation tests
- CLI JSON output schema tests

4. End-to-end tests (E2E)
- CLI against a real stack (local dev stack or ephemeral CI cluster)
- Covers:
  - org/project/app/env flow
  - manifest apply
  - release create gate for env/secrets
  - wait and event tailing
  - logs stream

5. Failure and chaos tests (as soon as core flows exist)
- Kill node-agent during reconcile and ensure convergence after restart
- Drop overlay connectivity and confirm clear status and recovery
- Delay image fetches and confirm timeouts and events are correct

## CLI-specific testing

- Golden tests for human output (stable ordering, stable formatting).
- JSON output tests:
  - schema validation
  - no extra fields removed unexpectedly
- Exit code tests:
  - auth failures
  - not found
  - validation errors
  - transient failures

## Runtime-specific testing

- Image fetch and cache behavior:
  - cache hit and miss
  - concurrent pulls
  - corrupt image handling

- Networking:
  - IPv6-first routing scenarios
  - IPv4 add-on path correctness
  - proxy protocol v2 toggles

- Volumes:
  - mount/unmount correctness
  - snapshot/restore path (when implemented)

- Limits and isolation:
  - cpu and memory enforcement
  - filesystem boundaries

## Secrets testing

- Encryption at rest:
  - roundtrip tests for encrypt/decrypt
  - key rotation test scaffolding

- Delivery:
  - control-plane writes desired secret bundle
  - node-agent reconciles file materialization
  - workload reads from mounted file

- Redaction tests:
  - ensure secrets do not appear in logs
  - ensure errors do not include secret values

## Determinism and time

- Prefer injected clocks in services for test determinism.
- Avoid relying on wall clock and sleeps in tests.
- For reconcile loops, test using:
  - explicit step functions
  - event-driven progression

## CI gating

Minimum gates for merge:
- fmt, lint
- unit tests
- contract tests for any change touching `api/` or CLI output formats

Minimum gates for release:
- integration tests
- e2e smoke tests
- performance regression checks (when baseline exists)

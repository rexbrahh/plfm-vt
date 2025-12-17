# Compatibility and versioning

This is the policy for evolving:
- API
- CLI
- manifest schema
- WorkloadSpec
- event types
- control-plane and node-agent interoperability

This matters because customers will not upgrade everything at once.

## Versioned surfaces

1. CLI (ghostctl)
- SemVer.
- Must interoperate with a range of server versions (defined below).

2. API (OpenAPI)
- Versioned as part of server SemVer, but changes must be tracked explicitly.
- Backwards incompatible changes require a major bump.

3. Schemas
- Manifest schema
- WorkloadSpec schema
- Event schema

Each schema must have:
- explicit `schemaVersion` (or equivalent)
- compatibility rules and defaulting behavior
- reserved fields list (to avoid reuse issues)

## Compatibility window (recommended policy)

Server to CLI:
- CLI supports server versions:
  - same major
  - current minor and up to 2 previous minors (N to N-2), best effort
- Server should accept requests from CLI within that window without behavior surprises.

Control-plane to node-agent:
- Node-agent supports:
  - same major
  - current minor and previous minor (N to N-1)
- If we need a larger window, treat it as an explicit project requirement and test it.

## Schema evolution rules

Allowed without breaking old clients:
- Add optional fields with safe defaults.
- Add new enum variants only if old clients can treat unknown values safely.
- Add new resources or capabilities behind discovery (see below).

Not allowed without a major bump or migration plan:
- Removing fields that clients may send or rely on.
- Changing field meanings.
- Making optional fields required.
- Renaming fields without alias support.

## Defaulting and unknown fields

- Server must ignore unknown fields from newer clients when safe.
- Server must apply defaulting for missing fields from older clients.
- Schemas should define defaults explicitly.

## WorkloadSpec evolution

WorkloadSpec is core and long-lived.

Rules:
- WorkloadSpec must carry a version and a stable identity.
- Control-plane stores a canonical internal form and can down-convert or project views for older clients when feasible.
- Node-agent consumes a well-defined version and must reject unsupported versions with an explicit, actionable error.

Recommended pattern:
- `workloadSpecVersion: 1`
- Additive changes inside a version.
- If a breaking change is required:
  - introduce `workloadSpecVersion: 2`
  - provide conversion logic in control-plane
  - gate rollout with compatibility tests

## Manifest evolution

Manifest is a user-facing contract.

Rules:
- Include `apiVersion` or `manifestVersion`.
- Changes must be:
  - documented
  - validated by schema
  - handled with defaulting
- CLI must validate locally and produce actionable errors.

Avoid:
- “silent interpretation changes” where the same manifest produces different resources across versions.

## Capability discovery

To avoid hard coupling:
- Server exposes a capabilities endpoint:
  - supported WorkloadSpec versions
  - supported schema versions
  - supported ingress options (IPv4 add-on, proxy protocol v2)
  - supported features and flags

CLI behavior:
- If a requested feature is unsupported, error clearly:
  - what is unsupported
  - how to upgrade or what alternative exists
- Prefer warning plus degradation only when safe.

## Deprecation process

1. Announce
- docs update
- release notes
- CLI warnings if appropriate

2. Grace period
- keep old behavior working for at least one compatibility window

3. Enforce
- server rejects deprecated behavior
- CLI errors become hard failures

## Compatibility testing requirements

Any PR that changes:
- schemas in `api/`
- CLI output formats
- WorkloadSpec fields

Must add or update:
- contract tests
- compatibility fixtures
- at least one scenario using an older client fixture against a newer server behavior (or vice versa)

## Example: adding proxy protocol v2 option

- Add optional field: `endpoint.proxyProtocolV2: boolean` default false
- Server advertises capability: `proxyProtocolV2=true`
- CLI:
  - if capability absent and user sets it, return an error with next steps
  - if capability present, apply and emit a receipt plus event references

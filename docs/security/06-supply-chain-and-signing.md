# Supply chain and signing

Last updated: 2025-12-17

This document defines the supply chain security posture for platform components and customer OCI images. The goal is to prevent running untrusted artifacts and to make provenance verifiable.

## Threats

- Malicious or compromised OCI images run in production.
- Tag mutation and registry compromise (for example `latest` retagged).
- Dependency typosquatting and compromised upstream packages.
- Compromised CI runner or build system producing malicious artifacts.
- Man in the middle attacks on artifact download channels.
- CLI binary replacement or update hijack.

## Principles

- Prefer immutable references (digests) over mutable tags.
- Verify before use (signature and policy) at every boundary.
- Separate build and deploy identities.
- Keep provenance and SBOM attached to artifacts.
- Reduce trust in registries by verifying independently.

## OCI image policy

### Customer images

Recommended v1 requirements:

- Deployments resolve images to digests at release creation time.
- Store resolved digests in release records and never redeploy a tag implicitly.
- Support optional org policies:
  - require signed images
  - allowlist registries and namespaces
  - deny mutable tags
  - require provenance attestations

### Platform component images

- Platform components must be built by a trusted CI pipeline.
- Images must be signed and pinned by digest in deployments.
- Rollouts should verify signature and digest before pulling.

## Signing and verification

### Signing mechanism

Use a widely adopted container signing approach (for example Sigstore Cosign) with either:

- key based signing (project managed keys stored in KMS or HSM)
- keyless signing (OIDC based, with transparency log)

Regardless of approach:
- document trust roots and rotation procedures
- support emergency key revocation

### Verification points

Verification should happen at:

- release creation time (control plane resolves and verifies images)
- host image fetch time (host agent verifies digest and policy before caching and running)
- any artifact promotion path (staging to production)

### Verification policy

- Default: allow unsigned images but pin by digest.
- Optional org policy: require signatures and specific issuers.
- Optional org policy: require SBOM and provenance attestation.

If verification fails:
- fail the release creation or deployment action with a clear error
- log an audit event with the policy and failure reason

## Provenance and SBOM

- Produce SBOMs for platform components and publish them alongside images.
- Support attaching SBOMs and provenance to customer releases.
- Track build provenance metadata:
  - source repo and revision
  - build system identity
  - build time and builder image digest
  - dependency lockfiles

A future goal is alignment with SLSA levels for high assurance builds.

## Dependency management

### Source dependencies

- Lock dependencies with lockfiles and enforce update review.
- Use automated updates with human review for high risk dependencies.
- Pin toolchains and build images.

### Third party services

- Treat registries, CDN, and CI providers as part of the trust chain.
- Use least privilege tokens for registry access.
- Rotate credentials on schedule and after incidents.

## Artifact distribution

### CLI binaries

- Distribute CLI binaries with signature verification.
- If auto update exists, it must verify signatures and pin trusted keys.
- Provide checksums and signed metadata for offline verification.

### Web console and web terminal

- Use strict CSP and subresource integrity where feasible.
- Pin WASM and JS asset hashes and serve from a controlled origin.
- Avoid loading third party scripts that can compromise sessions.

## Image cache security

Threats:
- cache poisoning
- path traversal and symlink tricks
- disk reuse leaking data between tenants

Controls:
- cache entries keyed strictly by digest
- verify digest after download
- isolate cache directories and permissions
- wipe staging areas after use
- treat cache as untrusted input, validate paths and metadata

## Operational response

- Rapid key rotation and revocation playbooks.
- Ability to block a compromised digest globally.
- Ability for orgs to add deny rules for specific images and issuers.
- Audit logs for all policy changes and verification outcomes.

## Testing

- Negative tests for tag mutation and digest mismatches.
- Signature verification tests including key rotation.
- Tests for policy enforcement at release creation and host fetch.
- Supply chain incident simulation (compromised dependency) to validate response speed.

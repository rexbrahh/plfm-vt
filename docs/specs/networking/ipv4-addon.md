# docs/specs/networking/ipv4-addon.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
Define the dedicated IPv4 add-on:
- what enabling IPv4 means
- allocation unit and lifecycle
- port exposure rules
- how IPv4 interacts with L4 routing and raw TCP
- billing and quota hooks (even if billing is not implemented yet)

Locked decision: IPv6-first; IPv4 is a paid add-on. See `docs/ADRs/0007-network-ipv6-first-ipv4-paid.md`.

## Scope
This spec defines behavior of the IPv4 add-on as a networking feature.

This spec does not define:
- pricing amounts (product doc)
- billing implementation (future)
- non-dedicated shared IPv4 tiers (explicitly not v1)
- L7 termination behavior (separate spec)

## Definitions
- **Dedicated IPv4 allocation**: a single public IPv4 address assigned exclusively to an environment.
- **Allocation unit**: the resource that owns the IPv4 address (v1: environment).
- **Port binding**: mapping of `(ipv4_address, public_port)` to a Route backend set.
- **Allowed port set**: policy-defined ports that can be exposed.

## High-level behavior (v1)
1) IPv4 is not provided by default.
2) Enabling IPv4 allocates one dedicated public IPv4 address to an environment.
3) IPv4 is used primarily for:
- IPv4-only clients
- raw TCP services that require IPv4 reachability
4) Internally, traffic continues to route over IPv6 overlay to backends.
5) Disabling IPv4 releases the address, subject to cooldown.

## Allocation unit (locked for v1)
- IPv4 is allocated per `(org, app, env)`.

Rationale:
- aligns with env-scoped config, secrets, and routing ownership
- makes billing and ownership unambiguous
- avoids mixing staging/prod under one IP

## Lifecycle

### Enable
When a user enables IPv4 add-on for an env:
1) control plane validates permission and quota
2) IPAM allocates an IPv4 address from the pool
3) control plane emits:
- `env.ipv4_addon_enabled` event containing allocation_id and ipv4_address
4) edge begins binding listeners on that IPv4 as required by routes

### Disable
When a user disables IPv4 add-on:
1) control plane validates:
- env has no active routes requiring IPv4 (unless forced by operator policy)
2) control plane emits:
- `env.ipv4_addon_disabled`
3) edge stops binding listeners on that IPv4
4) IPAM marks the address released and enters cooldown before reuse

Cooldown policy (v1 recommendation):
- minimum 24 hours before address reuse
- operator-configurable

### Re-enable
Re-enabling after disable allocates a potentially different IPv4 address.
- Users must not assume IPv4 stability after disable.

## Port exposure rules
### General
- IPv4 address is dedicated, but ports are still controlled and must be explicit.

Rules:
- Exposing any port requires a Route (or equivalent binding object).
- The platform rejects conflicting port bindings on the same IPv4.

### Default ports (v1 recommendation)
When IPv4 is enabled, the platform allows by policy:
- 80 and 443 by default (if the env has routes that use them)
- A small additional TCP port bundle for raw TCP (product policy, example 2–5 extra ports)

All other ports require explicit enablement and must pass policy checks.

### Denylist
Operator-defined denylist applies to IPv4 port exposure too (see ingress-l4.md).

### Binding semantics
A port binding maps:
- `(env.ipv4_address, public_port)` -> Route backend set

For tls_passthrough:
- the binding typically uses port 443 and SNI inspection may still apply if multiple hostnames are served (but because IP is dedicated per env, you can simplify and serve only that env’s hostnames).

For tcp_raw:
- the binding is per port and does not use SNI.

## Interaction with Routes
Routes remain env-scoped and define the backend target.

IPv4 add-on affects whether a Route can be activated:
- If a Route has `ipv4_required=true`, then env must have IPv4 enabled.
- If env has IPv4 enabled, routes may choose to be reachable over IPv4 and/or IPv6 depending on policy.

v1 recommendation:
- Provide clear route fields:
  - `public_ipv6 = true|false` (default true)
  - `public_ipv4 = true|false` (default false unless ipv4_required)
This can be additive later. For now, `ipv4_required` is the minimal signal.

## Routing behavior (data plane)
- Edge listens on the env’s dedicated IPv4 for the bound ports.
- Edge proxies connections to backends over IPv6 overlay, same as IPv6 ingress.
- No internal component should require IPv4 addressing.

## Quotas and abuse controls
IPv4 is scarce. Control plane must enforce:
- per-org max number of IPv4 allocations
- per-env max number of exposed ports (policy)
- rate limits on route changes that bind many ports

## Audit requirements
Enabling/disabling IPv4 must be auditable:
- who enabled
- when
- which env
- which IP address
- which ports became exposed as a result

These are captured by:
- env.ipv4_addon_enabled / disabled events
- route create/update events
- any port binding events if you model them separately later

## Failure behavior
- If IPv4 pool is exhausted:
  - enabling IPv4 fails with clear error `ipv4_pool_exhausted`.
- If edge cannot bind the IPv4 address:
  - the env is marked degraded and routes fail to activate.
- If IPv4 is disabled while routes still require it:
  - v1 recommendation: reject disable unless operator forces.

## Operator requirements
- IPv4 pool configuration:
  - operator provides a set of allocatable IPv4 addresses
  - pool must exclude addresses already in use
- Monitoring:
  - pool utilization
  - allocation failures
  - address leaks (allocated but env deleted)

## Open questions (explicitly deferred)
- Shared IPv4 tier for HTTP/HTTPS only (would likely require L7 termination and a new ADR).
- Per-port billing vs included port bundle pricing (product decision; API should support both).

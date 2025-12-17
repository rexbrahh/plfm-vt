# docs/adr/0009-proxy-protocol-v2-client-ip.md

## Title

Propagate client source identity using PROXY Protocol v2

## Status

Locked

## Context

With L4-first ingress and an overlay network, the backend workload will typically see the **edge proxy** or **host-side forwarding address** as the remote peer, not the true client IP and port.

We need a consistent way to propagate client source identity for:

* application logs and debugging
* rate limiting and abuse controls (present and future)
* security auditing
* consistent behavior across IPv6 and IPv4 (paid add-on)

We also want to avoid HTTP-specific mechanisms because ingress is not L7-first.

## Decision

1. **The platform uses PROXY Protocol v2 as the standard mechanism to propagate client source identity** from the edge ingress to workload instances.

2. **When client identity propagation is enabled for a route**, the edge ingress prepends a PROXY Protocol v2 header to the upstream connection before any application bytes.

3. **Workloads receiving proxied connections must either:**

* natively support PROXY Protocol v2 on the exposed port, or
* run behind a platform-provided L4 adapter inside the microVM that parses PROXY v2 and forwards a clean stream to the application.

4. **We treat PROXY Protocol v2 as trusted only when it originates from platform-controlled ingress.**

* Workloads must not accept PROXY headers from arbitrary sources on the public internet.
* The platform ensures the PROXY header is injected only on controlled ingress paths.

## Rationale

* PROXY Protocol v2 is protocol-agnostic (works for raw TCP and TLS passthrough).
* It supports IPv6 cleanly and efficiently (binary format).
* It avoids HTTP-only headers like `X-Forwarded-For` and does not require TLS termination.
* It creates a single cross-cutting “source identity” contract across ingress implementations.

## Consequences

### Positive

* Backends can access true client IP and port without L7 termination
* Consistent behavior for IPv6 and IPv4 add-on routes
* A clear security model: “trust PROXY only from the platform edge”

### Negative

* Upstream stream is modified (PROXY header prepended), which requires app support or an adapter
* Misconfiguration can cause confusing failures (apps reading the PROXY header as payload)
* Must ensure no path allows an untrusted party to spoof PROXY headers

## Alternatives considered

1. **No propagation (accept that backends see proxy IP only)**
   Rejected because it harms debuggability and blocks future abuse controls.

2. **HTTP headers (`X-Forwarded-For`, `Forwarded`)**
   Rejected because it is HTTP-only and implies L7-first behavior.

3. **Transparent proxying (TPROXY) to preserve source IP at L3/L4**
   Rejected for v1 due to operational complexity across overlay, microVM boundaries, and mixed IPv6/IPv4 requirements.

4. **Custom metadata side channel**
   Rejected because it reinvents a standard and complicates every runtime and language.

## Invariants to enforce

* If a route is configured for client identity propagation, the edge must always inject PROXY v2.
* Only platform-controlled ingress components may inject PROXY headers. Internal traffic paths must not allow spoofing.
* A workload port must be clearly designated as “expects PROXY v2” vs “raw stream” to avoid accidental breakage.
* Observability must record both the transport peer (proxy) and the propagated client identity for debugging.

## What this explicitly does NOT mean

* We are not terminating TLS by default.
* We are not promising that every off-the-shelf TCP service will work without configuration. Compatibility may require enabling PROXY support or using the platform adapter.
* We are not using PROXY Protocol as a general authentication mechanism. It is metadata for source identity only.

## Open questions

* Default policy: whether client identity propagation is enabled by default for public routes, or opt-in per route for compatibility.
* Adapter scope: which common servers/protocols we ship a built-in adapter for (or whether we standardize “apps must support PROXY” for v1).

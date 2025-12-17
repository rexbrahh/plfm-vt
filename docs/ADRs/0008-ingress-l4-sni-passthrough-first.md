# docs/adr/0008-ingress-l4-sni-passthrough-first.md

## Title

Ingress is L4-first with SNI passthrough as the default

## Status

Locked

## Context

We need an ingress model that:

* works for HTTP, HTTPS, and raw TCP without forcing application changes
* preserves end-to-end TLS by default (no mandatory TLS termination at the edge)
* keeps early platform complexity low while still being production-usable
* fits the IPv6-first posture and dedicated IPv4 add-on model
* allows adding selective L7 features later without redesigning the core routing plane

We also explicitly value not terminating or rewriting connections as the default behavior.

## Decision

1. **The primary ingress plane is Layer 4.**
   The platform routes connections based on L4 primitives: destination IP, port, and for TLS traffic, SNI where available.

2. **SNI passthrough is the default for TLS traffic.**

* the platform does not terminate TLS by default
* the platform routes TLS streams to the correct backend using SNI inspection only
* certificates and TLS termination are the responsibility of the user workload unless explicitly using an L7 feature later

3. **Raw TCP is a first-class ingress target.**

* raw TCP services are exposed via dedicated ports
* if IPv4 is required for raw TCP reachability, it uses the dedicated paid IPv4 allocation model

4. **L7 features are optional and additive.**

* we may later offer L7 termination for specific use cases (auth, rate limits, caching)
* enabling L7 must be an explicit opt-in per route or per service
* L7 must not become a dependency for core platform routing correctness

## Rationale

* L4-first is simpler, more general, and supports a broader range of workloads than HTTP-first ingress.
* SNI passthrough preserves end-to-end encryption and reduces certificate management burden on the platform.
* This posture minimizes “magic” and keeps routing behavior explainable and debuggable.
* It aligns with your requirement that L4 should not terminate connections and should feel transparent.

## Consequences

### Positive

* Supports HTTP, HTTPS, and arbitrary TCP protocols with one routing plane
* Reduces platform responsibility for certificate storage and key security in v1
* Keeps data plane implementation simpler and less fragile early

### Negative

* Limited ability to provide HTTP-level features without opt-in L7 termination
* Some routing requires TLS SNI presence, which not all TLS clients provide
* Observability at HTTP level is reduced unless L7 is enabled

## Alternatives considered

1. **L7-first ingress (HTTP proxies, TLS termination by default)**
   Rejected because it forces HTTP assumptions, increases complexity, and makes raw TCP a second-class path.

2. **Terminate TLS always**
   Rejected due to key management risks and because it violates the “transparent L4” intent.

3. **Pure L4 with no SNI inspection**
   Rejected because routing HTTPS by hostname would be impossible without dedicating IPs per hostname, which conflicts with IPv4 scarcity and increases cost.

## Invariants to enforce

* Default TLS routing must not require storing user private keys in the platform.
* L4 routing rules must be derived from control plane desired state and applied consistently across edge nodes.
* SNI inspection must not modify traffic; it is used only for routing.
* Raw TCP exposure must be explicit and auditable (port allocation, route ownership, billing ties to IPv4 when applicable).

## What this explicitly does NOT mean

* We are not implementing a full featured HTTP gateway in v1.
* We are not guaranteeing support for exotic TLS modes that hide SNI (for example, if a client does not send SNI, hostname routing cannot work without dedicated IPs).
* We are not promising WAF, CDN, or caching in v1.
* We are not doing per-request routing decisions. Routing is per-connection.

## Open questions

* Default ports and constraints: which ports are allowed on IPv6-only vs when IPv4 add-on is enabled.
* How we model “hostnames” in the control plane: route objects and their ownership.
* Strategy for clients without SNI: require dedicated IP or deny, and how that is exposed in UX.

Proceed to **ADR 0009** when you’re ready.

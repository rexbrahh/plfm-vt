# docs/ADRs/0007-network-ipv6-first-ipv4-paid.md

## Title

Networking is IPv6-first; IPv4 is a paid add-on

## Status

Locked

## Context

We want a networking model that is:

* simple to operate at small scale on bare metal
* compatible with a WireGuard overlay and multi-node scheduling
* consistent with L4 ingress and minimal middleboxes
* cost aware (IPv4 is scarce and increasingly expensive)
* future proof and not built around NAT as a default assumption

We also want product differentiation: a platform that is comfortable being IPv6-native.

## Decision

1. **IPv6 is the default and primary network protocol everywhere.**

* control plane to node communication is IPv6
* node to node overlay is IPv6
* workload addressing is IPv6
* default public endpoints are IPv6

2. **IPv4 is not provisioned by default.**
   Users who need IPv4 reachability must enable an add-on.

3. **IPv4 add-on provides dedicated public IPv4 resources.**

* the add-on allocates a dedicated public IPv4 address for an environment (or other unit defined in later specs)
* the platform controls port exposure for that IPv4 address
* pricing is attached to IPv4 allocation and ongoing usage policy

4. **Platform routing and service discovery are designed to not require IPv4.**

* no internal dependency on IPv4 for correctness
* IPv4 is treated as an optional compatibility layer at the edge

## Rationale

* IPv6 eliminates many NAT driven complexities and makes address allocation cleaner, especially over a WireGuard overlay.
* Defaulting to IPv6 reduces operational and cost pressure from scarce IPv4 addresses.
* A paid IPv4 add-on aligns costs with the users who require IPv4 reachability.
* This keeps the platform architecture consistent: the control plane and data plane do not have two equally primary networking stacks to maintain.

## Consequences

### Positive

* Cleaner internal networking model and IPAM story
* Lower default operating cost and less IPv4 pool management
* Encourages an IPv6-native platform posture from day one

### Negative

* Users with IPv4-only clients must pay for IPv4 or cannot serve those clients
* Support burden increases because many environments still assume IPv4 by default
* We must design a clear UX for “you are IPv6-only right now” and make IPv4 enablement explicit

## Alternatives considered

1. **Dual-stack by default (IPv4 + IPv6 for every service)**
   Rejected due to IPv4 scarcity and cost, and because it makes two stacks first-class in all components.

2. **IPv4-first with optional IPv6**
   Rejected because it bakes legacy assumptions into the architecture and weakens the intent of the platform.

3. **Shared IPv4 front door for all users**
   Rejected as a default posture because it still requires maintaining IPv4 capacity and can create contention and unclear guarantees. This may be reconsidered later as an additional product tier, but it is not the baseline decision here.

## Invariants to enforce

* Overlay and internal control plane traffic must function with IPv6 only.
* Every workload instance must receive IPv6 addressing and be routable internally over IPv6.
* DNS records are AAAA by default; A records only exist when IPv4 add-on is enabled.
* IPv4 allocations are explicit, tracked, auditable, and billable. No “accidental IPv4”.
* Any feature that requires IPv4 internally is considered a bug or a design regression.

## What this explicitly does NOT mean

* We are not promising that IPv6-only endpoints will be reachable by every end user on the internet.
* We are not implementing NAT traversal features for end users in v1.
* We are not guaranteeing a free shared IPv4 endpoint as part of the default plan.
* We are not building internal systems that assume IPv4 addresses exist.

## Open questions

* Exact unit of allocation for IPv4 add-on: per environment, per app, or per hostname.
* Whether IPv4 add-on includes a fixed port bundle or per-port pricing.
* Whether we later introduce a shared IPv4 tier for specific protocols while keeping dedicated IPv4 as the premium option.

Proceed to **ADR 0008** when you’re ready.

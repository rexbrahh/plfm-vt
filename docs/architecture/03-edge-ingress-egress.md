# docs/architecture/03-edge-ingress-egress.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
This document describes the edge layer (ingress) and the platform egress posture. It explains how external connections are routed to workloads, how raw TCP works, and how client identity is propagated without requiring L7.

Authoritative details live in:
- `docs/specs/networking/ingress-l4.md`
- `docs/specs/networking/ipam.md`
- `docs/specs/networking/overlay-wireguard.md`
- `docs/specs/networking/proxy-protocol-v2.md`
- `docs/specs/networking/ipv4-addon.md`
- ADR 0007, 0008, 0009

## What the edge is
The edge is a set of platform-controlled ingress nodes that:
- accept inbound connections (IPv6 by default, IPv4 when add-on is enabled)
- apply L4 routing rules derived from control plane state
- route traffic to the correct backend instances across the overlay
- optionally prepend PROXY protocol v2 headers (opt-in per route)

The edge is part of the trusted platform.

## What the edge is not
- Not an L7-first HTTP gateway by default.
- Not a WAF or CDN in v1.
- Not a general NAT box for customers.
- Not a place that stores user TLS private keys by default (SNI passthrough is the default stance).

## Ingress principles
### L4-first routing
Ingress routing is based on:
- destination IP
- destination port
- for TLS passthrough routes: SNI inspection from ClientHello (without terminating TLS)

Routing decisions are per-connection, not per-request.

### SNI passthrough default
For TLS routes:
- edge inspects the ClientHello to read SNI
- edge does not terminate TLS
- edge does not present certificates
- edge does not modify payload bytes except optional PROXY v2 injection when enabled for the route

Clients without SNI:
- hostname-based routing requires SNI
- if SNI is absent, routing is only supported when the `(IP,port)` mapping is unambiguous, typically via dedicated address-based routing

### Raw TCP as first-class
Non-HTTP protocols are supported via explicit port allocation and routing.
This is not a special-case feature. It is a core capability of the platform.

## Route model (control plane view)
Ingress is driven by first-class Route objects.

A Route binds:
- a hostname (or address-based binding)
- a listener port (443, 80, or an explicitly allocated TCP port)
- protocol hint (TLS passthrough vs raw TCP)
- target environment and process type
- backend port inside the microVM
- optional flags:
  - PROXY v2 enabled
  - IPv4 required
  - health source and gating behavior

Ownership and conflict rules:
- A hostname can only map to one environment at a time.
- Route changes are state transitions recorded as events with audit metadata.

## Addressing and reachability
### IPv6-first
Default behavior:
- public endpoints are IPv6 (AAAA records)
- internal routing uses IPv6 over the WireGuard overlay

### IPv4 add-on (dedicated)
IPv4 is not provisioned by default. When enabled:
- an environment receives a dedicated public IPv4 allocation
- public ports are bound to that IPv4 based on explicit policy
- traffic enters on IPv4 and is forwarded internally over IPv6 (overlay) to avoid polluting internal systems with IPv4 assumptions

Unit of allocation (v1 stance):
- dedicated IPv4 is allocated per environment.

## Edge-to-backend routing
Backend instances live on arbitrary hosts across the overlay.

Edge routes to backends using:
- overlay IPv6 addresses allocated to workloads or to host-level proxies that forward into the microVM
- health-gated backend selection (only route to instances that are ready)

The exact load-balancing strategy can be simple in v1:
- round-robin across ready instances for a route
- optional least-connections later
- strict stickiness is not required in v1 unless a product need forces it

## Health checks and readiness gating
Ingress must not route to instances that are not ready.

Readiness signals can be derived from:
- host agent instance readiness events (control plane view)
- optional direct TCP health checks performed by edge (if needed)

A simple v1 approach:
- control plane computes “route backends” as the set of ready instances
- edge consumes route-backend updates and applies routing tables

Avoid in v1:
- edge directly querying hosts in a way that creates coupling or tight feedback loops.

## Client identity propagation (PROXY protocol v2)
Because ingress is L4-first, the backend often sees edge as the peer.

Mechanism:
- If a Route enables it, edge prepends PROXY protocol v2 header to the upstream connection.

Default stance:
- PROXY protocol is opt-in per Route and off by default to avoid breaking servers.

Operational consequences:
- Backends must explicitly support PROXY v2 on that port or run a front proxy that does.
- The platform must prevent spoofing:
  - only platform-controlled ingress may inject PROXY headers
  - do not allow public traffic paths that can send arbitrary PROXY headers into trusted ports

## Egress posture
Workloads often need outbound internet access.

Default v1 posture:
- allow outbound egress from workloads to the public internet, subject to platform-defined safety limits
- egress goes from microVM -> host -> underlay

Policy considerations (v1 minimal):
- rate limiting and abuse controls are operator features
- record enough metadata to investigate abuse:
  - which org/env/instance generated traffic
  - approximate volumes (not full packet capture)

Not a v1 goal:
- fine-grained egress firewall rules per app unless required by a specific customer story
- NAT gateway products

## Failure modes and degraded behavior
### Edge node failure
- If multiple edge nodes exist, traffic shifts to surviving edge nodes (DNS or anycast strategy defined elsewhere).
- Backends keep running.

### Partial overlay partition
- Edge may lose reachability to some hosts.
- Those instances are removed from routing tables.
- Platform surfaces degraded state.

### Control plane down
- Edge continues routing using last applied routing config.
- No new route changes or deploys are possible.
- This requires edge config persistence and safe reloads.

### Misconfiguration
- If a backend does not support PROXY v2 but route has it enabled, traffic will break.
- The platform should detect this early via validation checks or by surfacing clear failures in health.

## Operational requirements
- Edge config must be derived only from control plane desired state and applied atomically.
- Edge must have observability:
  - per-route connection counts
  - error rates
  - backend selection stats
  - reachability and overlay health
- Edge must support safe reloads without dropping existing connections when possible.

## Next document
- `docs/architecture/04-state-model-and-reconciliation.md`

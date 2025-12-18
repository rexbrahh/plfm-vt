# docs/ADRs/0004-overlay-wireguard-full-mesh.md

## Title

Inter-node overlay network is WireGuard with full-mesh peering

## Status

Locked

## Context

We need secure connectivity between hosts to support:

* scheduling workloads across multiple physical machines
* service to service communication and internal control plane traffic
* consistent routing and addressability for IPv6-first design
* a simple operational model early in the product lifecycle

We are building a small team platform and want to avoid large dependencies and operationally heavy SDN stacks in v1.

## Decision

1. **WireGuard is the overlay network technology** used to connect all platform nodes.

2. **Topology is full mesh in v1**:

* every node maintains a WireGuard peer session with every other node
* nodes can communicate directly over the overlay without transiting a central gateway

3. **The control plane is the source of truth for overlay membership**:

* node identity, public keys, and allowed IPs are issued and rotated under platform control
* joining and leaving the mesh is managed via node enrollment procedures

4. **Overlay addressing is IPv6-first**:

* each node receives stable IPv6 addresses on the overlay
* workload and service addressing builds on IPv6 allocations and routing derived from the control plane

5. **Overlay is used for internal traffic, not as a customer visible VPN feature** in v1.

## Rationale

* WireGuard is simple, fast, and well understood operationally.
* Full mesh is the simplest topology to start with, avoids central bottlenecks, and keeps routing rules straightforward.
* It aligns with IPv6-first decisions and provides a clean substrate for node-to-node traffic.

## Consequences

### Positive

* Minimal operational dependency footprint for secure node interconnect
* Direct path between nodes (lower latency, simpler failure domains)
* Clear security model: cryptographic identity per node, controlled membership

### Negative

* Full mesh does not scale indefinitely (peer count grows O(n²))
* Key rotation and membership changes must be handled carefully to avoid partitioning
* Debugging network issues requires good tooling and observability

## Alternatives considered

1. **Hub-and-spoke WireGuard**
   Rejected for v1 because it introduces gateway bottlenecks and more routing complexity.

2. **Tailscale / external control plane overlay**
   Rejected because it introduces external dependency and reduces platform ownership.

3. **BGP based underlay routing only**
   Rejected because it is provider dependent and does not provide encryption and membership control by default.

4. **Cilium or other SDN stacks**
   Rejected for v1 due to operational complexity and the desire to avoid Kubernetes centric dependencies.

## Invariants to enforce

* Every node has a unique WireGuard keypair, managed and rotated by the platform.
* Node overlay addresses and allowed IPs are allocated by the control plane and must not overlap.
* Overlay membership changes are atomic from the control plane perspective: we must avoid partial propagation that splits the cluster.
* Control plane traffic must be able to traverse the overlay even during partial underlay failures, where possible.

## What this explicitly does NOT mean

* We are not offering customer accessible WireGuard tunnels in v1.
* We are not committing to full mesh forever. This is a v1 simplification.
* We are not implementing complex overlay routing policies in v1.

## Open questions

* When we outgrow full mesh: introduce regional hubs, or move to a routed overlay with dynamic peers.
* Node bootstrap: how keys are provisioned at first enrollment, and how we prevent rogue nodes from joining.
* Whether overlay MTU and fragmentation behavior needs standardization for customer workloads.

Proceed to **ADR 0005** when ready.

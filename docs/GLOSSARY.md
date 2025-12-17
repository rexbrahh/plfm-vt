# docs/GLOSSARY.md

Status: reviewed  
Owner: TBD  
Last reviewed: 2025-12-16

## Core entities
**Org (tenant)**  
A top-level account boundary. Billing, quotas, and authorization are scoped here.

**Project**  
A grouping inside an org. Optional, but useful for teams and separation.

**App**  
A named service owned by an org (and optionally grouped under a project). Example: `api`, `worker`, `web`.

**Environment (env)**  
A deployable instance of an app with its own config, secrets, routes, and releases. Example: `prod`, `staging`.

**Release**  
An immutable deploy artifact defined by an OCI image digest plus manifest content hash. Releases are what you roll forward and back to.

**Manifest**  
The platform configuration file that describes how to run a release. It declares ports, resources, process types, health checks, secrets mounts, volume mounts, and routing intent.

**Workload**  
A runnable unit derived from a release plus environment configuration.

**Process type**  
A named run command within an environment (example: `web`, `worker`). Each process type scales independently.

**Instance**  
One running replica of a process type. In this platform, one instance corresponds to one microVM.

## Runtime and host
**MicroVM**  
A lightweight virtual machine used as the workload isolation boundary. Implemented with Firecracker.

**Firecracker**  
The microVM VMM used to run instances.

**Host (node)**  
A physical or virtual machine that runs the node agent and hosts microVMs.

**Host agent (node agent)**  
The daemon on each host that fetches images, boots microVMs, applies resource limits, wires networking, mounts volumes, injects secrets, and reports status.

**Control plane**  
APIs and services that accept user intent, store state, schedule workloads, and publish desired state to agents and edge.

**Data plane**  
The parts that directly run and route workloads: host agents, microVMs, and edge ingress.

**Edge**  
The ingress layer that accepts external connections and routes them to the correct workload instance.

## Networking
**Underlay**  
The physical network between machines (public internet, datacenter network).

**Overlay**  
A virtual network built on top of the underlay. Here, WireGuard.

**WireGuard mesh**  
The overlay topology where each node peers with every other node in v1.

**IPAM**  
IP address management. Allocates IPv6 addresses and prefixes for hosts and workloads.

**IPv6-first**  
Design stance that IPv6 is the default everywhere (internal and external). IPv4 exists only when explicitly enabled.

**IPv4 add-on**  
A paid feature that allocates a dedicated public IPv4 address for an environment and enables IPv4 reachability.

**Ingress**  
Incoming connections from the internet (or external clients) into the platform.

**Egress**  
Outbound connections from workloads to the internet.

**L4 (Layer 4) ingress**  
Routing based on IP and port, and for TLS traffic, using SNI inspection without terminating TLS.

**L7 (Layer 7) ingress**  
HTTP-aware routing and features. Optional and explicitly separate from v1 core routing.

**SNI (Server Name Indication)**  
A TLS ClientHello field carrying the intended hostname. Used for routing TLS streams without terminating TLS.

**SNI passthrough**  
The platform inspects SNI for routing but does not terminate TLS and does not modify payload bytes (except optional PROXY v2 when enabled).

**PROXY Protocol v2**  
A binary header prepended to upstream connections to convey the true client source address and port. Opt-in per route.

**Route**  
A first-class object binding a hostname and listener port to an environment and process type, with routing and health semantics.

**Port allocation**  
The rules and state for which public ports are bound to which environments (especially for raw TCP).

## State model
**Event log**  
An append-only sequence of state transition records that forms the source of truth.

**Event**  
One immutable record describing a validated state change, including actor and payload.

**Projection**  
Code that consumes events and updates derived state.

**Materialized view**  
A derived table representing current state for fast reads, built by projections.

**Reconciler (reconciliation loop)**  
A controller that continuously compares desired state (from views) to actual state (from agents) and takes action to converge.

**Desired state**  
What the control plane says should be running and routed.

**Actual state**  
What is actually running on hosts and how traffic is currently routed.

## Secrets and storage
**Secret bundle**  
A set of secrets scoped to `(org, app, env)`.

**Secret version**  
A specific immutable version of a secret bundle.

**Secrets file**  
The mounted file inside the microVM containing secrets in a fixed platform format.

**Volume**  
A persistent storage unit that lives on a specific host (local volume).

**Snapshot**  
A point-in-time capture of a volume.

**Backup**  
An asynchronous copy of a snapshot stored remotely for recovery.

**Restore**  
Creating a new volume from a snapshot or backup.

**Drain / evict / reschedule**  
Operations used during maintenance or failures to move workloads off a host (stateless) or to restore stateful workloads via restore and reattach.

# Problem statement

This project is building a developer-focused Platform as a Service that prioritizes **clarity**, **control**, and **repeatability** over “magic”.

Most platforms ask developers to trade away one or more of these:

- **Transparency**: what is running, why it is running, and what changed.
- **Network control**: raw TCP services, explicit ports, predictable ingress.
- **Operational ergonomics**: first-class logs, events, exec, rollback, and introspection.
- **Scripting guarantees**: stable IDs, machine-readable output, consistent exit codes.
- **Portability**: minimal assumptions about language, framework, or build toolchain.

We want a platform where the default experience is:

- You can explain the system state to a teammate in plain language.
- You can reproduce an environment from a small manifest.
- You can deploy, debug, and recover quickly with a single interface.

## What is broken today

### The “dashboard trap”
Many PaaS offerings start simple, then drift into a UI-driven workflow where:

- Mutations happen via clicks with weak auditability.
- “Current state” is implied by the UI, not declared in source control.
- Advanced operations (rollbacks, incident response) require tribal knowledge.

This is fine until you need to script, automate, or debug reliably.

### The “HTTP-only” bias
A lot of platforms are excellent for HTTP, but become awkward for:

- Raw TCP services (databases you own, game servers, proxies, custom protocols)
- Non-standard ports
- IPv6-first deployments
- Workloads that need L4 transparency (no hidden termination)

We want raw TCP to be a first-class citizen from day one.

### Secrets and environment drift
Teams suffer from environments that “sort of work” because:

- Secrets are scattered across a UI, CI, and runtime.
- Deploys succeed but runtime fails due to missing config.
- No single artifact exists that represents “what this release needs”.

We want releases to be gated by explicit environment and secret decisions.

### Debugging is the product
In real life, “deploy” is not the hard part. Debugging is.

The platform should make it easy to answer:

- What is running right now?
- What changed recently?
- Why did this instance restart?
- Which release is serving traffic?
- What is the network surface and how is it configured?

## Our approach

### CLI-first, with a web terminal as a complement
The primary interface is a CLI designed for:

- idempotent operations
- stable IDs
- predictable output (human and machine)
- explicit state transitions

A web console exists, but it is **terminal-native** (libghostty-vt via WASM), and it speaks the same primitives as the CLI.

### Manifest-first workflow
The smallest useful manifest should be enough to:

- define an app and environments
- define workloads (processes)
- define network endpoints
- define volumes
- define required config surface (env and secrets)

The manifest is not a “template”. It is the core contract.

### Reconciliation over imperative magic
The control plane maintains a desired state and continuously reconciles toward it.

User-facing UX should reflect this:

- commands return a **receipt** (what was accepted)
- you can `wait`, `events`, `describe`, and `diff` desired vs current
- eventual consistency is explicit, not hidden

### L4-first networking, IPv6 by default
Ingress is L4 first:

- TCP and UDP (where supported) are explicit endpoints
- IPv6 is the default externally and internally
- dedicated IPv4 is available as a paid add-on (scarce resource)

Optional Proxy Protocol v2 exists for apps that need client connection metadata at L4.

## Non-goals (for v1)

- A click-first dashboard product
- A full CI/CD replacement
- A managed database suite on day one
- A “one button” abstraction that hides networking and runtime details

We want to ship a platform where advanced users feel at home, and newer users can learn by seeing everything laid out plainly.

# Target users and use cases

This document describes who the platform is built for, the jobs they are trying to accomplish, and which users we are explicitly not optimizing for in v1.

## Primary target users

### 1) Builders who live in the terminal
People and teams who:

- ship production services
- automate workflows (CI, scripts, makefiles)
- want a repeatable deployment interface

They value speed, composability, and predictable tooling.

### 2) Small teams that want “just enough platform”
Teams who are past VPS management but do not want a heavy Kubernetes layer.

They want:

- a simple manifest
- safe deploys and rollbacks
- first-class logs and exec
- predictable networking

### 3) Workloads that do not fit “HTTP-only”
Users deploying:

- TCP services (custom protocols, game servers, message brokers, proxies)
- services that need L4 transparency
- IPv6-first deployments

They need explicit ports and stable endpoint behavior.

### 4) Platform-curious teams migrating from Heroku-style PaaS
Teams that want a PaaS experience, but with:

- better network control
- better introspection and debugging surfaces
- fewer hidden side effects

## Secondary target users

### 5) Internal tools and ephemeral environments
Examples:

- preview environments per branch
- staging environments with strict parity
- admin tools that need secure access paths

These benefit from explicit manifests and stable, scriptable commands.

## Users we are not optimizing for (v1)

- Users who want a fully GUI-driven experience
- Large enterprises that require complex compliance features (SOC2 workflows, custom legal controls) out of the gate
- Teams that need a managed ecosystem of add-ons on day one (databases, queues, etc.) instead of bringing their own

This does not mean “never”. It means “not the first product promise”.

## Core use cases

### Deploy a web service
- Deploy an OCI image (or build from a Dockerfile if supported by the CLI)
- Expose a TCP endpoint on 80/443
- Optionally attach a dedicated IPv4 address
- Use the platform’s event stream and logs for debugging

### Deploy a background worker
- Run a long-lived process without public ingress
- Scale instance counts
- Exec into running instances for investigation

### Deploy a TCP service with explicit ports
- Create one or more L4 endpoints with fixed ports
- Enable Proxy Protocol v2 when required
- Keep TLS termination inside the app if desired

### Attach persistent storage
- Create and attach volumes
- Restore from snapshots (if enabled)
- Move workloads while keeping data safe

### Operate and recover quickly
- View current vs desired state
- Tail logs and system events
- Roll back to a previous release deterministically
- Inspect why an instance restarted or failed health checks

## Product promises

These are UX-level guarantees that shape everything else:

- **You can understand your runtime**: the platform always provides a clear explanation of what is running and why.
- **You can automate safely**: commands are consistent, idempotent where possible, and script-friendly.
- **You can debug without guessing**: logs, events, and describe commands are part of the primary experience.
- **Networking is explicit**: endpoints are concrete resources with clear configuration and ownership.

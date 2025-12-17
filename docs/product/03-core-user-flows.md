# Core user flows

This document defines the “happy path” and the recovery path for the core platform actions.

A theme across all flows: the system is reconciliation-based. After a mutation, users should expect:

1. A receipt that confirms what was accepted as desired state
2. The ability to watch progress via events
3. A final converged state visible in `status` or `describe`

The same flows should work in both:
- the CLI (primary)
- the web console terminal (secondary, same commands and outputs)

## Glossary of nouns

- **Org**: billing and membership boundary
- **Project**: grouping of apps and environments
- **App**: a named application within a project
- **Env**: environment within an app (prod, staging, dev)
- **Release**: immutable deployable artifact + config reference
- **Workload**: desired runtime configuration (process type, resources, scaling)
- **Instance**: a running unit of a workload
- **Endpoint**: an L4 ingress mapping (IPv6 by default, IPv4 optional)
- **Volume**: persistent storage resource
- **Secret bundle**: a named set of secrets delivered as a file

## 1) Onboarding

### Goals
- user can authenticate
- user creates first app environment
- user deploys a first release
- user sees traffic reach the app

### Flow
1. Install CLI
2. Authenticate (browser-based device flow or token)
3. Create org and project (or select existing)
4. Create app and environment
5. Create or import configuration:
   - env variables
   - secret bundle(s)
6. Deploy
7. Expose endpoint (if public service)
8. Confirm success with:
   - status
   - logs
   - simple health request

### UX requirements
- The user should not be allowed to accidentally deploy with “mystery config”.
- If required config is missing, the CLI must block release creation, explain what is missing, and offer next actions.
- The web console should provide a guided transcript-like onboarding session, not a form.

## 2) Deploy

### Goals
- create a release from an OCI image (and optional build step)
- roll out with clear progress
- produce an audit trail: what changed, where, when

### Flow
1. User runs deploy with a manifest (and optionally an image tag)
2. CLI validates:
   - manifest is well-formed
   - env and secret gates have been explicitly handled for this release
3. CLI submits desired state (new release + updated workload references)
4. CLI prints a receipt:
   - release ID
   - target environment
   - diff summary (workload changes, endpoint changes)
   - suggested follow-ups (`wait`, `events`, `status`)
5. Platform reconciles:
   - fetches image
   - schedules instances
   - attaches volumes
   - configures endpoints
6. User watches progress via events
7. User confirms via status and logs

### Failure and recovery
- If rollout fails, the CLI should:
  - show the failing step and error category
  - offer direct links to logs/events
  - offer rollback or retry paths

## 3) Logs

### Goals
- view application logs and system logs in one stream or separately
- support both “tail” and “fetch history”
- work well in incident response

### Flow
1. User tails logs for an environment or workload
2. Optionally filter:
   - workload
   - instance
   - severity
   - time range
3. Provide stable formatting:
   - timestamp
   - source (app/system)
   - workload and instance ID
   - message

### UX requirements
- Logs must be accessible even when instances crash.
- Log streaming should behave like a long-running session.
- In the web console, long-running streams should be easy to detach from the command transcript (separate session panel).

## 4) Exec

### Goals
- run a command in a running instance (debug, migrations, shell)
- attach an interactive TTY when needed

### Flow
1. User selects target:
   - workload
   - instance (or let CLI choose)
2. CLI creates an exec session and attaches
3. Output is streamed to the user

### UX requirements
- Exec should be explicit about which instance you are in.
- If there are multiple instances, the selection step must be visible and scriptable.
- The web console should handle interactive TTY cleanly (libghostty-vt embedded session).

## 5) Rollback

### Goals
- return to a previously known-good release quickly
- keep rollbacks deterministic and visible

### Flow
1. User lists releases for an environment
2. User selects a target release ID
3. CLI submits desired state change: env points to previous release
4. Platform reconciles toward that release
5. User watches events and confirms status

### UX requirements
- The platform must preserve release immutability.
- Rollback must not “rebuild” or mutate a release artifact.

## 6) Scale

### Goals
- scale instance counts and resources
- give visibility into placement and health

### Flow
1. User updates scale settings:
   - min and max instances (v1 may only support fixed count)
   - CPU and memory sizes
2. CLI prints a receipt with new desired scale
3. Platform reconciles:
   - starts or stops instances
   - re-places workloads when required
4. User confirms with status and events

### UX requirements
- Scaling operations should be safe and reversible.
- Status output must show both desired and current counts.

## 7) Expose ports (endpoints)

### Goals
- create and manage L4 ingress
- make IPv6 default and IPv4 explicit

### Flow
1. User declares an endpoint in manifest (or creates via CLI):
   - protocol (tcp)
   - external port(s)
   - internal port
   - IPv6 allocation (default)
   - IPv4 add-on (optional)
   - Proxy Protocol v2 (optional)
2. CLI prints endpoint details:
   - IPv6 address and port
   - IPv4 address if enabled
   - any special notes (proxy protocol, TLS responsibility)
3. Platform configures ingress and routes to instances

### UX requirements
- Endpoint resources must be visible and describable.
- “What is exposed to the internet” should be answerable in one command.

## 8) Attach volumes

### Goals
- create and attach persistent storage
- make mount paths explicit
- support safe rescheduling

### Flow
1. User creates a volume:
   - size
   - region/cluster constraints (if applicable)
2. User attaches volume to a workload at a mount path
3. Platform reconciles:
   - schedules instance where volume can attach
   - mounts volume
4. User confirms via status and a simple exec validation

### UX requirements
- Volume attachment should be explicit in the manifest.
- The platform must explain placement constraints when they exist.
- Operations like resize and snapshot should be discoverable via describe output.

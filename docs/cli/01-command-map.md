# CLI command map

This document maps the CLI command surface for v1. It is the authoritative list of top level commands, their intent, and the expected subcommands.

## Conventions

### Target selection
Most commands operate on an app and an environment.

Resolution order:
1. Explicit flags (`--app`, `--env`, `--org`, `--project`)
2. Local manifest in the current directory
3. Saved local context (if configured)

If no target can be resolved, the CLI fails with a usage error and prints the exact flag or command needed.

### Output modes
- Default output is human readable.
- `--json` outputs machine readable JSON and nothing else.

### Asynchrony and convergence
All mutations record desired state and return a receipt. Convergence is observed via:
- `status` (desired vs current)
- `events tail`
- `wait`

Mutation commands support:
- `--wait`
- `--wait-timeout`
- `--no-wait`

### Runtime config gate (required before release creation)
Before creating a release (or deploying), users must satisfy exactly one of:
- Set or import at least one runtime variable via `secrets set` or `secrets import`
- Explicitly acknowledge no runtime variables via `secrets confirm --none`

This avoids “fake required env vars” while preventing accidental missing config.

## Top level command tree

Usage:
  vt [flags] <command> [args]

Core workflow:
  launch       Create a new app from a folder or image, write a minimal manifest
  deploy       Build, create release, and deploy to an environment (manifest-first)
  status       Show desired vs current state for an app and its environment

App and lifecycle:
  apps         Create, list, rename, delete apps
  releases     Create, list, describe, promote, rollback releases
  workloads    List and manage workload groups (if applicable)
  instances    List and manage running instances (restart, exec, ssh)

Runtime configuration:
  secrets      Manage runtime variables and delivery state (set, unset, import, render)

Networking:
  endpoints    Manage L4 endpoints (IPv6 default, IPv4 add-on, Proxy Protocol v2)

Storage:
  volumes      Manage persistent volumes (create, attach, detach)
  snapshots    Manage snapshots (create, list, restore)

Observability and debugging:
  logs         View and tail logs
  events       View and tail control plane events
  describe     Deep inspect any resource (desired, current, conditions)
  wait         Wait for convergence on a resource

Account and access:
  auth         Login, logout, status, tokens
  orgs         Manage organizations
  projects     Manage projects

Help and tooling:
  doctor       Diagnose local setup and control plane connectivity
  completion   Generate shell completion scripts
  version      Show version and API compatibility
  help         Help for any command

Global flags:
  --app <name|id>
  --env <name|id>
  --org <name|id>
  --project <name|id>
  --json
  --debug
  --verbose
  --yes
  --no-input

## Command details

### launch
Create and configure a new app from a local directory or an image, write a minimal manifest, and optionally deploy.

Common:
- `vt launch` (uses current directory)
- `vt launch --image <ref>` (prebuilt image path)
- `vt launch --no-deploy` (only create app + manifest)

### deploy
Manifest-first deployment. Default behavior:
1. Validate manifest
2. Enforce runtime config gate
3. Build or resolve image
4. Create release
5. Promote release to env
6. Optionally wait for convergence

Common:
- `vt deploy` (current directory)
- `vt deploy --env staging --wait`
- `vt deploy --release <id>` (promote an existing release)

### status
Show desired vs current for the selected app and env:
- current release id, desired release id
- instance counts desired vs running
- endpoint status
- last reconcile time and last error if any

Common:
- `vt status`
- `vt status --json`

### apps
- `vt apps list`
- `vt apps create <name>`
- `vt apps delete <name|id>`
- `vt apps describe <name|id>`

### releases
- `vt releases list`
- `vt releases create` (create release only, no deploy)
- `vt releases describe <id>`
- `vt releases promote <id> --env <env>`
- `vt releases rollback [--to <id>]`
- `vt releases diff <id1> <id2>` (optional, if useful)

Notes:
- Releases are immutable.
- Promote and rollback are idempotent and emit receipts.

### workloads
Workload groups are optional in v1. If the manifest supports multiple workloads, this becomes active.

- `vt workloads list`
- `vt workloads describe <name|id>`
- `vt workloads scale <name|id> --count <n>`

If v1 is single workload only, `workloads` can be an alias to the default workload.

### instances
- `vt instances list`
- `vt instances describe <id>`
- `vt instances restart <id|--all>`
- `vt instances exec <id> -- <cmd...>`
- `vt instances ssh <id>` (if supported)

### secrets
Runtime variables are stored per app and environment and delivered via reconciliation into the fixed runtime file format.

- `vt secrets set KEY=VALUE [KEY2=VALUE2...]`
- `vt secrets unset KEY [KEY2...]`
- `vt secrets import --from <file|stdin>`
- `vt secrets list` (keys only)
- `vt secrets render` (shows delivered file structure, values redacted)
- `vt secrets status` (delivery state, last revision)
- `vt secrets confirm --none` (satisfies the runtime config gate)

Rules:
- Values never print by default.
- `render` is safe by default and redacts values.

### endpoints
L4 endpoints, IPv6 default. Dedicated IPv4 is explicit. Proxy Protocol v2 is per endpoint.

- `vt endpoints list`
- `vt endpoints expose --port 443 --target-port 8080 --proto tcp`
- `vt endpoints expose --port 443 --target-port 8080 --proto tcp --proxy-proto v2`
- `vt endpoints expose --port 443 --target-port 8080 --proto tcp --ipv4 dedicated`
- `vt endpoints update <id> ...`
- `vt endpoints unexpose <id>`
- `vt endpoints describe <id>`
- `vt endpoints wait <id>`

### volumes
- `vt volumes list`
- `vt volumes create --size 10gb`
- `vt volumes attach <vol-id> --mount /data`
- `vt volumes detach <vol-id>`
- `vt volumes delete <vol-id>`

### snapshots
- `vt snapshots list`
- `vt snapshots create --volume <vol-id>`
- `vt snapshots restore <snap-id> --to-volume <vol-id|new>`
- `vt snapshots delete <snap-id>`

### logs
- `vt logs tail`
- `vt logs tail --instance <id>`
- `vt logs query --since 1h`
- `vt logs query --release <id>`

### events
Events are the primary way to understand reconciliation.

- `vt events tail`
- `vt events tail --release <id>`
- `vt events query --since 30m`

### describe
Deep inspection with desired vs current and conditions.

- `vt describe app <id>`
- `vt describe release <id>`
- `vt describe endpoint <id>`
- `vt describe volume <id>`
- `vt describe instance <id>`

### wait
Wait for convergence on a resource.

- `vt wait release <id> --timeout 5m`
- `vt wait deploy --timeout 10m`
- `vt wait endpoint <id> --timeout 2m`

### auth
- `vt auth login`
- `vt auth log

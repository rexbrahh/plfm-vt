
# docs/specs/manifest/manifest-schema.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
This document defines the schema and validation rules for the platform manifest file.

The manifest is the user-authored, versioned runtime configuration that pairs with an OCI image to form an immutable Release.

Locked decision: artifact is OCI image plus manifest. See `docs/adr/0002-artifact-oci-image-plus-manifest.md`.

## Scope
This spec defines:
- file format and schema
- validation rules and defaults
- merge and override rules (env vs process)
- what is runtime-relevant vs CLI-only

This spec does not define:
- routing objects and hostname ownership (see networking specs, Routes are first-class)
- scheduler-to-agent wire format (see `docs/specs/workload-spec.md`)
- secret at-rest encryption (see secrets specs)

## File format
- Format: TOML
- Encoding: UTF-8
- File name: `<platform>.toml` (canonical). The CLI may accept `--manifest` to override.
- Comments are allowed and ignored by parsers.
- Unknown fields are rejected by default (strict parsing).

## Versioning
Top-level field:
- `schema_version` (string, required)
  - v1 value: `"v1"`

Compatibility rule:
- v1 parsers must reject manifests with unknown `schema_version`.
- v1 parsers must reject unknown fields unless explicitly marked as extension fields.

## Top-level schema (v1)

### Required
- `schema_version = "v1"`

### Optional, recommended
- `app.name` (string)
- `image.ref` (string)

### Top-level tables
- `[app]`
- `[image]`
- `[env]`
- `[processes.<process_name>]` (one or more)
- `[[volumes]]` (optional)

## `[app]`
Fields:
- `name` (string, optional)
  - human-readable name
  - does not define identity in the control plane
- `description` (string, optional)

Validation:
- `name` length 1..64
- `description` length 0..256

## `[image]`
Fields:
- `ref` (string, optional)
  - OCI registry reference, tag allowed for convenience
  - examples:
    - `"ghcr.io/acme/myapp:latest"`
    - `"ghcr.io/acme/myapp@sha256:..."`
- `pull_policy` (string, optional)
  - `"if_not_present"` (default)
  - `"always"`

Release immutability rule:
- A Release stored by the control plane always pins an image digest.
- If `image.ref` contains a tag, the CLI and control plane resolve it to a digest at deploy time.

Validation:
- `pull_policy` must be one of the allowed values.
- If `image.ref` includes a digest, it must be sha256-based OCI digest.

## `[env]` (environment-level runtime configuration)
This table defines environment-wide configuration shared by all process types.

Fields:
- `vars` (table of string -> string, optional)
  - environment variables injected into every process
- `workdir` (string, optional)
  - default working directory for processes that do not override
- `user` (string, optional)
  - default user inside guest, if processes do not override
- `prestart` (array of strings, optional)
  - command to run before each process entrypoint
  - this is executed inside the guest by platform init
  - it must reference an executable available in the image
- `prestart_timeout_seconds` (int, optional, default 30)

Merge rules:
- Process-level vars override env-level vars on key collision.
- Process-level workdir and user override env-level defaults.
- If both env and process specify prestart, env prestart runs first, then process prestart.

Validation:
- `vars` keys must match `[A-Z_][A-Z0-9_]*`
- values must be strings (no implicit numbers or booleans)

## `[processes]`
A manifest must define at least one process type.

Process type names:
- TOML table keys under `[processes.<name>]`
- Allowed pattern: `[a-z][a-z0-9-]{0,31}`
- Reserved names (v1): none, but do not use `default` unless you intend it to be the primary process.

### `[processes.<name>]` fields (v1)
Required:
- `resources.memory` (string)

Optional:
- `command` (array of strings)
  - overrides image entrypoint
  - first element is executable path
- `workdir` (string)
- `user` (string)
- `vars` (table string -> string)
- `prestart` (array of strings)
- `prestart_timeout_seconds` (int, default 30)
- `restart.policy` (string)
- `restart.max_retries` (int)
- `restart.backoff_seconds` (int)
- `resources.cpu` (float)
- `resources.disk` (string)
- `scaling.min` (int)
- `scaling.max` (int)
- `scaling.autoscale` (not supported in v1, reject if present)
- `[[processes.<name>.ports]]`
- `[processes.<name>.health]`
- `[[processes.<name>.mounts]]`
- `secrets.required` (bool)

#### `resources`
`resources.memory` (required):
- string with binary units: `Mi`, `Gi`
- examples: `"256Mi"`, `"2Gi"`

`resources.cpu` (optional, default 1.0):
- float representing requested vCPU share
- CPU is a soft resource and may be oversubscribed at placement time

`resources.disk` (optional, default `"4Gi"`):
- ephemeral scratch disk size for the instance
- does not create persistent storage

Validation:
- memory must be >= `64Mi`
- cpu must be > 0 and <= 64 (cap is arbitrary, but should be bounded)
- disk must be >= `1Gi` if set

#### `scaling`
`scaling.min` (optional):
- default rule:
  - if there is exactly one process type, default min = 1
  - otherwise default min = 0

`scaling.max` (optional):
- default: equal to min
- max must be >= min

v1 note:
- autoscaling is not supported. Any autoscale fields must be rejected.

#### `restart`
`restart.policy` (optional, default `"always"`):
- `"always"`
- `"on-failure"`
- `"never"`

`restart.max_retries`:
- only valid when policy is `"on-failure"`
- default 3

`restart.backoff_seconds`:
- default 2
- exponential backoff may be applied by agent, but policy is controlled here

#### `ports`
Each process may declare zero or more listening ports.

Each port entry:
- `name` (string, optional)
  - if omitted, a name is derived: `p<internal>`
- `internal` (int, required)
  - port inside the microVM
- `protocol` (string, optional, default `"tcp"`)
  - v1 allowed: `"tcp"`

Validation:
- `internal` must be 1..65535
- Port names must match `[a-z][a-z0-9-]{0,31}`

Important rule:
- Routes and ingress bindings must reference ports declared here (by name or number). If a route targets an undeclared port, it is rejected by control plane.

#### `health`
Health checks gate readiness and routing.

Fields:
- `type` (string, optional)
  - `"tcp"` (default if health exists)
  - `"http"` (allowed, optional in v1)
- `port` (string or int, required)
  - port name declared in `ports`, or port number
- `path` (string, required for http)
- `interval_seconds` (int, default 10)
- `timeout_seconds` (int, default 2)
- `grace_period_seconds` (int, default 10)
- `success_threshold` (int, default 1)
- `failure_threshold` (int, default 3)

Validation:
- `port` must resolve to a declared port
- `http` health checks must not assume TLS termination at the edge
- TCP health checks should target the internal port and consider the guest network

#### `mounts`
Mounts connect named volumes to a process.

Each mount entry:
- `volume` (string, required)
  - references a `[[volumes]]` entry by name
- `path` (string, required)
  - absolute path inside microVM
- `read_only` (bool, default false)

Validation:
- `path` must be absolute and must not be:
  - `/proc` or under `/proc`
  - `/sys` or under `/sys`
  - `/dev` or under `/dev`
  - the reserved secrets path (see below)

#### `secrets`
`secrets.required` (optional, default false):
- If true, the platform must refuse to start instances for this process unless the environment has a secret bundle configured.
- This does not change the secrets delivery mechanism. It only enforces presence.

Reserved secrets path (v1, fixed):
- `/run/secrets/platform.env`
- This is not configurable in v1.

## `[[volumes]]`
Defines persistent local volumes. Volumes are local to a host and are backed up asynchronously.

Fields:
- `name` (string, required)
- `size` (string, required)
- `filesystem` (string, optional, default `"ext4"`)
- `backup_enabled` (bool, optional, default true)

Validation:
- `name` must match `[a-z][a-z0-9-]{0,31}`
- `size` must be >= `"1Gi"`
- `filesystem` allowed in v1: `"ext4"` only
- `[[volumes]]` names must be unique

Semantics:
- Volume creation and attachment are controlled operations, represented as control plane events.
- The scheduler must respect volume locality. A process requiring a volume can only run where the volume exists.

## Defaults and canonicalization
The CLI provides:
- `platform validate` which applies defaults and validates all rules.
- `platform fmt` which writes a canonical representation.

Canonicalization goals:
- stable ordering of tables and keys
- stable formatting of arrays and tables
- stable derived default values (like port names)

The control plane must validate manifests identically to the CLI.

## Examples

### Minimal single-process web service
```toml
schema_version = "v1"

[image]
ref = "ghcr.io/acme/hello:latest"

[env.vars]
RUST_LOG = "info"

[processes.web]
command = ["./server"]
[processes.web.resources]
cpu = 1.0
memory = "512Mi"

[[processes.web.ports]]
name = "http"
internal = 3000
protocol = "tcp"

[processes.web.health]
type = "tcp"
port = "http"
interval_seconds = 10
timeout_seconds = 2
grace_period_seconds = 10

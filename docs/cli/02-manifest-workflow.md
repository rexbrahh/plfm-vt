````md
# Manifest workflow

This platform is manifest-first. Every deploy is driven by a manifest file in your app directory.

The manifest is the declarative desired state for your app’s runtime configuration, build inputs, and operational intent. If it is in the manifest, it is eligible to be applied on deploy.

## File name and location

The CLI looks for a manifest in the current directory:

- `vt.toml` (recommended)

You can also override with a flag:

- `vt deploy --manifest path/to/vt.toml`

## What belongs in the manifest

Put in the manifest things you want to be reproducible across machines and environments:

- build configuration (Dockerfile, context, image reference)
- runtime resources (cpu, memory)
- run command and args (optional)
- exposed internal ports and endpoint intent (optional, but recommended)
- volume mounts (mount points and volume references)

Do not put secrets in the manifest. Runtime variables and secrets are managed via `vt secrets` and are scoped to app + env.

## Required vs minimal

The manifest must exist, but it can be minimal.

Minimal means:
- enough information for the CLI to build or resolve an image
- enough information to run the container (defaults may apply)

Everything else is optional and can be added incrementally.

## Manifest schema

The examples below use TOML.

### Minimal example (Dockerfile build)

```toml
manifest_version = 1

[app]
name = "hello"

[build]
type = "dockerfile"
context = "."
dockerfile = "Dockerfile"
````

### Minimal example (prebuilt image)

```toml
manifest_version = 1

[app]
name = "hello"

[build]
type = "image"
image = "ghcr.io/acme/hello:1.2.3"
```

## Build configuration

### Dockerfile build

```toml
[build]
type = "dockerfile"
context = "."
dockerfile = "Dockerfile"
```

Recommended conventions:

* Keep the build context small.
* Pin base images for reproducibility.
* Prefer multi-stage builds.

### Prebuilt image

```toml
[build]
type = "image"
image = "ghcr.io/acme/hello:1.2.3"
```

Use this when:

* you build in CI and push to a registry
* you want a fully reproducible artifact independent of local tooling

### Optional host build steps (v1)

v1 may support host-side build steps that run before the container build. These steps run on the build host, not in the running workload.

Example:

```toml
[build]
type = "dockerfile"
context = "."
dockerfile = "Dockerfile"

[build.hooks]
prebuild = [
  "pnpm install --frozen-lockfile",
  "pnpm build"
]
```

Rules:

* Hooks must be deterministic, fast, and safe to rerun.
* Hooks should not rely on secrets from the manifest.
* If hooks need secrets, use CI secrets and bake artifacts into the image.

## Runtime configuration

### Resources

```toml
[resources]
cpu = "shared-1x"   # example enum, actual values are platform-defined
memory = "512mb"
```

Notes:

* Resources are part of desired state and should be applied consistently on deploy.
* If you later support per-env overrides, they should not break determinism.

### Command and args (optional)

If your image has a good default `CMD`, you can omit this.

```toml
[run]
command = ["./server"]
args = ["--port", "8080"]
```

## Networking in the manifest

The platform is L4-first. The CLI and control plane should avoid HTTP assumptions.

There are two ways to manage exposure:

1. Declaratively in the manifest (recommended for reproducibility)
2. Imperatively via `vt endpoints ...`

### Internal ports

Declare which ports your workload listens on.

```toml
[[ports]]
internal = 8080
protocol = "tcp"
```

### Endpoint intent (optional)

If you want `vt deploy` to reconcile endpoints as part of applying desired state, declare endpoint intent.

```toml
[[endpoints]]
listen_port = 443
target_port = 8080
protocol = "tcp"

# Optional knobs
proxy_protocol = "v2"   # enable Proxy Protocol v2
ipv4 = "dedicated"      # requires IPv4 add-on
```

Notes:

* IPv6 is the default for endpoints.
* Dedicated IPv4 is an explicit add-on and should be called out in CLI output.

If you prefer imperative networking, omit `[[endpoints]]` and use:

* `vt endpoints expose ...`
* `vt endpoints update ...`
* `vt endpoints unexpose ...`

## Storage in the manifest

Volumes are persistent resources that may be created and managed via CLI. The manifest can declare mount intent.

```toml
[[mounts]]
volume = "data"     # logical name or volume reference
path = "/data"
```

Typical workflow:

1. Create a volume: `vt volumes create ...`
2. Add mount intent to manifest
3. Deploy to apply mounts

v1 should be explicit if mounts cannot be applied without an existing volume.

## Secrets and runtime variables (not in the manifest)

Secrets are managed via CLI and delivered to the runtime via reconciliation into a fixed file format. The manifest must not embed secret values.

Commands:

* `vt secrets set KEY=VALUE`
* `vt secrets unset KEY`
* `vt secrets import --from <file|stdin>`
* `vt secrets list` (keys only)
* `vt secrets render` (structure, redacted by default)
* `vt secrets status`
* `vt secrets confirm --none`

### Required runtime config gate before creating a release

Before a release can be created (and before `deploy` can proceed), you must satisfy exactly one of:

* Set or import at least one runtime variable:

  * `vt secrets set ...`
  * `vt secrets import ...`
* Explicitly confirm there are no runtime variables:

  * `vt secrets confirm --none`

This is a deliberate guardrail against accidental missing configuration. It is an acknowledgement gate, not a fake required variable.

## End to end workflow

### 1) Create an app and manifest

```bash
vt launch
# or
vt launch --no-deploy
```

This should:

* create the remote app
* write a minimal `vt.toml`
* choose a default environment if not specified

### 2) Configure runtime variables (or confirm none)

```bash
vt secrets set DATABASE_URL=...
# or
vt secrets confirm --none
```

### 3) Create a release

```bash
vt releases create
```

This should:

* validate manifest
* build or resolve the image
* create an immutable release id
* print a mutation receipt with the release id and next steps

### 4) Deploy the release

```bash
vt deploy --wait
```

Deploy should:

* promote the chosen release to the selected env
* apply manifest-declared intents that are part of v1 (resources, mounts, endpoints)
* optionally wait for convergence

### 5) Observe

```bash
vt status
vt events tail
vt logs tail
```

## Updating the manifest

Edits to the manifest do nothing until you create a new release and deploy it.

Typical loop:

1. edit `vt.toml`
2. `vt releases create`
3. `vt deploy --wait`

Because releases are immutable, a manifest change always results in a new release.

## CI friendly usage

Recommended CI flow:

* build and push image in CI
* set `build.type = "image"` with a pinned tag or digest
* create release and deploy from CI

Example:

```bash
vt secrets confirm --none
vt releases create
vt deploy --wait
```

If CI needs runtime variables:

* use `vt secrets import --from -` and pipe from CI secret store
* never commit secrets into the repo

## Troubleshooting

### “No manifest found”

* Run `vt launch` in the app directory, or add `vt.toml`.

### “Runtime config not confirmed”

* Run `vt secrets set ...` or `vt secrets confirm --none`.

### “Deploy finished but app is not healthy”

Remember the system converges asynchronously.

* `vt events tail` to see reconcile progress and failure reasons
* `vt describe release <id>` and `vt describe endpoint <id>` for desired vs current
* `vt logs tail` for runtime errors

### “Endpoint created but not reachable”

* Confirm the workload is listening on the declared internal port.
* Check endpoint status and conditions:

  * `vt endpoints describe <id>`
* If you requested IPv4, confirm the IPv4 add-on is active for that endpoint.

```
```

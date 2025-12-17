````md
# Templates and examples

This document is a grab bag of copy paste templates and real workflows for common tasks.

Conventions:
- CLI binary: `vt`
- Manifest file: `vt.toml`
- Secrets are managed via `vt secrets ...` and are scoped to app + env
- Endpoints are L4 (TCP or UDP). IPv6 is default. Dedicated IPv4 is an add-on.
- Releases are immutable. Manifest changes take effect only after a new release is created and deployed.

## Quickstart templates

### 1) Minimal Dockerfile app

`vt.toml`:
```toml
manifest_version = 1

[app]
name = "hello"

[build]
type = "dockerfile"
context = "."
dockerfile = "Dockerfile"

[resources]
cpu = "shared-1x"
memory = "512mb"

[[ports]]
internal = 8080
protocol = "tcp"
````

Commands:

```bash
vt auth login
vt launch --no-deploy

vt secrets confirm --none
vt releases create
vt deploy --env prod --wait

vt status
vt logs tail
```

### 2) Prebuilt image app (CI builds the image)

`vt.toml`:

```toml
manifest_version = 1

[app]
name = "hello"

[build]
type = "image"
image = "ghcr.io/acme/hello:1.2.3"

[resources]
cpu = "shared-1x"
memory = "512mb"

[[ports]]
internal = 8080
protocol = "tcp"
```

Commands:

```bash
vt secrets confirm --none
vt releases create
vt deploy --env prod --wait
```

Tip: prefer a pinned digest when possible for true immutability.

## Secrets and runtime config patterns

### Set a few variables safely

```bash
vt secrets set --env prod DATABASE_URL='...' REDIS_URL='...'
vt secrets status --env prod
```

### Import from a local dotenv file

If you have `.env.prod`:

```bash
vt secrets import --env prod --from .env.prod
```

### Import from stdin (CI friendly)

```bash
printf 'DATABASE_URL=%s\nREDIS_URL=%s\n' "$DATABASE_URL" "$REDIS_URL" \
  | vt secrets import --env prod --from -
```

### Confirm no runtime variables

This satisfies the release gate when your app genuinely needs none:

```bash
vt secrets confirm --env prod --none
```

### Render the delivered secrets file format (redacted)

Use this when debugging what the runtime is actually receiving:

```bash
vt secrets render --env prod
```

## Endpoints and networking examples (L4)

You can manage endpoints declaratively in the manifest or imperatively via `vt endpoints`.

### A) Declarative endpoint intent in the manifest

`vt.toml`:

```toml
[[ports]]
internal = 8080
protocol = "tcp"

[[endpoints]]
listen_port = 443
target_port = 8080
protocol = "tcp"
```

Deploy:

```bash
vt deploy --env prod --wait
vt endpoints list --env prod
```

### B) Imperative endpoint creation

Expose 443 externally to 8080 internally:

```bash
vt endpoints expose --env prod --port 443 --target-port 8080 --proto tcp --wait
```

Enable Proxy Protocol v2:

```bash
vt endpoints expose --env prod --port 443 --target-port 8080 --proto tcp --proxy-proto v2 --wait
```

Request dedicated IPv4 (add-on):

```bash
vt endpoints expose --env prod --port 443 --target-port 8080 --proto tcp --ipv4 dedicated --wait
```

Describe endpoint state (desired vs current, conditions):

```bash
vt endpoints describe <endpoint-id> --env prod
```

## Volumes and persistence examples

### Create and attach a volume

```bash
vt volumes create --env prod --size 10gb
vt volumes list --env prod
```

Add a mount intent to the manifest:

`vt.toml`:

```toml
[[mounts]]
volume = "data"
path = "/data"
```

Deploy to apply mounts:

```bash
vt releases create
vt deploy --env prod --wait
```

### Snapshot and restore

Create a snapshot:

```bash
vt snapshots create --env prod --volume <volume-id>
vt snapshots list --env prod
```

Restore to a new volume (recommended default):

```bash
vt snapshots restore --env prod <snapshot-id> --to-volume new
```

## Release workflows

### Create a release without deploying

This is useful for review, promotion, or CI pipelines.

```bash
vt releases create --env staging
vt releases list
vt releases describe <release-id>
```

### Promote the same release to another environment

Recommended flow:

1. Deploy to staging
2. Promote the exact release id to prod

```bash
vt releases promote <release-id> --env prod --wait
```

### Rollback

Rollback usually means “promote an earlier release”:

```bash
vt releases rollback --env prod --to <release-id> --wait
```

## Observability and debugging recipes

### The standard “something is stuck” loop

```bash
vt status --env prod
vt events tail --env prod
vt describe release <release-id> --env prod
vt logs tail --env prod
```

### If deploy was run with --wait and timed out

A wait timeout is not necessarily a failure. Inspect convergence:

```bash
vt events tail --env prod
vt status --env prod
```

### Diagnose local environment and connectivity

```bash
vt doctor
vt doctor --json
```

## Scripting examples (JSON output and receipts)

### Extract a release id from `releases create`

Example pattern (exact JSON fields are platform-defined, but this shows the intent):

```bash
rel_id="$(vt releases create --env prod --json | jq -r '.receipt.release.id')"
vt releases describe "$rel_id" --json | jq .
```

### Idempotent deploy script skeleton

```bash
set -euo pipefail

vt auth status >/dev/null

vt secrets status --env prod >/dev/null || {
  echo "Missing secrets state for prod"
  exit 5
}

rel_id="$(vt releases create --env prod --json | jq -r '.receipt.release.id')"

vt deploy --env prod --release "$rel_id" --wait --wait-timeout 10m

vt status --env prod --json | jq .
```

### Prefer IDs over names in automation

Names can be renamed. IDs should be stable. When a command prints IDs in a receipt, scripts should capture and reuse them.

## Common app patterns

### HTTP API (still L4)

Even if your app is HTTP, treat it as TCP in v1:

* expose a TCP endpoint
* your app owns TLS termination if needed
* do not assume HTTP routing exists

### Worker (no endpoint)

A worker may have no externally exposed ports:

* omit `[[endpoints]]`
* omit `[[ports]]` unless needed internally
* use logs and events for monitoring

### Multi-process or sidecars

If v1 does not support multiple workloads per app, keep it simple:

* one container per app instance
* use managed external services for extras
  If v1 does support multiple workloads later, this document will add a dedicated section.

## CI examples (patterns)

### CI prerequisites

* Use an automation token, not interactive login
* Use `--no-input` to prevent prompts
* Use `--json` for parsing
* Make secrets delivery explicit (`secrets import` or `confirm --none`)

Example flow:

```bash
vt deploy --env prod --no-input --json
```

If the deploy is blocked by the runtime config gate, fix it in CI by importing:

```bash
printf 'KEY=%s\n' "$KEY" | vt secrets import --env prod --from - --no-input
```

## Troubleshooting by symptom

### “I changed the manifest but nothing changed”

You must create and deploy a new release:

```bash
vt releases create
vt deploy --env prod --wait
```

### “My endpoint exists but connections fail”

* verify your container is listening on the internal port
* inspect endpoint conditions:

```bash
vt endpoints describe <endpoint-id> --env prod
vt logs tail --env prod
```

### “My deploy succeeded but the app is unhealthy”

Look at reconciliation and runtime:

```bash
vt events tail --env prod
vt logs tail --env prod
vt status --env prod
```

```
```

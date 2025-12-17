````md
# Debug tooling

This platform is reconciliation-based. Most mutations record desired state and then converge asynchronously. Debugging is therefore primarily about answering:

- What is the desired state?
- What is the current state?
- Why is there a gap?
- What changed recently?

The CLI provides a small set of introspection primitives that work across all resources.

## The core primitives

You can debug almost anything with these commands:

- `status` shows a compact desired vs current summary for an app and env.
- `events` shows what the control plane is doing and why.
- `describe` shows full desired/current state and conditions for any resource.
- `logs` shows runtime output from workloads.
- `wait` blocks until a resource converges or times out.
- `secrets status` and `secrets render` show runtime config delivery state.
- `doctor` validates local setup and control plane connectivity.

When a mutation command prints a receipt containing IDs, use those IDs for follow up. IDs are more reliable than names.

## Common flags (recommended)

Most introspection commands support:

- `--app <name|id>`
- `--env <name|id>`
- `--org <name|id>`
- `--project <name|id>`
- `--json` (machine readable)
- `--since <duration>` (queries)
- `--limit <n>` (queries)
- `--follow` or `tail` (streaming)

For scripts, prefer:
- `--json`
- explicit `--app` and `--env` when not in an app directory

## Status

### `vt status`

Use `status` first. It is the fastest way to see if the system is converging.

Typical output should include:
- desired release id and current release id
- desired instance count and running count
- endpoint summary (created, provisioning, ready, error)
- last reconcile time
- last reconcile error (if any)

Examples:
```bash
vt status --env prod
vt status --env prod --json
````

## Events

### Why events are the main debugging tool

In a reconciliation system, the reason something is not working is often not in logs. It is in a control plane decision, a dependency not ready yet, or a failed reconcile step.

Events should answer:

* what controller acted
* on which object
* what it tried to do
* what happened
* why it failed (if it failed)

### `vt events tail`

Stream events in real time:

```bash
vt events tail --env prod
vt events tail --env prod --release <release-id>
vt events tail --env prod --endpoint <endpoint-id>
```

### `vt events query`

Look back in time:

```bash
vt events query --env prod --since 30m
vt events query --env prod --since 2h --limit 200
```

What to look for:

* repeated retries on the same step (image pull, scheduling, endpoint provision)
* oscillations (ready then not ready)
* dependencies failing (volume attach, secret delivery, endpoint readiness)
* permission or quota errors

## Describe

### `vt describe`

`describe` is the ground truth view for a specific object. It should include:

* stable id and human name
* desired spec and current observed state
* conditions (Ready, Reconciling, Degraded, etc)
* last transition timestamps
* recent events summary or pointers
* references to related resources (release, instances, endpoints, volumes)

Examples:

```bash
vt describe app <app-id> --env prod
vt describe release <release-id> --env prod
vt describe endpoint <endpoint-id> --env prod
vt describe volume <volume-id> --env prod
vt describe instance <instance-id> --env prod
```

If you do not have an ID yet:

```bash
vt releases list --env prod
vt endpoints list --env prod
vt volumes list --env prod
vt instances list --env prod
```

### Reading conditions

A good rule:

* If `Ready` is false, find the condition reason and message.
* If `Reconciling` is true for a long time, inspect events.
* If `Degraded` is true, inspect events and logs.

## Logs

### `vt logs tail`

Use runtime logs to debug application errors and crash loops.

```bash
vt logs tail --env prod
vt logs tail --env prod --instance <instance-id>
```

If supported, narrow by release:

```bash
vt logs query --env prod --release <release-id> --since 30m
```

Recommendations:

* Start with `events tail` and `status` first for platform level failures.
* Use logs for application level failures (crashes, misconfig, bind failures, migrations).

## Wait

### `vt wait`

`wait` blocks until a resource converges or a timeout occurs. It does not prove correctness, it proves convergence to a reported state.

Examples:

```bash
vt wait release <release-id> --env prod --timeout 10m
vt wait endpoint <endpoint-id> --env prod --timeout 5m
```

If a wait times out:

* treat it as a transient failure
* inspect with:

  * `vt events tail`
  * `vt status`
  * `vt describe <resource> <id>`

## Secrets introspection

Secrets and runtime variables are scoped to app + env. They are delivered to the runtime as a fixed format file via reconciliation.

### `vt secrets status`

Shows delivery state and last applied revision.

```bash
vt secrets status --env prod
```

Look for:

* whether the env config gate is satisfied
* last revision id and timestamp
* last delivery success or failure
* which workloads have applied the latest revision (if available)

### `vt secrets render`

Shows the rendered secrets file structure as the runtime would see it. Values are redacted by default.

```bash
vt secrets render --env prod
```

Use this when:

* the app cannot find configuration it expects
* you suspect a key mismatch
* you need to confirm the file path and structure used by the runtime

Never rely on `render` to verify actual values. It is a shape and delivery debugging tool.

## Workload and instance introspection

### `vt instances list` and `vt instances describe`

Use these to debug scheduling, restarts, and per instance failures.

```bash
vt instances list --env prod
vt instances describe <instance-id> --env prod
```

A useful `describe` output for an instance includes:

* current state (starting, running, exited, restarting)
* exit code and reason (if exited)
* last start time
* resource allocation and limits
* attached volumes and mounts
* applied secrets revision
* network bindings and ports (internal)

### In situ debugging (optional)

If v1 supports it:

* `vt instances exec <id> -- <cmd...>`
* `vt instances ssh <id>`

Use these sparingly. Prefer `events`, `describe`, and `logs` first.

## Endpoint debugging

Endpoints are L4. IPv6 is default. Dedicated IPv4 is an explicit add-on. Proxy Protocol v2 is optional.

### Key commands

```bash
vt endpoints list --env prod
vt endpoints describe <endpoint-id> --env prod
vt wait endpoint <endpoint-id> --env prod --timeout 5m
```

Common failure patterns:

* endpoint provisioning not complete yet (watch `events tail`)
* workload not listening on the target internal port (check logs, instance describe)
* IPv4 requested but add-on not active (describe should show add-on state)
* Proxy Protocol enabled but application not expecting it (application sees corrupted first bytes)

Debug loop:

```bash
vt endpoints describe <endpoint-id> --env prod
vt events tail --env prod --endpoint <endpoint-id>
vt instances describe <instance-id> --env prod
vt logs tail --env prod
```

## Volume and snapshot debugging

### Volumes

```bash
vt volumes list --env prod
vt volumes describe <volume-id> --env prod
```

### Snapshots

```bash
vt snapshots list --env prod
vt snapshots describe <snapshot-id> --env prod
```

Common failure patterns:

* mount path mismatch between manifest and app expectations
* attach not yet converged (events show waiting on placement or attach)
* restore created a new volume but manifest still points to old logical mount name

## Doctor

### `vt doctor`

`doctor` is for local and connectivity debugging, not app debugging.

It should check:

* CLI version and API compatibility
* authentication validity
* control plane reachability
* local build prerequisites (if using local Dockerfile builds)
* obvious misconfigurations (missing manifest, missing env selection)

Examples:

```bash
vt doctor
vt doctor --json
```

## Debug recipes

### Deploy appears to do nothing

1. Check target selection:

```bash
vt status --env prod
```

2. Watch control plane activity:

```bash
vt events tail --env prod
```

3. Inspect the release:

```bash
vt releases list --env prod
vt describe release <release-id> --env prod
```

### Deploy is stuck in progress

1. `status` to see the gap:

```bash
vt status --env prod
```

2. Find the last failing step:

```bash
vt events tail --env prod
```

3. Inspect the resource that is not converging:

```bash
vt describe release <release-id> --env prod
vt describe endpoint <endpoint-id> --env prod
vt describe volume <volume-id> --env prod
```

### App is crash looping

1. Confirm restarts at instance level:

```bash
vt instances list --env prod
vt instances describe <instance-id> --env prod
```

2. Check logs:

```bash
vt logs tail --env prod --instance <instance-id>
```

3. Verify secrets delivery:

```bash
vt secrets status --env prod
vt secrets render --env prod
```

### Endpoint exists but is unreachable

1. Check endpoint conditions:

```bash
vt endpoints describe <endpoint-id> --env prod
```

2. Check events for provisioning failures:

```bash
vt events tail --env prod --endpoint <endpoint-id>
```

3. Confirm the workload is listening:

```bash
vt logs tail --env prod
vt instances describe <instance-id> --env prod
```

4. If Proxy Protocol v2 is enabled, confirm the app expects it.

### Secrets set but app still sees old config

1. Verify the secrets revision and delivery status:

```bash
vt secrets status --env prod
```

2. Check instance applied revision:

```bash
vt instances describe <instance-id> --env prod
```

3. Watch events for delivery failures:

```bash
vt events tail --env prod
```

## Using JSON output in debugging

Prefer IDs and structured fields:

```bash
vt status --env prod --json | jq .
vt events query --env prod --since 30m --json | jq .
vt describe release <release-id> --env prod --json | jq .
```

If the platform provides `trace_id` in errors, log it in tickets and incident reports.

```
```

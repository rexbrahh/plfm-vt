# Core loop demo (request-id, idempotency, read-your-writes)

This demo validates the control-plane “core loop hardening” behaviors end-to-end:
- `X-Request-Id` is always present on responses.
- `Idempotency-Key` safely replays successful writes.
- Write endpoints wait for projections and return materialized state (RYW).

## Prereqs

- `docker` (with `docker compose`)
- `just`
- `curl`
- `jq`

## Auth

The demo uses the dev auth stub (`Authorization: Bearer user:<email>`). Override as needed:

```bash
export VT_AUTHORIZATION="Bearer user:demo@example.com"
```

## Run

```bash
# Start postgres (and wipe any prior dev DB volume)
scripts/dev/demo-core-loop.sh --reset

# Or reuse your existing dev DB volume
scripts/dev/demo-core-loop.sh
```

## What it does

1. Starts the dev Postgres container (`just dev-up`).
2. Starts `plfm-control-plane` in dev mode (`GHOST_DEV=1`) and waits for `GET /healthz`.
3. Enrolls a demo node so the scheduler can allocate instances.
4. Creates:
   - org (with idempotency replay)
   - app
   - env
   - release (with idempotency replay)
   - deploy
   - scale update (GET then PUT `/scale`)
   - instances list (waits for scheduler allocations)
   - route (with idempotency replay)
5. Prints projection checkpoints via `GET /v1/_debug/projections`.

## Notes

- The demo generates unique names/keys per run, so it’s safe to re-run without conflicts.
- Use `--keep-running` to leave the control-plane process running after the demo.

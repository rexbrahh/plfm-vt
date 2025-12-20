# Development environment

This guide standardizes local development so contributors can build, run, and test reliably.

## Supported development modes

Mode A: Local dev stack
- Run control-plane + node-agent + ingress in a local environment.
- Intended for feature work and fast iteration.

Mode B: Remote dev cluster
- Run only CLI and frontend locally.
- Connect to a shared dev cluster for runtime-heavy work (Firecracker, networking).

## Host OS support

- Linux: full support, including VM runtime and networking features.
- macOS: supported for CLI, frontend, and most control-plane work. Runtime testing should use remote dev cluster or a Linux VM.

If you are on macOS and need runtime work:
- Use a Linux VM with KVM support, or use remote dev cluster.

## Toolchain

Required:
- git
- container runtime (Docker or compatible)
- a task runner (recommended: `just`)

Recommended (project standard):
- Nix (for pinned toolchain and reproducibility)
- Rust and Node.js toolchains if you are not using Nix

## One command setup (recommended)

If the repo provides a Nix flake:
- `nix develop`
- `just bootstrap`
- `just test`

If Nix is not available in your environment:
- `./scripts/bootstrap/dev.sh`
- Then follow the printed instructions.

## Local dev stack

The dev stack should provide:
- control-plane API
- a single node-agent
- ingress (L4)
- a local event/log stream for debugging
- optional: a minimal web console

Standard commands (define these in `justfile`):
- `just dev-up`       Bring up the stack
- `just dev-down`     Tear down the stack
- `just dev-logs`     Tail logs for all components
- `just dev-reset`    Wipe local state and restart
- `just dev-status`   Health check summary

Expected environment variables:
- `GHOST_REGION=local`
- `GHOST_API_URL=http://127.0.0.1:<port>`
- `GHOST_DEV=1`

## CLI local workflow

The CLI is the primary product surface. Dev expectations:
- Every mutating command prints a receipt:
  - what was requested (desired)
  - current observed state (may lag)
  - how to wait or watch events
- Prefer JSON output for scripting: `--json`
- Prefer deterministic formatting for human output.

Suggested commands:
- `ghostctl auth login --local`
- `ghostctl org list`
- `ghostctl project create ...`
- `ghostctl app create ...`
- `ghostctl env set ...`
- `ghostctl release create --wait`

## Runtime dev notes (Linux)

Firecracker and VM lifecycle work typically require:
- KVM enabled
- correct permissions for `/dev/kvm`
- CPU virtualization enabled in BIOS
- a supported kernel configuration

Common checks:
- `ls -l /dev/kvm`
- `kvm-ok` (if available)
- confirm your user is in the right group for KVM access

## Secrets handling in dev

Rules:
- Never commit real secrets.
- Never print secret values to logs.
- Use `.env.local` or `secrets.local/` ignored by git.
- The secrets delivery mechanism should be exercised end-to-end in dev:
  - control-plane stores encrypted secret bundle
  - node-agent reconciles to a fixed on-disk file format
  - workloads read from the mounted file

## Networking dev notes

Ingress expectations:
- L4 does not terminate connections.
- IPv6-first in all internal and external defaults.
- Dedicated IPv4 is modeled as an explicit add-on.

Overlay networking:
- Prefer a deterministic “dev overlay” mode for local runs.
- Keep wire formats and behavior identical to production.

## Frontend web terminal dev

Web terminal uses libghostty-vt embedded via WASM.
Dev expectations:
- A local terminal session can connect to:
  - a sandbox shell (for frontend-only work)
  - logs stream (for platform introspection)
  - optional: ssh-like session into a workload (when supported)

Standard commands:
- `just web-up`
- `just web-test`
- `just web-build`

## Troubleshooting checklist

- Build fails: run `just clean` then `just build`.
- Tests flaky: confirm you are not sharing state across tests.
- CLI outputs not stable: update golden tests and confirm deterministic ordering.
- Runtime failures on macOS: switch to remote dev cluster for runtime testing.

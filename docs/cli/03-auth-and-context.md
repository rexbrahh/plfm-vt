# Auth and context

The CLI is the primary interface to the platform. Authentication and target selection must be reliable for both humans and scripts.

This document defines:
- how users authenticate
- how sessions and tokens work
- how org, project, app, and env are selected

## Goals

- Secure by default for interactive users
- Non-interactive friendly for CI and scripting
- Explicit and predictable scoping (org, project, app, env)
- No hidden behavior that changes what you are deploying

## Authentication

### Interactive login
Interactive login is intended for developers using a terminal.

Commands:
- `vt auth login`
- `vt auth status`
- `vt auth logout`

Expected behavior:
- `vt auth login` performs an interactive flow and stores credentials locally.
- `vt auth status` prints who you are authenticated as, and whether your session is valid.
- `vt auth logout` removes local credentials.

If a command requires auth and you are not logged in, the CLI should:
- show a clear error
- print one exact command to fix it (`vt auth login`)
- avoid ambiguous retries

### Automation tokens (CI)
Automation should not depend on interactive login.

Commands:
- `vt auth token create`
- `vt auth token revoke <id>`

Recommended behavior:
- `vt auth token create` produces a token suitable for CI usage.
- Tokens are never printed again after creation unless explicitly requested at creation time.
- `vt auth token revoke` invalidates a token.

Use in CI:
- set the token as an environment variable in your CI system
- run commands with `--no-input` and `--json` as needed

## Credential storage

Design requirements:
- Prefer secure OS credential storage when available.
- If a filesystem fallback is used, it must:
  - store credentials in a dedicated path
  - set strict permissions (not world-readable)
  - avoid leaking tokens in logs or debug output

The CLI must never write tokens into the app manifest.

## Sessions and expiration

- Interactive sessions may expire and require re-authentication.
- The CLI may refresh credentials automatically if it can do so securely.
- If refresh fails, the CLI must:
  - fail with an auth error
  - print `vt auth login`

## Context and scoping

Most commands operate on an app and an environment within a project and org.

### Resources
Primary resources for context:
- org
- project
- app
- env

### Selection order
The CLI resolves the target in this order:

1. Explicit flags:
   - `--org`, `--project`, `--app`, `--env`
2. Local manifest in the current directory:
   - used to identify the app (and other app-level intent)
3. Saved local context (defaults):
   - last used org, project, app, env

If the CLI cannot resolve a required target, it fails with a usage error and prints the exact command or flags needed.

### Global flags (recommended)
- `--org <name|id>`
- `--project <name|id>`
- `--app <name|id>`
- `--env <name|id>`
- `--json`
- `--no-input`
- `--yes`

Rules:
- Flags always win over saved context.
- The CLI must print the resolved target in human output for mutation commands.

## Setting and switching context

### Listing available targets
- `vt orgs list`
- `vt projects list`
- `vt apps list`

### Choosing defaults
If convenience commands exist, they should only set defaults and must never mutate remote state.

Example patterns:
- `vt projects use <name|id>`
- `vt apps use <name|id> --env <env>`

If these commands do not exist in v1, the same result must be achievable with flags:
- `vt status --org <...> --project <...> --app <...> --env <...>`

### Environment selection
Environments are explicit and user-chosen (for example: `prod`, `staging`, `preview`).
The CLI must never silently deploy to a different environment than the one resolved.

For safety, consider:
- default env is required for `deploy` and `releases promote`
- if no env is set, fail with a message like:
  - “No environment selected. Use `--env prod` or set a default.”

## Non-interactive behavior

When `--no-input` is set (or when not running in a TTY), the CLI must:
- never open a browser
- never prompt for input
- fail fast if auth or context is missing
- provide actionable error messages

Recommended CI flags:
- `--no-input`
- `--json`
- explicit `--org`, `--project`, `--app`, `--env` if the working directory cannot be relied on

## Security and safety notes

- Do not place secrets in the manifest.
- Avoid passing secrets via command line arguments in shared environments (they may appear in shell history).
- Prefer `vt secrets import --from -` and pipe from a secret store.

## Troubleshooting

### “Not authenticated”
Run:
- `vt auth login`
Then confirm:
- `vt auth status`

### “Permission denied”
Confirm you are in the correct org and project:
- `vt auth status`
- `vt orgs list`
- `vt projects list`

### “Target not selected”
Use explicit flags or set defaults:
- `vt status --org <...> --project <...> --app <...> --env <...>`

### “CI job is hanging”
Ensure you are running with:
- `--no-input`

If the CLI is waiting for convergence, add:
- `--no-wait` (if appropriate), or
- `--wait-timeout <duration>` to bound the wait

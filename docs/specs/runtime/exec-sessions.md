# Exec Sessions (v1)

Status: approved  
Owner: Runtime Team  
Last reviewed: 2025-12-17

## Purpose

Define the end-to-end flow for interactive and non-interactive command execution inside running instances.

This spec is normative for:
- CLI `plfm exec` command
- Web console terminal via libghostty-vt
- Control plane exec session API
- Host agent exec handling
- Guest init exec service

## Goals

Provide secure, auditable command execution inside running instances:
- CLI: `plfm exec <instance> -- <cmd...>`
- Web console: terminal-based exec via libghostty-vt WASM

Exec MUST be secure by default, auditable, and bounded (timeouts, concurrency).

## Non-goals (v1)

- Recording full session transcripts by default
- Cross-instance exec fanout
- Privilege escalation inside the guest
- File transfer via exec (use separate mechanism)

## Terminology

- **Exec session**: a single authorized attempt to attach an interactive (PTY) or non-interactive (pipes) process to a client stream.
- **Exec gateway**: a control-plane component that terminates client TLS and proxies the exec stream to the correct host agent.
- **PTY mode**: session with a pseudo-terminal for interactive shells.
- **Pipe mode**: session with separate stdin/stdout/stderr for scripted commands.

## Security invariants (normative)

1. Exec authorization MUST be explicit and scoped (`instances.exec` permission on target env).
2. Tokens MUST be short-lived (default 60 seconds) and single-use.
3. Exec MUST be fully audit logged (who, what, where, when, outcome).
4. Exec MUST NOT be possible to instances that are not in `Running` state.
5. Exec MUST be bounded:
   - Default max duration: 1 hour
   - Default max concurrent exec sessions per env: 10
   - Default max concurrent exec sessions per instance: 2

## API Surface (Control Plane)

### Create exec session

`POST /v1/exec-sessions`

Request body:
```json
{
  "instance_id": "ulid",
  "command": ["sh", "-lc", "uptime"],
  "tty": true,
  "cols": 120,
  "rows": 34,
  "env": { "TERM": "xterm-256color" },
  "stdin": true
}
```

Response:
```json
{
  "exec_session_id": "ulid",
  "connect_url": "wss://api.<domain>/v1/exec-sessions/<id>/connect",
  "token": "<opaque>",
  "expires_at": "2025-12-17T03:50:00Z"
}
```

Rules:
- `token` MUST expire within 60 seconds.
- `token` MUST be single-use. A second connect attempt with the same token MUST fail.
- `command` MUST be recorded in the audit log (redaction policy is a separate spec).

### Get exec session

`GET /v1/exec-sessions/{id}`

Returns status and outcome (exit code, timestamps).

Response:
```json
{
  "exec_session_id": "ulid",
  "instance_id": "ulid",
  "status": "ended",
  "command": ["sh", "-lc", "uptime"],
  "tty": true,
  "created_at": "2025-12-17T03:49:00Z",
  "connected_at": "2025-12-17T03:49:05Z",
  "ended_at": "2025-12-17T03:49:10Z",
  "exit_code": 0,
  "end_reason": "exited"
}
```

## Eventing (State Model)

The control plane MUST emit:
- `exec_session.requested` - session creation requested
- `exec_session.granted` - session approved and token issued
- `exec_session.connected` - client connected via WebSocket
- `exec_session.ended` - session terminated (includes exit_code, reason)

A session that is granted but never connected MUST transition to ended with reason `connect_timeout`.

Session status values:
- `pending` - requested, not yet granted
- `granted` - token issued, awaiting connection
- `connected` - actively streaming
- `ended` - terminated (check `end_reason`)

End reasons:
- `exited` - process exited normally
- `killed` - process killed by signal
- `timeout` - session duration exceeded
- `connect_timeout` - client never connected
- `client_disconnect` - client closed connection
- `operator_revoked` - admin terminated session

## Connection Flow (Normative)

1. Client calls `POST /v1/exec-sessions`.
2. Control plane validates:
   - Caller has `instances.exec` permission for the target env
   - Instance is in `Running` state
   - Concurrency limits not exceeded
3. Control plane creates exec_session record and emits `exec_session.granted`.
4. Client opens WebSocket to `/v1/exec-sessions/{id}/connect` with the token as query param or header.
5. Exec gateway validates token:
   - Signature or introspection is valid
   - Token not expired
   - Token not previously used (nonce consumption)
   - Token exec_session_id matches URL
6. Exec gateway resolves instance placement to (host_id, agent_endpoint).
7. Exec gateway opens a mutually authenticated stream to host agent and proxies bytes.
8. Host agent connects to the guest exec service over vsock and starts the process.
9. Bidirectional streaming continues until:
   - Client disconnects
   - Session duration exceeded
   - Process exits
   - Operator revokes session

## Data Plane Contract

### Host Agent Responsibilities

- Enforce instance state is `Running` at connect time.
- Connect to guest exec service via vsock port 5162.
- Create PTY if `tty=true`.
- Forward resize events to PTY.
- Forward allowed signals (INT, TERM, KILL).
- Ensure cleanup on disconnect:
  - Send SIGHUP immediately
  - Send SIGTERM after 5 second grace
  - Send SIGKILL after 30 seconds if still running
- Report `exit_code` and termination reason to control plane.

### Guest Init Responsibilities

- Provide an exec service reachable via vsock port 5162.
- Accept exec requests with command, env, tty, cols, rows.
- Spawn requested command with the requested environment.
- For PTY mode:
  - Allocate PTY
  - Attach stdin/stdout/stderr
  - Apply window resize events
- Report process exit to host agent.

## Exec Stream Protocol (v1)

Transport: WebSocket (binary frames).

Frame format:
- Byte 0: frame type
- Bytes 1-N: payload

Frame types:
- `0x01`: stdin bytes (client -> server)
- `0x02`: stdout bytes (server -> client)
- `0x03`: stderr bytes (server -> client)
- `0x10`: JSON control message (bidirectional)
- `0x11`: exit status JSON (server -> client)

### Control Messages

Resize (client -> server):
```json
{ "type": "resize", "cols": 120, "rows": 34 }
```

Signal (client -> server):
```json
{ "type": "signal", "name": "INT" }
```

Allowed signals: `INT`, `TERM`, `KILL`, `HUP`

Client close (client -> server):
```json
{ "type": "close" }
```

### Exit Status (server -> client)

```json
{ "type": "exit", "exit_code": 0, "reason": "exited" }
```

Rules:
- stdout/stderr separation is best-effort. For `tty=true`, stderr MAY be merged into stdout.
- For `tty=false`, stdout and stderr MUST be distinct streams.

## Audit Requirements (Normative)

Each exec session MUST record:
- Actor identity (user or service principal)
- `org_id`, `app_id`, `env_id`
- `instance_id`, `host_id`
- Command array (may be redacted per policy)
- TTY flag
- `created_at`, `connected_at`, `ended_at`
- `exit_code` and `end_reason`

Audit events are immutable and retained per audit retention policy.

## CLI Semantics

`plfm exec` behavior:
- Exits with the remote process exit code when available.
- If the session fails before starting a process, use CLI exit codes:
  - 10: auth failure
  - 20: instance not running
  - 30: connect timeout
  - 40: server error
  - 50: rate limited

Example usage:
```bash
# Interactive shell
plfm exec i-01JEXAMPLE -- /bin/sh

# Non-interactive command
plfm exec i-01JEXAMPLE -- uptime

# With environment variables
plfm exec --env FOO=bar i-01JEXAMPLE -- printenv FOO
```

## Rate Limiting

Default limits:
- 10 concurrent exec sessions per env
- 2 concurrent exec sessions per instance
- 100 exec session creations per org per hour

429 response includes `Retry-After` header.

## Timeout Configuration

| Parameter | Default | Min | Max |
|-----------|---------|-----|-----|
| Token expiry | 60s | 30s | 300s |
| Session duration | 1 hour | 1 minute | 24 hours |
| Connect timeout | 30s | 10s | 120s |

## Security Considerations

- Exec tokens MUST NOT be logged.
- Command arguments MAY contain sensitive data; redaction policy applies.
- Exec sessions provide shell access; treat as high-privilege operation.
- Audit logs are the primary forensic tool for exec misuse.

## Compliance Tests (Required)

1. Exec to running instance succeeds with correct exit code.
2. Exec to non-running instance fails with clear error.
3. Expired token is rejected.
4. Reused token is rejected.
5. Resize events update PTY dimensions.
6. Signal forwarding works (SIGINT, SIGTERM).
7. Cleanup on disconnect kills orphaned processes.
8. Audit log contains all required fields.
9. Concurrency limits enforced.
10. Session duration timeout triggers cleanup.

## Open Questions (v2 Candidates)

- Transcript capture opt-in for compliance
- Stronger token binding (client cert, device binding)
- Session recording and replay
- File transfer via exec channel

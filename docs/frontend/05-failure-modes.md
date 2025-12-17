# Failure modes

This document enumerates failure modes for the web terminal and the intended user facing behavior.

A good terminal experience is mostly about how it fails.

## Connection failures

### WebSocket cannot connect

Causes:
- auth expired
- network blocked
- gateway down

UI behavior:
- show a clear error banner with reason category
- offer "retry"
- if auth expired, offer "re-auth" (redirect to login)

### Connection drops mid-session

Causes:
- wifi change
- laptop sleep
- mobile network handoff

UI behavior:
- switch to "reconnecting" state
- keep local scrollback visible
- attempt reconnect with exponential backoff
- on success, show "reconnected" toast and resume streaming
- on failure after N attempts, show "disconnected" with manual retry

If resume is enabled:
- replay missed output within buffer window
If resume fails:
- warn that output may have been missed

## Host failures

### PTY host process exits

Causes:
- user ran `exit`
- shell crashed

UI behavior:
- mark session closed
- show exit code
- allow "new session" with same context

### Host sandbox killed

Causes:
- resource limit exceeded
- platform restart

UI behavior:
- show "session terminated by host" with reason if known
- allow restart, but do not auto restart without user action

## WASM and rendering failures

### WASM module fails to load or instantiate

Causes:
- wasm bundle fetch fails
- CSP blocks wasm or worker execution
- unsupported browser (no WebAssembly)

UI behavior:
- show a blocking error with a clear reason when possible
- offer retry
- provide a copyable support bundle (browser version, feature flags, console errors)

### Worker or OffscreenCanvas unavailable

Causes:
- older browser
- cross origin isolation not enabled (SharedArrayBuffer path disabled)

UI behavior:
- fall back to main thread rendering
- show a non-blocking banner: "performance may be reduced"

### Canvas or GPU context lost

Causes:
- GPU reset
- tab backgrounding or memory pressure

UI behavior:
- attempt to recreate the canvas context and re-render from terminal state
- if recovery fails, keep the session alive and prompt the user to reload the page


## Output overload

### Client cannot keep up

Symptoms:
- UI freezes
- memory grows

Behavior:
- apply backpressure
- if still overloaded, drop output and show explicit warning:
  - "output dropped due to slow client"
- offer a "reduce output" hint:
  - suggest filtering logs
  - suggest `--tail` limits

### Scrollback too large

Behavior:
- cap scrollback
- when truncating, insert a visible marker line:
  - "[scrollback truncated]"
- do not silently discard

## Input issues

### Stuck modifier or key repeat

Behavior:
- provide an "input reset" action in the status bar
- allow sending a literal `Ctrl+C` and `Ctrl+Z` from UI buttons for mobile users

### Paste hazards

Behavior:
- if paste > threshold (for example 5k chars), show a confirmation
- show first few lines in preview
- allow cancel

## Protocol mismatches

- Version mismatch between client and gateway

Behavior:
- gateway returns a clear `unsupported_version`
- client prompts for refresh

## UI metadata failures

If receipt metadata cannot be parsed:
- ignore and keep terminal correct
- do not break the session
- log a client side warning metric

If metadata suggests an action that fails (for example, "wait" command errors):
- show the error as normal output
- avoid hiding failures behind UI

## User expectations

Always prefer:
- explicit banners and markers
- recoverability
- no silent data loss

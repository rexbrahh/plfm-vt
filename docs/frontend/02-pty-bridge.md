# PTY bridge

This document specifies the server side bridge between a PTY (or equivalent) and the browser terminal protocol.

## Components

- Terminal gateway (edge service)
  - Authenticates the WebSocket
  - Owns session lifecycle
  - Streams bytes between browser and host
  - Implements resume ring buffer

- Terminal host (compute)
  - Runs the actual shell or program
  - Exposes a PTY master to the gateway (directly or via an agent)

In v1, "host" can be:
- a container on the same machine as the gateway
- a microVM (preferred when available)
- a per-tenant VM

The bridge spec does not assume which.

## Session lifecycle

1. Client requests a new session (via console API).
2. Control plane creates a session record and returns a `session_id` and `token`.
3. Client opens WebSocket and sends `hello`.
4. Gateway attaches to the host PTY and starts streaming.

Closing:
- User closes tab: gateway keeps the session alive for a short grace period (for reconnect).
- Explicit close: gateway terminates the child process and cleans up.

## PTY creation

On the host, create a PTY pair:

- master: owned by gateway/agent
- slave: becomes stdin/out/err for the child process

Set:
- initial window size (cols, rows)
- TERM and locale
- an environment marker for UI integration, for example:
  - `TERM_PROGRAM=ghostty-web`
  - `GHOSTTY_WEB=1`

## Terminal type and capabilities

The client terminal engine is `libghostty-vt` in wasm, but the remote side should remain broadly compatible.

Defaults:

- `TERM=xterm-256color`
- `COLORTERM=truecolor` (if supported)
- `TERM_PROGRAM=ghostty-web`
- `GHOSTTY_WEB=1`

Notes:

- If the host image includes a matching terminfo entry, we may optionally set a more specific `TERM`, but it must never be required for correctness.
- The gateway should accept the client reported `cols`, `rows`, and pixel cell metrics, but treat `cols` and `rows` as the source of truth for PTY sizing.

The client may also send a capabilities object at session start (for example, truecolor, hyperlink support) purely for observability and future negotiation.


## Resize handling

- Client sends `resize`
- Gateway calls `ioctl(TIOCSWINSZ)` on PTY master and delivers `SIGWINCH` to the child

Rate limit resize events (for example 30 Hz max) to avoid pathological layouts.

## Flow control and backpressure

### Client input

- Browsers can burst paste large inputs.
- Gateway should:
  - cap max input frame size
  - apply backpressure by pausing websocket reads if PTY write buffers fill
  - optionally chunk large pastes

### PTY output

- Output can exceed websocket throughput.
- Gateway should:
  - maintain a ring buffer for resume (bounded memory)
  - when the client is slow, either:
    - buffer (until a cap), then
    - drop output with an explicit warning meta frame

Dropping bytes is bad but sometimes better than OOM. When dropping, we must surface it clearly.

## Multi-attach (optional)

Support multiple browser clients attached to the same PTY:

- One "owner" has write permission
- Others are read-only observers

This is useful for:
- pair debugging
- customer support sessions

## Shell integration (optional but recommended)

To enable transcript view and rich UI cards, we want reliable command boundaries.

Approach:
- Ship a small shell integration snippet in the host image.
- On shell startup, it:
  - emits prompt boundary markers (OSC sequences)
  - sets PS1 (or hooks) in a minimal, compatible way

We should keep this compatible with bash first, then add zsh and fish.

## Recording (optional)

If we add "download transcript", do it at the gateway:

- store a bounded session log (bytes or transcript blocks)
- encrypt at rest if persisted
- never enable by default unless user opts in

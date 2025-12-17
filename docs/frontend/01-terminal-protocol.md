# Terminal protocol

This document defines the browser to gateway protocol for interactive terminal sessions.

We treat the terminal stream as primary (bytes in and out). We optionally support a sideband control channel for reliability, session management, and UI metadata.

## Transport

- WebSocket over TLS: `wss://.../terminal`
- One socket per session
- Messages are either:
  - Binary frames (performance critical, used for TTY bytes)
  - JSON text frames (control and metadata)

We avoid HTTP chunking or Server Sent Events because we need bidirectional, low latency input.

## Versioning

Every session starts with a JSON `hello` message from client to server:

```json
{
  "type": "hello",
  "v": 1,
  "session_id": "sess_...",
  "token": "opaque",
  "cols": 120,
  "rows": 34,
  "features": {
    "resume": true,
    "compression": false,
    "osc_metadata": true
  }
}
```

The server responds with `welcome`:

```json
{
  "type": "welcome",
  "v": 1,
  "server_time_unix_ms": 1730000000000,
  "resume": { "enabled": true, "buffer_bytes": 1048576 }
}
```

## Terminal engine integration (libghostty-vt wasm)

The browser side terminal emulator is `libghostty-vt` running in WebAssembly. The protocol is intentionally simple:

- The server streams bytes.
- The client feeds bytes into the emulator.
- The emulator owns VT parsing, screen state, and scrollback.

Implications:

- Treat terminal output as a byte stream. Do not decode and re-encode text in the middle.
- WebSocket frames can cut through escape sequences. Feeding raw bytes into the wasm module naturally handles partial sequences across frames.
- If we use OSC 777 metadata inside the byte stream, we surface it to JS via either:
  - an OSC hook exported from the wasm module (preferred)
  - a thin, stream-safe JS shim that detects and removes OSC 777 sequences while emitting UI events


## Binary frames

Binary frames use a 1 byte type tag, followed by payload.

- `0x01` Client to server: input bytes destined for PTY stdin
- `0x02` Server to client: output bytes read from PTY master
- `0x03` Server to client: output replay chunk (only during resume)

Notes:
- Payload is raw bytes as seen on a PTY. No UTF-8 assumptions.
- The frontend terminal emulator is responsible for decoding VT sequences.

## JSON control frames

### Resize

Client sends when viewport changes:

```json
{ "type": "resize", "cols": 120, "rows": 34 }
```

Gateway applies this to the PTY and delivers `SIGWINCH` to the child process.

### Ping / pong

Either side may send:

```json
{ "type": "ping", "t": 1730000000000 }
{ "type": "pong", "t": 1730000000000 }
```

Used to estimate RTT and detect dead sockets.

### Close

Client requests a clean close:

```json
{ "type": "close", "reason": "user_close" }
```

Server replies:

```json
{ "type": "closed", "exit_code": 0 }
```

### Resume

If the socket drops, client reconnects and replays `hello` with `resume_from`:

```json
{
  "type": "hello",
  "v": 1,
  "session_id": "sess_...",
  "token": "opaque",
  "cols": 120,
  "rows": 34,
  "resume_from": { "out_seq": 391239 }
}
```

Server replays any buffered output not yet acknowledged (see acks below), then continues streaming live output.

### Ack (optional)

To support lossless resume, client can periodically ack the last received output sequence number:

```json
{ "type": "ack", "out_seq": 391239 }
```

Server keeps a ring buffer of recent output and uses it for replay. If the client resumes from a sequence outside the buffer window, the server returns:

```json
{ "type": "resume_failed", "reason": "buffer_too_small" }
```

The UI should then surface: "Session continued, but you may have missed output while disconnected."

## UI metadata (optional)

There are two ways to deliver UI metadata.

### Option A: OSC metadata inside the terminal stream (preferred)

The platform CLI can emit metadata using OSC sequences that are ignored by normal terminals but parsed by our emulator.

We reserve OSC `777` for ghostty-web:

- Format: `ESC ] 777 ; <kind> ; <payload> ST`
- `kind` is an identifier like `receipt`, `resource`, `event`, `hint`
- `payload` is base64url encoded JSON

Example JSON payload for a mutation receipt:

```json
{
  "receipt_id": "rcpt_...",
  "kind": "release.create",
  "status": "accepted",
  "ids": { "release_id": "rel_...", "app_id": "app_..." },
  "next": [
    { "label": "Wait for deploy", "cmd": "ghost release wait rel_..." },
    { "label": "Tail events", "cmd": "ghost event tail --release rel_..." }
  ]
}
```

The terminal engine must:
- parse OSC 777
- decode base64url
- emit an internal event for the UI
- not render the bytes as visible text

### Option B: JSON `meta` frames over WebSocket

If we do not want to depend on OSC parsing, the gateway can send:

```json
{ "type": "meta", "kind": "receipt", "payload": { ... } }
```

This is useful for gateway generated metadata (connection state, server warnings). For CLI generated metadata, OSC is usually simpler.

## Security considerations

- Tokens are short lived, bound to user session, and scoped to a single terminal session id.
- The gateway must enforce origin checks and rate limits.
- No metadata payload should be trusted without validation (schema and size limits).

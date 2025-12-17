# Worker message schema (v1)

This document defines the `postMessage` protocol between the main thread UI and the terminal worker.

It is intentionally independent from the websocket protocol used to talk to the server PTY.

## Transport

- The main thread creates a dedicated worker: `new Worker(url, { type: "module" })`.
- Messages are JSON objects with an envelope.
- Binary payloads use ArrayBuffer and are transferred (Transferable) to avoid copying.

## Envelope

All messages use this envelope:

```ts
type MsgEnvelope<T = any> = {
  v: 1;                // schema version
  sid: string;         // terminal session id (stable per tab/pane)
  id?: number;         // request id (present only for request/response)
  type: string;        // message type
  payload?: T;         // message payload
  ts?: number;         // optional ms timestamp from sender
};
```

Rules:
- Messages with `id` are requests. Responses must echo `id` and use `type` prefixed with `resp.`.
- Events do not carry `id`.
- Unknown `type` must not crash either side. Log and ignore.

## Binary payload convention

For messages that carry bytes, `payload` includes a `buf` field that is an ArrayBuffer.

Example:

```ts
type PtyChunk = { buf: ArrayBuffer };
```

Send with:
- `worker.postMessage(msg, [msg.payload.buf])`

This transfers ownership to the receiver.

## Message types

### Main thread -> worker

#### `req.init`

Initialize a session and pass static configuration.

Payload:

```ts
type InitPayload = {
  wasmURL: string;                   // location of libghostty-vt.wasm
  renderer: "offscreen" | "main";    // desired renderer mode
  theme?: { name?: string; bg?: string; fg?: string };
  font?: { family?: string; sizePx?: number; weight?: number };
  scrollbackLines?: number;
  features?: {
    osc777?: boolean;                // receipt metadata parsing
    osc52?: boolean;                 // clipboard integration if supported
    hyperlinks?: boolean;
  };
};
```

Response:
- `resp.init` with `{ ok: true }` or `{ ok: false, error }`

#### `req.attachCanvas`

Attach an OffscreenCanvas for rendering.

Payload:

```ts
type AttachCanvasPayload = {
  canvas: OffscreenCanvas;
  dpr: number;
  widthPx: number;
  heightPx: number;
};
```

Transfer the canvas in the postMessage transfer list.

Response:
- `resp.attachCanvas`

#### `req.connect`

Connect to a remote PTY stream.

Payload:

```ts
type ConnectPayload = {
  url: string;                 // websocket URL
  token?: string;              // optional auth token if not using cookies
  protocols?: string[];        // websocket subprotocols (optional)
};
```

Response:
- `resp.connect`

Events:
- `evt.connected`
- `evt.disconnected`

#### `evt.input.key`

Forward a key event. The worker must decide how to encode it.

Payload:

```ts
type KeyPayload = {
  key: string;                 // KeyboardEvent.key
  code: string;                // KeyboardEvent.code
  location: number;            // KeyboardEvent.location
  repeat: boolean;
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
  meta: boolean;
};
```

No response.

#### `evt.input.text`

Committed text input (IME commit, mobile input, or text insertion).

Payload:

```ts
type TextPayload = { text: string };
```

No response.

#### `evt.input.paste`

Paste text from clipboard.

Payload:
- `{ text: string }`

No response.

#### `evt.viewport.resize`

Viewport pixel size or DPR changed.

Payload:

```ts
type ResizePayload = {
  dpr: number;
  widthPx: number;
  heightPx: number;
};
```

No response.

The worker must:
- call `term_set_dpi` and `term_resize_px`
- compute cols and rows
- send resize control to the server (worker-owned websocket) or emit `evt.grid.changed` to main thread so main can inform server

#### `req.getSelection`

Request current selection text.

Response:
- `resp.getSelection` with `{ text: string }`

#### `req.setOptions`

Update runtime options.

Payload can include any subset of init options, plus:
- `cursorStyle`, `blink`, `ligatures`, `copyOnSelect` (if supported)

Response:
- `resp.setOptions`

#### `req.dispose`

Tear down the session.

Response:
- `resp.dispose`

### Worker -> main thread

#### `evt.ready`

Worker is initialized and WASM is loaded.

Payload:

```ts
type ReadyPayload = {
  wasmVersion: string;
  renderer: "offscreen" | "main";
  features: {
    osc777: boolean;
    osc52: boolean;
    hyperlinks: boolean;
  };
};
```

#### `evt.connected` / `evt.disconnected`

Payload:

```ts
type DisconnectedPayload = {
  code?: number;
  reason?: string;
  wasClean?: boolean;
};
```

#### `evt.grid.changed`

Worker computed terminal grid and cell metrics.

Payload:

```ts
type GridPayload = {
  cols: number;
  rows: number;
  cellWpx: number;
  cellHpx: number;
};
```

Main thread may display these and can use them to size UI overlays.

#### `evt.title.changed`

Remote app set a window title (OSC 0 or 2).

Payload:
- `{ title: string }`

#### `evt.bell`

Terminal bell event.
- `{}`

#### `req.clipboard.writeText`

Worker requests main thread to write to clipboard (ex: OSC 52, copy-on-select).

Payload:
- `{ text: string, reason?: "osc52" | "copy" | "select" }`

Response:
- `resp.clipboard.writeText` with `{ ok: boolean, error?: string }`

#### `evt.selection.changed`

Payload:
- `{ hasSelection: boolean }`

#### `evt.link.hint`

Optional hyperlinks support.
Payload:
- `{ url: string, x: number, y: number }`

#### `evt.receipt`

Receipt metadata emitted by CLI (recommended via OSC 777) surfaced to UI.

Payload:

```ts
type ReceiptPayload = {
  kind: string;                 // ex: "release.created"
  data: any;                    // structured object
  raw?: string;                 // raw payload for debugging
};
```

#### `evt.stats`

Periodic performance counters for overlays and debugging.

Payload:

```ts
type StatsPayload = {
  fps?: number;
  renderMsP50?: number;
  renderMsP99?: number;
  bytesInPerSec?: number;
  bytesOutPerSec?: number;
  backlogBytes?: number;        // queued PTY bytes not yet processed
};
```

#### `evt.error`

Structured error.

Payload:

```ts
type ErrorPayload = {
  code: string;                 // ex: "WASM_LOAD_FAILED"
  message: string;
  detail?: any;
};
```

## Backpressure rules

- The worker may drop `evt.stats` if the main thread is busy.
- PTY output bytes must never be dropped silently. If backlog grows beyond a threshold:
  - emit `evt.error` with code `OUTPUT_BACKLOG`
  - optionally request a reconnect or pause
- Input events may be coalesced:
  - repeated wheel events can be coalesced
  - repeated resize events can be debounced
  - text commits must not be reordered

## Optional fast path: SharedArrayBuffer ring buffers

If the page is cross-origin isolated (COOP/COEP), we may add an optional SharedArrayBuffer transport.

This is not required for v1. If enabled later:
- main allocates SAB for input and output rings
- worker and main use Atomics to coordinate
- websocket stays in worker

We should only implement this after profiling shows postMessage transfer is a bottleneck.


# WASM terminal component contract (v1)

This document defines the v1 contract between:
- the browser UI (main thread)
- the terminal worker
- the embedded `libghostty-vt` WebAssembly module

The goal is to make the interface stable so we can evolve implementation details without breaking the UI.

## Goals

- Treat the PTY byte stream as the source of truth.
- Keep terminal state, input encoding, and rendering in the worker to protect UI responsiveness.
- Provide a minimal, explicit JS <-> worker <-> WASM API surface.
- Support a canvas-first renderer (OffscreenCanvas when available) with damage-based redraw.

## Non-goals

- Replacing the terminal engine with a JS parser.
- Implementing a full IDE or GUI shell inside the terminal.
- Guaranteeing perfect IME behavior in v1. We support a pragmatic baseline and leave room for iteration.

## Architecture overview

### Threads and responsibilities

Main thread:
- DOM, layout, focus management, accessibility shell
- capturing user input events (keyboard, pointer, wheel)
- clipboard access (read and write)
- owns product UI around the terminal (panels, tabs, inspector)

Worker:
- owns the websocket (recommended) or receives PTY bytes from main thread
- runs `libghostty-vt` WASM
- converts input events into bytes according to terminal modes
- feeds PTY bytes into the terminal engine
- renders into OffscreenCanvas (preferred) or produces a framebuffer for main thread presentation (fallback)
- emits UI events (title change, bell, link hints, selection changes)

WASM (`libghostty-vt`):
- terminal state machine (VT sequences, modes, scrollback)
- input encoder (key events to escape sequences)
- renderer hooks (damage, cell grid changes, optional GPU path)

### Primary data flows

1) Remote output:
- websocket binary frames -> worker -> `term.feedPty(bytes)` -> damage -> render

2) Local input:
- main thread event -> worker -> `term.inputKey(...)` or `term.inputText(...)` -> worker gets output bytes -> websocket send

3) Resize:
- main thread observes pixel size and DPR -> worker -> `term.resizePx(...)`
- worker computes cols and rows -> worker sends resize control to server

## Core concepts

### Viewport and grid

- The terminal is logically a grid (cols x rows) and a scrollback buffer.
- The viewport is the visible rows, rendered to a canvas in device pixels.
- The relationship between pixel size and (cols, rows) depends on font metrics and terminal settings. The worker owns this computation.

### Damage rectangles

Damage is reported as a list of rectangles (in device pixels) that changed since the last render. Rendering should only redraw damaged regions when possible. If the renderer cannot efficiently partial redraw, it may treat any non-empty damage list as "full redraw".

### Input encoding

Keydown events must not be converted to bytes on the main thread. Terminal input depends on runtime modes negotiated via VT sequences (application cursor keys, keypad mode, bracketed paste, etc). The worker must pass events into WASM and use WASM-produced bytes as the canonical output.

## JS <-> worker API surface

The main thread interacts with the worker using the message schema defined in `07-worker-message-schema.md`.

The contract here is behavioral, not the literal TS class names.

Main thread responsibilities:
- Send: init, attachCanvas, connect, input events, resize, theme settings
- Receive: ready, status, clipboard requests, title changes, selection updates, stats

Worker responsibilities:
- Enforce sequencing (no input before ready)
- Apply backpressure for extreme input bursts
- Surface errors in a structured form

## Worker <-> WASM API surface

WASM should expose a narrow, stable set of functions. The worker provides a small wrapper around these exports.

### Required WASM exports

The names here are descriptive. The actual export names can differ, but the semantics must match.

#### Lifecycle

- `term_create(config_ptr, config_len) -> term_handle`
- `term_destroy(term_handle) -> void`
- `term_reset(term_handle, mode_flags) -> void`

Config is an opaque blob (recommended: msgpack or JSON) so we can add options without ABI breakage.

#### Feeding remote PTY output

- `term_feed_pty(term_handle, bytes_ptr, bytes_len) -> void`

Behavior:
- Consumes raw bytes exactly as received from the PTY.
- Updates internal state and scrollback.
- Does not allocate unbounded memory. If scrollback grows, it must follow configured limits.

#### Polling damage and events

- `term_poll_damage(term_handle, out_ptr, out_cap) -> rect_count`
- `term_clear_damage(term_handle) -> void`

Rect format (packed):
- `x: u32, y: u32, w: u32, h: u32` in device pixels.

Optional but recommended:
- `term_poll_events(term_handle, out_ptr, out_cap) -> event_count`
- `term_clear_events(term_handle) -> void`

Events include title changes, bell, mode changes, and clipboard requests if the terminal supports OSC 52.

#### Input encoding

- `term_input_key(term_handle, key_event_ptr, key_event_len) -> out_len`
- `term_input_text(term_handle, utf8_ptr, utf8_len) -> out_len`
- `term_input_paste(term_handle, utf8_ptr, utf8_len) -> out_len`
- `term_take_output(term_handle, out_ptr, out_cap) -> taken_len`

Behavior:
- `term_input_*` enqueues outgoing bytes to an internal output queue.
- `term_take_output` drains bytes from that queue.
- Output bytes must be the exact sequences to send to the remote PTY.

Key event payload:
- A structured object describing key, code, modifiers, and physical key location.
- The worker may canonicalize browser key events into a stable format.

#### Resize and metrics

- `term_set_dpi(term_handle, dpi_x_f32, dpi_y_f32) -> void`
- `term_resize_px(term_handle, width_px_u32, height_px_u32) -> void`
- `term_get_grid(term_handle, out_ptr) -> void`

`term_get_grid` writes:
- `cols: u32`
- `rows: u32`
- `cell_w_px: u32`
- `cell_h_px: u32`

The worker uses cols and rows to send a resize control to the server.

#### Rendering

Two supported rendering strategies.

Strategy A (preferred): WASM renders directly to an OffscreenCanvas context owned by the worker.
- `term_render_begin(term_handle) -> void`
- `term_render_to_canvas(term_handle, canvas_handle, damage_ptr, damage_len) -> void`
- `term_render_end(term_handle) -> void`

Strategy B (fallback): WASM renders into a framebuffer and JS presents it.
- `term_get_framebuffer(term_handle) -> fb_ptr`
- `term_get_framebuffer_info(term_handle, out_ptr) -> void`
- `term_render(term_handle, damage_ptr, damage_len) -> void`

Framebuffer info:
- `width_px: u32`
- `height_px: u32`
- `stride_bytes: u32`
- `format: u32` (RGBA8888)

In v1 we prefer Strategy A because it avoids copying large buffers across thread boundaries.

### Error model

WASM exports must not trap for normal errors. They should:
- return a status code, or
- enqueue an error event retrievable via `term_poll_events`.

Traps are reserved for programmer bugs.

### Versioning

WASM must expose:
- `term_get_version(out_ptr, out_cap) -> len`

The worker includes this in the `ready` message so the main thread can log and gate features.

## Minimum rendering loop contract

The worker is responsible for calling render in a predictable cadence.

- On every incoming PTY chunk: feed -> poll damage -> schedule render soon.
- On animation frames: if damage non-empty, render.
- On idle: do nothing.

The render loop is detailed in `08-render-loop.md`.

## Practical v1 constraints

- If OffscreenCanvas is not available, run the terminal engine in the main thread only as a temporary compatibility fallback, and expect worse responsiveness on high throughput.
- Clipboard write must be requested from main thread.
- IME is supported by sending committed text via `input.text`. Full preedit rendering is not required in v1.

## Test vectors

We must keep a small suite of deterministic tests:
- VT output parsing correctness (cursor movement, colors, wrapping)
- mode-dependent input encoding (arrow keys, function keys, alt combinations)
- resize correctness (cols, rows, scroll region)
- performance under spam output (10 MB bursts)


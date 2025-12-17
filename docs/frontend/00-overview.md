# Web terminal frontend overview

This folder specifies the customer facing web terminal experience that embeds a full terminal emulator, plus optional UI augmentations for our platform CLI and console.

The goal is to feel like "a real terminal first", with UI help that never compromises copy/paste workflows, scripting guarantees, or the ability to run full screen TUI programs.

## Goals

- Provide a low latency, reliable terminal in the browser (SSH-like feel).
- Keep the terminal transcript as the source of truth. UI features are strictly additive.
- Support both:
  - Raw TTY programs (vim, htop, ssh, interactive prompts).
  - Structured platform workflows (select, inspect, deploy, tail, wait).
- Make long running sessions easy (logs, ssh, tails) without polluting the main interaction area.
- Keyboard first UX.

## Non-goals (v1)

- A full IDE.
- Arbitrary remote desktop.
- Custom per-user shell customization beyond what is needed for prompt integration.
- Perfect offline operation (we aim for graceful disconnect, not offline compute).

## Terminal engine (libghostty-vt via WebAssembly)

We embed `libghostty-vt` compiled to WebAssembly as the terminal emulator and renderer. This is a hard constraint, not an implementation detail.

High level flow:

- WebSocket delivers raw PTY bytes (UTF-8 plus VT escape sequences).
- JS glue feeds bytes into the wasm module as an opaque byte stream.
- `libghostty-vt` owns parsing, screen state, scrollback, and damage tracking.
- The UI layer owns browser integration (focus, IME, clipboard, accessibility) plus optional panels and overlays.

Threading and rendering:

- Preferred: run the emulator in a Worker with `OffscreenCanvas` so heavy output does not block the UI thread.
- Fallback: run on the main thread if Worker or `OffscreenCanvas` is unavailable.
- If we want SharedArrayBuffer backed rings for very low latency, the app must be cross origin isolated (COOP/COEP).

Public JS boundary (sketch):

- `init({ canvas, cols, rows, dpi })`
- `onBytes(Uint8Array)` for PTY output
- `onKey(...)` and `onText(...)` for user input
- `resize(cols, rows, cellPx)`
- `render()` or `present()` called on animation frames

All UI metadata stays additive. If we add prompt markers or receipt cards, they must not break raw TTY behavior.


## Primary UX primitives

### 1. Sessions

A session is a single PTY-backed terminal stream (shell or program) with its own scrollback and connection state.

- Sessions can be opened from:
  - Console navigation (for an org/project/app/env).
  - Inline actions (for example, "Open logs" spawns a logs session).
- Sessions can be:
  - Interactive (read/write).
  - Watch-only (read-only observers).

### 2. Terminal surface

The terminal surface is always present and always correct.

Enhancements must not:
- Hide bytes that would normally be printed.
- Reorder output.
- Change exit codes or command semantics.

### 3. Command transcript (optional enhanced view)

When we can reliably detect command boundaries (prompt integration), we render a "transcript view":

- Each command becomes a block:
  - input line
  - output stream
  - exit status
  - timestamps
- Blocks can be collapsed, copied, and linked.
- Full screen programs automatically switch to raw terminal view.

If prompt integration is unavailable, we still work as a normal terminal, just without block semantics.

### 4. Inspector panel (right side)

A persistent panel on the right provides "instruments":

- Current context (org, project, app, env).
- Resource details (release, workload, endpoint, volume).
- Event stream for selected resource (derived from CLI receipts or console selection).
- Help for the current command (flags, examples).

The inspector must be optional and hideable.

### 5. Streams drawer (toggle)

Some commands produce a persistent running feed (logs, ssh, watch, event tail).

Instead of forcing the user to keep that output in the main session, we support a streams drawer:

- Toggle show/hide: `Ctrl+L` (configurable).
- A stream runs in its own session behind the scenes.
- The drawer can:
  - show 1 stream full width
  - show a list of active streams
  - detach a stream into a separate tab

This matches the mental model: work in the main session, observe in the drawer.

## Layout

Desktop default:

- Top: context bar (breadcrumbs + session tabs)
- Left 2/3: main terminal surface (or transcript view)
- Right 1/3: inspector panel
- Bottom: status bar (connection, latency, recording, shortcuts)

```
+--------------------------------------------------------------+
| Org / Project / App / Env        [tab1][tab2][+ new]          |
+----------------------------------------------+---------------+
|                                              |               |
|                Terminal surface              |   Inspector   |
|     (raw terminal or transcript blocks)      | (context etc) |
|                                              |               |
+----------------------------------------------+---------------+
| status: connected   rtt: 28ms   Ctrl+L streams   Ctrl+K cmd   |
+--------------------------------------------------------------+
```

Small screens:

- Terminal is full width.
- Inspector becomes a slide-over panel.
- Streams drawer becomes a bottom sheet.

## Interaction design

### Focus model

- Click inside terminal to focus.
- `Esc` exits any transient UI (palette, inspector search) and returns focus to terminal.
- A visible caret indicator shows which surface owns input (terminal vs palette).

### Command palette and slash commands

We support a palette for UI level commands, not to replace the shell.

- Open palette: `Ctrl+K`
- Also: typing `/` at an empty prompt opens slash command suggestions.
- Palette commands act like macros that run one or more real CLI commands and render their outputs as normal.

Examples:
- `/switch env` -> runs `ghost env select ...`
- `/open logs` -> spawns a logs session and opens the streams drawer

### Selection and "receipt cards"

When the platform CLI emits a mutation receipt (see 01-terminal-protocol.md for metadata transport), the UI can render an inline receipt card:

- stable IDs
- status summary
- next step commands (copy buttons)
- link to open inspector on that resource

If metadata is missing, we still show the plain text output.

### Copying

- Default terminal selection copies plain text.
- Each transcript block has:
  - Copy output
  - Copy command
  - Copy as runnable snippet (command + output redacted)
- Receipt cards have "copy next command".

### Search

- `Ctrl+F` searches within the active session scrollback.
- In transcript view, search results are grouped by command blocks.

## Rendering and theming

- Use a single monospace font with good glyph coverage.
- Minimal chrome, dark mode first.
- Respect system preferences:
  - reduce motion
  - contrast

## Instrumentation and telemetry

We capture local metrics (no user content):

- keystroke to paint latency
- frames per second
- websocket RTT
- reconnect counts and durations
- scrollback memory usage

These metrics feed the latency budget doc.

## Open questions

- Do we want a "split pane terminal" in v1, or rely on tabs + streams drawer?
- How much shell integration do we ship by default (bash only, or bash+zsh+fish)?
- Should transcript view be the default, or an opt-in toggle until it is battle tested?

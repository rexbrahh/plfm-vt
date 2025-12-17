# Render loop and damage policy (v1)

This document defines how the worker schedules rendering for `libghostty-vt` in WASM.

The purpose is to keep latency low while avoiding unnecessary CPU and GPU work.

## Definitions

- "Damage" means a region of the terminal surface changed since last render.
- "Frame" means a render pass that updates the canvas.
- "Present" means the canvas reflects the latest terminal state.

## Worker scheduling model

### Event-driven render

On every PTY chunk:
1) `term_feed_pty(bytes)`
2) poll damage
3) if damage non-empty, schedule render soon

"Schedule render soon" means:
- if not already scheduled, request the next animation frame (preferred)
- if in a burst where RAF is too slow, allow a timer fallback (ex: 8 ms)

### RAF loop

Pseudo:

```ts
let renderPending = false;

function onPty(bytes) {
  term.feedPty(bytes);
  if (term.hasDamage()) schedule();
}

function schedule() {
  if (renderPending) return;
  renderPending = true;
  requestAnimationFrame(render);
}

function render() {
  renderPending = false;
  const damage = term.pollDamage();
  if (damage.length === 0) return;

  term.renderToCanvas(canvas, damage);
  term.clearDamage();

  // If new damage arrived while rendering, schedule again.
  if (term.hasDamage()) schedule();
}
```

Notes:
- `pollDamage` must be cheap. If it is not, allow a "full redraw" mode where any damage triggers a redraw without enumerating rectangles.

## Damage semantics

### Coordinates

Damage rectangles are in device pixels, relative to the top-left of the canvas.

### Coalescing

The worker may coalesce rectangles to reduce overhead:
- if more than N rectangles exist, merge them into a small set
- if rectangles cover more than X percent of the surface, treat as full redraw

Suggested defaults:
- N = 64
- X = 0.35

These defaults should be driven by profiling.

## Resize behavior

When `evt.viewport.resize` arrives:
1) update DPR and pixel size
2) call `term_resize_px`
3) treat resize as "full redraw"
4) send new cols and rows to server

Resizes should be debounced:
- consecutive resize events within 50 ms can be collapsed into the last one

## Input latency and frame budget

Targets (v1):
- keydown to bytes on websocket send: p50 under 8 ms, p99 under 25 ms
- PTY chunk arrival to present: p50 under 16 ms, p99 under 50 ms

If targets are missed:
- prioritize correctness over fancy effects
- reduce optional overlays
- disable expensive features (hyperlink scanning, high-frequency stats)

## Main-thread fallback

If OffscreenCanvas is unavailable:
- keep the worker for parsing and encoding if possible
- render via framebuffer transfer only if profiling indicates acceptable performance
- otherwise accept a v1 compatibility mode where WASM runs on main thread

This fallback should be explicit in code and in UI diagnostics.


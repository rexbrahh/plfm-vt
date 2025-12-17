# Performance and latency budget

This document defines measurable performance targets for the web terminal.

The user experience threshold is simple: typing must feel local.

## Key metrics

- Keystroke to paint latency (K2P)
  - time from keydown to the glyph appearing on screen (local echo or remote echo)
- End to end round trip time (RTT)
  - websocket ping RTT
- Throughput
  - sustained output bytes per second (logs)
- Jank
  - dropped frames during heavy output
- Memory
  - terminal buffer memory footprint per session

## Targets (v1)

These are targets, not guarantees, but we should design toward them.

Interactive typing:
- K2P p50: <= 30 ms
- K2P p95: <= 80 ms

Round trip:
- RTT p50 (same region): <= 60 ms
- RTT p95 (same region): <= 150 ms

Output throughput:
- sustain 2 to 5 MB/s of text output without freezing the UI
- do not block input while output is streaming

Startup:
- first paint of terminal UI: <= 500 ms on a modern laptop
- session connect: <= 2 s in-region

## Design techniques

Frontend:
- render terminal cells with an incremental renderer (canvas or WebGL), not DOM per glyph
- batch draw operations per frame
- virtualize scrollback, do not keep an unbounded DOM
- parse VT sequences in a worker when possible

libghostty-vt wasm:
- prefer a Worker + OffscreenCanvas pipeline
- keep a single wasm instance per session to preserve scrollback
- feed PTY output as Uint8Array without intermediate UTF-8 decoding
- coalesce many small output frames into one render tick to avoid thrash
- track and render only damaged regions when possible
- measure and pin cell metrics (font size, DPI, line height) and avoid layout driven resizes

Protocol and gateway:
- binary frames for TTY bytes
- minimize JSON chatter
- keep resume ring buffer bounded
- avoid per-byte allocations

Host:
- avoid CPU heavy shell prompts
- disable expensive motd output

## Measurement plan

- Collect client side metrics (no content):
  - K2P histograms
  - frame time histograms
  - dropped frame counts
  - websocket RTT
- Collect server metrics:
  - session connect time
  - bytes in/out
  - replay size on resume
  - output drop events

## Performance hazards

- Very large scrollback
- TUI apps that redraw constantly (top, htop)
- High latency links causing remote echo delay
- Output storms (cat huge file, verbose logs)
- Excessive resizing from responsive layouts

Mitigations:
- scrollback cap (configurable)
- warn and offer "download output" when too large
- optional output throttling for known log streams

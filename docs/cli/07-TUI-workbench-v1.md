# TUI Workbench v1

A dialog-first terminal workbench that wraps the platform CLI with a guided, discoverable interface. It feels like a chat-style REPL with slash commands, selection prompts, and structured receipts, plus a persistent sidebar for live status.

The CLI remains the canonical interface for scripting and automation. The TUI is a client of the same core libraries and APIs.

---

## Goals

- Make common workflows discoverable and low-friction without hiding what is happening.
- Preserve CLI guarantees: idempotency, stable IDs, machine-readable output, consistent exit codes, explicit mutation receipts.
- Embrace eventual consistency: show desired vs current state and provide first-class wait and event streaming.
- Provide a single “workbench” view that helps users deploy, observe, debug, and rollback.

## Non-goals (v1)

- Not a full terminal multiplexer (tmux/zellij replacement).
- Not an embedded editor distribution (no bundling LazyVim, Doom, etc.).
- Not a web console replacement.
- Not an AI agent surface. The dialog is deterministic and command-driven.

---

## Product contract

1) **Every slash command maps to a real CLI command (or an explicit short sequence).**
2) **The feed always shows the equivalent CLI invocation** (copyable).
3) **Mutations always produce a receipt** with stable IDs, what changed, and how to wait for convergence.
4) **No hidden side effects.** If something is created, updated, or deleted, it is stated plainly.
5) **Never reveal secret values.** Show metadata, delivery status, and versioning only.

---

## Layout

### Main area (left, 2/3): Dialog feed
A vertical timeline of:
- user inputs (slash commands, selections, confirmations)
- system outputs rendered as structured cards
- optional streaming blocks (events/logs) that can be collapsed

### Sidebar (right, 1/3): Live status and inspector
Always visible:
- current context: org, project, app, env
- active release and health summary
- desired vs current convergence indicator
- endpoints summary (L4, IPv6 default, IPv4 add-on, Proxy Protocol v2 flags)
- warnings (quota, degraded workloads, pending secret delivery)

Contextual inspector:
- when the user selects an item in the feed (release, workload, endpoint), the sidebar shows its details

---

## Interaction model

### Input modes
- **Command mode:** user types `/something ...`
- **Selection mode:** user picks from a list using:
  - number shortcuts (1, 2, 3)
  - fuzzy search filter (`/filter <text>`)
  - direct ID paste (stable IDs)
- **Confirmation mode:** destructive actions require explicit confirmation (type the app name, or type `confirm`)

### Output types (cards)
- **Summary card:** short outcome plus key fields
- **List card:** numbered list with stable IDs and short labels
- **Diff card:** manifest or config diffs (collapsed by default unless small)
- **Receipt card:** mutation receipt with stable IDs and “next steps”
- **Stream card:** logs/events stream with filters and pause/resume

### Copyable CLI echo
Every command execution shows:
- the canonical CLI command line
- optional `--json` equivalent for scripts

Example snippet in the feed:
- “Equivalent CLI: `ghost deploy -a APP -e ENV --manifest ./ghost.toml`”

---

## Core flows (v1)

### Deploy flow (`/deploy`)
1) Resolve context (org/project/app/env)
2) Select manifest source:
   - current directory
   - explicit path
   - “use last”
3) Validate manifest
4) **Env and secrets gate (required before release creation):**
   - show required env keys and secret bundles referenced
   - user must either:
     - set/import env and secrets, or
     - explicitly acknowledge none required
5) Show plan:
   - diff: desired changes vs current state
   - resources affected (release, workloads, endpoints, volumes)
6) Confirm
7) Execute deploy (create release)
8) Stream events until:
   - converged, or
   - user detaches
9) Print receipt with:
   - release ID
   - how to wait
   - how to rollback
   - suggested next commands

### Rollback flow (`/rollback`)
- list recent releases
- select target
- confirm
- execute promotion/rollback
- wait or detach
- receipt

### Observe flow (`/status`, `/workloads`, `/endpoints`)
- always show desired vs current
- show drift and pending reconciliation reasons when available
- quick actions: “tail events”, “tail logs”, “describe workload”

### Debug flow (`/events`, `/logs`, `/describe`)
- event tail with filters: app/env/workload/severity/type
- log tail with filters: workload/instance, time window, grep-like filter
- describe commands return structured fields suitable for copy/paste into bug reports

---

## Slash command set (v1)

### Context
- `/context`  
  Guided picker for org, project, app, env.
- `/whoami`  
  Auth identity, token scope summary, current context.
- `/use <org>/<project>/<app>@<env>`  
  Fast path context set.

### Deploy and releases
- `/deploy [--manifest PATH]`  
  Guided deploy flow (includes env/secrets gate).
- `/releases`  
  List releases.
- `/release <id>`  
  Describe a release.
- `/rollback`  
  Guided rollback flow.
- `/wait [release|workload] <id>`  
  Wait for convergence with progress.

### Workloads and instances
- `/workloads`  
  List workloads and health.
- `/workload <id>`  
  Describe workload.
- `/instances <workload>`  
  List instances.
- `/restart <instance>`  
  Restart instance (confirm).

### Networking
- `/endpoints`  
  List endpoints and configs (L4, IPv6, IPv4 add-on, Proxy Protocol v2).
- `/endpoint <id>`  
  Describe endpoint.
- `/ipv4 enable`  
  Enable IPv4 add-on (confirm, show billing note if applicable in product terms).

### Secrets and env
- `/env`  
  Show env summary, required keys, and status.
- `/env set KEY=VALUE`  
  Set env var (confirm if production env).
- `/env import`  
  Import from file or shell format.
- `/secrets`  
  List secret bundles and delivery status (no values).
- `/secrets import`  
  Guided import (bundle selection, source).
- `/secrets status`  
  Delivery reconciliation status.

### Observability
- `/events [filters...]`  
  Tail events, collapsible stream card.
- `/logs [filters...]`  
  Tail logs, collapsible stream card.
- `/diag`  
  Generate a debug bundle summary (commands to reproduce, IDs, recent events).

### Operations and help
- `/ops`  
  List ongoing operations (deploys, waits, tails). Attach/detach/stop.
- `/help`  
  Searchable command reference.
- `/keys`  
  Keybindings cheat sheet.

---

## Receipts

Every mutation ends with a receipt card.

Minimum fields:
- operation name
- timestamp
- affected resource IDs (release, workloads, endpoints, volumes)
- desired state summary
- current state summary (if immediately known)
- follow-ups:
  - how to wait
  - how to describe
  - how to rollback

Example receipt (conceptual):
- Operation: Deploy
- Release: `rel_01H...`
- App/Env: `myapp@prod`
- Status: Converging (2 workloads pending)
- Wait: `ghost release wait rel_01H...`
- Next: `/events`, `/logs`, `/workloads`

---

## Concurrency model

The TUI supports multiple active operations without turning the feed into noise.

- Each long-running action creates an operation handle.
- The feed shows a compact header for active ops with:
  - status
  - elapsed time
  - attach/detach shortcuts
- `/ops` is the control surface to manage them.

Streaming (logs/events) is:
- collapsed by default once it exceeds a threshold
- resumable
- filterable live

---

## Error handling

- Errors are shown as cards with:
  - human summary
  - machine code (matches CLI exit semantics)
  - suggested fix
  - “show details” expansion (request ID, server error, stack trace if available)
- Distinguish:
  - validation errors (manifest, flags)
  - auth errors (token scope, expired)
  - transient control-plane errors (retry guidance)
  - eventual consistency drift (expected, show wait path)

---

## Keybindings (v1)

- `/` focuses input (or inserts `/` when already focused)
- `Ctrl+P` opens command palette (same as slash search)
- `Tab` cycles focus: input -> feed -> sidebar -> input
- `Enter` submits input
- `Esc` cancels current prompt/selection
- `Space` toggles collapse on selected card
- `g/G` scroll top/bottom of feed
- `?` opens help overlay

Mouse support is optional but useful:
- click to select a card
- scroll feed and sidebar independently

---

## Configuration

- Uses the same auth and context store as CLI.
- Reads environment variables that influence CLI behavior (where safe).
- TUI-specific config:
  - default sidebar width ratio
  - compact vs detailed cards
  - streaming collapse threshold
  - keybinding preset (future)

---

## Security notes

- Never print secret values, even if the CLI can render them locally.
- Redact tokens and credentials in all output and debug bundles.
- Prefer OS keychain integration for token storage where available.

---

## Implementation approach

### Code organization
- `core/` shared library:
  - auth + context
  - API client
  - manifest parsing, validation, diff
  - wait primitives
  - event/log stream client
- `cmd/ghost/` CLI front-end:
  - thin wrappers over core
- `tui/` TUI front-end:
  - flow state machines
  - rendering layer (cards, lists, streams)
  - operation manager (`/ops`)

### Flow engine
Model each slash command as a small state machine:
- gather inputs -> validate -> plan/diff -> confirm -> execute -> stream -> receipt

This keeps behavior consistent and testable.

---

## Acceptance criteria for v1

- Users can deploy from a manifest end-to-end with the env/secrets gate, see a diff, confirm, and watch convergence.
- Users can tail events and logs without drowning the main feed.
- Every TUI mutation is reproducible as a CLI command shown in the feed.
- The sidebar always shows correct context and a truthful desired vs current indicator.
- Receipts include stable IDs and next-step commands.

---

## Example session transcript (conceptual)

User:
- `/context`

System:
- Select org:
  1) acme
  2) personal
- Select project:
  1) ghostty
  2) chapterwebsite
- Select app:
  1) api
  2) web
- Select env:
  1) staging
  2) prod

User:
- `2`

System:
- Context set: `acme/ghostty/api@prod`
- Equivalent CLI: `ghost context use acme/ghostty/api@prod`

User:
- `/deploy`

System:
- Manifest detected: `./ghost.toml`
- Env/secrets gate:
  - Required env: `DATABASE_URL`, `REDIS_URL`
  - Secret bundles: `prod-core`
  Options:
  1) Import env from file
  2) Set env interactively
  3) Acknowledge none required (disabled, requirements detected)

User:
- `1`

System:
- Import source?
  1) .env file
  2) shell export format
  3) paste inline
- Equivalent CLI will be shown after import

System:
- Plan (diff):
  - New release will update image to `sha256:...`
  - Endpoint unchanged
  - Workload `api` rolling update
- Confirm deploy by typing `api`

User:
- `api`

System:
- Deploy started. Operation: `op_01H...`
- Streaming events (collapsed after threshold)
- Receipt:
  - Release: `rel_01H...`
  - Status: Converging
  - Wait: `ghost release wait rel_01H...`
  - Next: `/logs workload=api`, `/workloads`, `/endpoints`

---


update:

## Persistent streams and interactive sessions (logs, events, ssh)

Some commands produce a continuous stream (logs, events) or require a real interactive TTY (ssh, exec). These do not belong in the main dialog feed long-term.

### Design: Live Pane (toggleable, persistent)

Add a **Live Pane** that can be shown or hidden. It hosts:
- streaming output sessions (logs, events)
- interactive PTY sessions (ssh, exec)

When hidden, sessions keep running in the background and continue buffering output. The main feed only shows a compact “session running” card with a handle you can reattach to.

### Layout update

- Left 2/3: dialog feed (commands, selections, receipts)
- Right 1/3: status + inspector
- Optional: **Live Pane drawer** (bottom or overlay), toggleable

Recommended default: bottom drawer at ~35 to 45% height, resizable later.

### Session behavior

Each Live Pane session has:
- `session_id` (stable within the TUI runtime)
- `kind`: `logs | events | ssh | exec`
- `source`: the equivalent CLI command line
- state: running, stopped, error, detached
- bounded buffer (last N lines for streams; scrollback for PTY if feasible)

Rules:
- Streams (logs/events) should default to Live Pane once output becomes “continuous”.
- Interactive commands (ssh/exec) always run in Live Pane or an external terminal fallback.
- The feed never becomes a log waterfall. It only shows entry, filters, and a summary.

### Fallback strategy for v1 (important)

Interactive PTY embedding can be finicky across platforms. v1 should support:
1) Preferred: run `ssh/exec` inside a PTY-backed Live Pane
2) Fallback: if PTY not supported, open a separate shell session:
   - if inside tmux: `split-window` or `new-window`
   - if inside zellij: `new-pane`
   - otherwise: spawn `$TERMINAL` (best effort) or print an exact command for the user to run

In all cases, the feed still prints the equivalent CLI command.

---

## Slash commands changes (v1)

### Observability
- `/logs [filters...] [--pane|--feed]`
  - default: `--pane`
  - `--feed` prints a short snapshot plus a “attach to pane” action
- `/events [filters...] [--pane|--feed]`
  - default: `--pane`

### Interactive
- `/ssh <workload|instance> [--pane|--external]`
  - default: `--pane` (or fallback to external if unsupported)
- `/exec <workload|instance> -- <cmd...> [--pane|--external]`
  - default: `--pane`

### Pane management
- `/pane`
  - shows current pane sessions and quick actions (attach, detach, stop)
- `/pane attach <session_id>`
- `/pane stop <session_id>`
- `/pane clear <session_id>`

---

## Concurrency model update

Long-running streams and interactive sessions are tracked as operations and are discoverable via:
- `/ops` (all operations)
- `/pane` (pane sessions only)

When the Live Pane is hidden:
- sidebar shows a small indicator: “Pane: 2 running”
- the feed shows compact cards like:
  - “Logs session running: `pane_01H...` (filters: workload=api) [Attach] [Stop]”

---

## Keybindings update (v1)

Add:
- `Ctrl+L` toggles Live Pane show/hide (when focus is NOT inside an interactive PTY)
- `Esc` returns focus to input (and can also exit selection mode)
- `Ctrl+W` hides the Live Pane when focus IS inside an interactive PTY (so `Ctrl+L` can pass through to the shell for clear-screen)

Optional (nice to have):
- `Ctrl+J / Ctrl+K` cycles between pane sessions when the Live Pane is visible
- `Ctrl+F` search within stream buffer (streams only, not PTY)

Note: `Ctrl+L` is traditionally “clear screen” in shells. The rule above avoids stealing it while the user is actively inside ssh/exec.

---

## Acceptance criteria additions (v1)

- Users can start `/logs` and `/events` and keep them running while continuing to use slash commands in the main feed.
- Users can start `/ssh` and interact normally, then hide the Live Pane and return later without losing the session (or fall back to an external terminal session cleanly).
- The feed remains readable: continuous output does not drown receipts and diffs.

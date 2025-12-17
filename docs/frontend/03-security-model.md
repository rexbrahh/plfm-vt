# Security model

This document defines the security posture for the web terminal.

The web terminal is high risk because it is an interactive remote execution surface. The default stance is: minimize privilege, constrain blast radius, and make sessions auditable.

## Threat model (high level)

We defend against:

- Token theft leading to unauthorized shell access
- Cross site attacks (CSRF, clickjacking, XSS) that cause unintended terminal actions
- Session fixation or replay
- Lateral movement from terminal host to control plane or other tenants
- Data exfiltration via unintended network access
- Abuse (crypto mining, scanning, denial of service)

We do not try to defend against a fully compromised user machine.

## Identity and authentication

- The console web app authenticates the user (OIDC, SSO, etc).
- A terminal session is created via a console API call that returns:
  - `session_id`
  - a short lived `token` bound to that session id and user
- WebSocket auth:
  - token presented only in the initial `hello` frame (not URL query string)
  - server validates token, user, and session scope

Token properties:
- short TTL (minutes)
- single use if feasible
- audience restricted to terminal gateway

## Authorization

A terminal session must be scoped:

- org + project + app + env context
- allowed capabilities, for example:
  - "run platform CLI only" (locked down)
  - "shell in sandbox container"
  - "ssh into workload instance" (only if permitted)

Avoid giving a general purpose root shell in v1 unless the product explicitly needs it.

## Isolation

Preferred isolation stack:

- Each terminal session runs in its own sandbox:
  - container with seccomp + no privileged
  - or microVM (better)
- No access to the control plane private network by default.
- Egress allowlist:
  - platform API endpoints
  - package registries only if needed

If the terminal is intended to be a "CLI runner", it should not be a general purpose Linux box.

## Browser security

- Strict Content Security Policy (CSP)
- No inline scripts
- SameSite cookies
- WebSocket origin checks
- Clickjacking protection (frame-ancestors)

The terminal renderer must treat all output as untrusted text. Never interpret it as HTML.

Hyperlinks:
- If we support OSC 8 hyperlinks, only allow safe schemes (`https`, maybe `ssh` if handled).
- Show a hover preview and require click.

### WebAssembly and cross origin isolation

Because we embed `libghostty-vt` in wasm, the terminal inherits typical wasm supply chain and browser isolation concerns:

- The wasm bundle must be served from the same origin as the app, with a strict CSP to reduce XSS risk.
- Prefer Subresource Integrity (SRI) for the wasm asset if it is ever served from a CDN.
- If we use `SharedArrayBuffer` (for worker rendering or shared rings), the app must be cross origin isolated:
  - `Cross-Origin-Opener-Policy: same-origin`
  - `Cross-Origin-Embedder-Policy: require-corp`

Cross origin isolation is both a performance enabler and a security boundary. Without it, we must fall back to a main thread or message passing design.


## Input safety

Terminal input is powerful. We do not try to sanitize shell input.

Instead:
- make it hard for other websites to send keystrokes:
  - focus must be inside terminal
  - disable "typeahead" when not focused
- make paste explicit:
  - large paste shows a confirmation (optional)
  - allow paste preview for multiline

## Audit and observability

At minimum, record metadata (not full content):

- session start and end
- user id
- IP / device info
- session scope (org/project/app/env)
- attach and detach events
- bytes in/out counts
- abnormal events (drops, resume failures)

Full keystroke logging is usually a bad idea for privacy. If we ever support recording, it must be explicit opt-in with clear retention.

## Rate limiting and abuse controls

- Per user and per org session limits
- Per session bandwidth caps
- Idle timeouts (with keepalive allowance for long running tasks)
- Captcha or extra verification if abuse patterns detected (later)

## Secrets handling

If the terminal host needs credentials:

- do not bake long lived secrets into the host image
- use short lived tokens fetched at session start
- store secrets in memory only
- avoid writing secrets to disk
- when possible, use the same "secret file delivery" format the platform uses, but with minimal scope

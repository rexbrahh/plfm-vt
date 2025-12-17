# Errors and exit codes

This document defines the CLI error contract and exit code semantics.

The CLI is the product. Errors must be actionable for humans and predictable for scripts.

## Goals

- Stable exit codes across releases
- Clear separation between:
  - user mistakes (fix your input)
  - platform state (wait or investigate)
  - transient failures (retry)
- Machine readable error output under `--json`
- No secrets leakage in errors or debug logs

## Non-goals

- Mirroring raw API errors verbatim
- Encoding internal implementation details into the public contract

## Error principles

### 1) Actionable by default
Every error should include, when possible:
- what failed
- why it failed
- how to fix it (one concrete command or flag)
- what to inspect next (status, events, logs)
- a trace id when available

### 2) Stable exit codes
Exit codes are a public interface. Do not change meanings without a major version bump.

### 3) Errors are structured
- Human mode: readable message with a clear hint
- JSON mode: a stable schema that is safe for automation

### 4) Retryability is explicit
Where possible, errors should state whether a retry is likely to succeed.

### 5) No secrets in output
Secret values must never appear in:
- error messages
- JSON error payloads
- debug logs
- traces

## Exit code table

| Exit code | Name | Meaning | Retry guidance |
|---:|---|---|---|
| 0 | success | Command succeeded | N/A |
| 1 | failure | Generic failure with no better category | Maybe |
| 2 | invalid_usage | Invalid flags, bad syntax, failed local validation | No |
| 3 | auth | Authentication required or permission denied | No (until fixed) |
| 4 | not_found | Resource does not exist or is not visible | No |
| 5 | conflict | Precondition failed or conflicting state | Sometimes |
| 6 | transient | Temporary failure (network, timeouts, service unavailable, rate limits) | Yes |
| 7 | internal | CLI bug or unexpected platform error | Maybe |

Notes:
- For scripts, treat 6 as retryable by default with backoff.
- 5 is often user fixable (for example wrong env, wrong release, stale resource version) but may also resolve after convergence.

## JSON error schema

When `--json` is set, commands must output valid JSON and nothing else.

Recommended schema:

```json
{
  "error": {
    "category": "invalid_usage | auth | not_found | conflict | transient | internal | failure",
    "message": "Human readable summary",
    "hint": "One actionable next step",
    "retryable": true,
    "trace_id": "trc_123",
    "details": {
      "command": "vt deploy",
      "resource": {
        "type": "app | env | release | endpoint | volume | instance",
        "id": "app_123",
        "name": "hello"
      },
      "fields": {
        "env": "prod"
      }
    }
  }
}

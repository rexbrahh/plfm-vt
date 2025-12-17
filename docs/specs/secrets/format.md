# docs/specs/secrets/format.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
Define the exact on-disk format for the secrets file that is delivered into each microVM.

Locked decision: secrets are delivered as a platform-managed file with a fixed format. See `docs/adr/0010-secrets-delivery-file-format.md`.

This spec defines:
- file syntax
- versioning rules
- canonicalization (for hashing and idempotency)
- permissions expectations (format-adjacent)

Delivery timing and restart semantics live in:
- `docs/specs/secrets/delivery.md`

Encryption-at-rest for stored secrets lives in:
- `docs/specs/secrets/encryption-at-rest.md`

## File identity
### Fixed mount path (v1)
- `/run/secrets/platform.env`

This path is reserved and not configurable in v1.

### Encoding
- UTF-8 text file
- Line endings may be `\n` or `\r\n` on input
- Canonical form uses `\n` only

## Versioning
The first non-empty line MUST be the header:

- `# TRC_SECRETS_V1`

Rules:
- If the header is missing, reject.
- If the header is unknown (not `TRC_SECRETS_V1`), reject.
- Comment lines after the header are allowed, but only as full-line comments.

Future versions:
- v2 would use `# TRC_SECRETS_V2` and would be defined by a new spec update.

## Syntax (v1)
After the header, the file contains zero or more entries and optional comment lines.

### Allowed line types
1) Header line:
- `# TRC_SECRETS_V1`

2) Comment line:
- begins with `#` as the first character
- comment lines are ignored by parsers
- inline comments are not supported

3) Entry line:
- `KEY=VALUE`

Where:
- `KEY` matches: `[A-Z_][A-Z0-9_]*`
- `VALUE` is everything after the first `=` up to the line ending, including spaces

Empty lines:
- Empty or whitespace-only lines are allowed and ignored.

### VALUE encoding
There are two value forms:

#### Plain value
`VALUE` is interpreted as a UTF-8 string as-is (no unescaping).

Constraints for plain value:
- Must not contain `\n` or `\r` (line-based format).
- Must not contain NUL (`\u0000`).

If a secret value requires bytes that violate these constraints, it MUST use the base64 form.

#### Base64 value
`VALUE` may be encoded as:

- `base64:<B64>`

Where:
- `<B64>` is RFC 4648 base64 using the standard alphabet
- `<B64>` MUST NOT contain whitespace
- Padding `=` is allowed and recommended

Semantics:
- The decoded bytes are the secret value bytes.
- The decoded bytes may be any bytes (including non-UTF8).

## Parsing rules (normative)
Given input bytes:
1) Decode as UTF-8. If invalid UTF-8, reject.
2) Normalize line endings:
- accept `\n` and `\r\n`
- treat `\r` not followed by `\n` as invalid (reject)
3) Skip leading empty/whitespace-only lines until the header.
4) Require header line exactly `# TRC_SECRETS_V1`.
5) For each subsequent line:
- if empty/whitespace-only: ignore
- if starts with `#`: ignore
- else must contain a `=` with at least one char before it (the key)
- split on the first `=` only
- validate key pattern
- parse value:
  - if value starts with `base64:`, parse and base64-decode, or reject if invalid
  - else interpret as plain UTF-8 string bytes (excluding line ending)

Duplicate keys:
- Not allowed in canonical form.
- For non-canonical input, parsers MUST reject duplicate keys to avoid ambiguity.

## Canonicalization and hashing (normative)
The platform computes a content hash for idempotency and audit (`data_hash`) from the canonical form.

### Canonicalization algorithm
Given parsed secrets as a map `KEY -> bytes`:
1) Sort keys lexicographically ascending (byte order of ASCII keys).
2) For each key, choose representation:
- If value bytes are valid UTF-8 AND do not contain `\n`, `\r`, or NUL:
  - represent as plain value with that exact UTF-8 string
- Otherwise:
  - represent as `base64:<B64>` where `<B64>` is base64-encoded bytes with padding and no newlines
3) Emit output with:
- First line: `# TRC_SECRETS_V1\n`
- Then one entry per line: `KEY=VALUE\n`
4) No extra comments are emitted in canonical form.
5) Canonical output MUST end with a trailing newline.

### Hash
- `data_hash = sha256(canonical_bytes)`
- Represented as lowercase hex string.

Rules:
- All codepaths that accept secrets (API, CLI, internal tooling) MUST canonicalize the same way and produce the same hash for the same logical secret set.

## Size and limit constraints (v1)
These are validation limits to protect platform stability.

Recommended defaults (operator-configurable):
- Max canonical file size: 256 KiB
- Max number of keys: 256
- Max key length: 128
- Max plain value length (bytes): 8192
- Max base64 value decoded length (bytes): 65536

If limits are exceeded:
- Reject the secrets update with `invalid_argument` and a clear error message pointing to the offending key.

## Permissions (v1 expectations)
The secrets file is sensitive and must be readable only by the workload identity.

Default (v1):
- Path: `/run/secrets/platform.env`
- Owner: `uid=0`, `gid=0`
- Mode: `0400`

Optional (when workload runs as non-root and needs access):
- Owner: workload `uid`, `gid`
- Mode: `0400`

Group-readable is allowed only if explicitly configured by the platform:
- Mode: `0440`
- Group must be a dedicated app group, not a shared group.

Never allowed:
- world-readable secrets files (`04xx` where other has read)
- writing secrets to persistent disk by default (this file should live on tmpfs under `/run`)

## Examples

### Plain values
```text
# TRC_SECRETS_V1
DB_HOST=db.internal
DB_USER=appuser
DB_PASS=correct horse battery staple
```

### Base64 values (binary or multiline)
# TRC_SECRETS_V1
```
TLS_PRIVATE_KEY=base64:LS0tLS1CRUdJTiBQUklWQVRFIEtFWS0tLS0tCk1JSUV2...
```

### Mixed
# TRC_SECRETS_V1
```API_TOKEN=base64:AAEC/f8=
LOG_LEVEL=info
```
## Security notes

This format is intentionally simple and deterministic to reduce parser differences across languages.

Do not log file contents.

Treat any endpoint that returns secret material as high risk. v1 should avoid it by default.

::contentReference[oaicite:0]{index=0}

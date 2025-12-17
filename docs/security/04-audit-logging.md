# Audit logging

Last updated: 2025-12-17

This document specifies the audit logging requirements for the platform. Audit logs exist to support incident response, forensics, and customer trust. They are not the same as workload logs.

## Goals

- Provide a complete record of security relevant actions and decisions.
- Make logs hard to tamper with and easy to validate.
- Support multi tenant access controls and exports.
- Avoid leaking sensitive data into audit logs.

## What must be logged

### Authentication events

- Login success and failure (including MFA success and failure).
- Token issuance, refresh, and revocation.
- Session creation and termination for web console.
- Service account token creation and rotation.

### Authorization decisions

- All allow and deny decisions for sensitive actions.
- Policy version and rule id used for the decision.
- Any step up auth requirements triggered.

### State mutation events

For every mutation API call:

- Resource type and immutable id
- Action and parameters summary (no secrets)
- Actor identity and scope
- Request id and idempotency key if present
- Result (success or failure) and error class

Examples:
- Create or update app, env, release, endpoint, volume, secret bundle.
- Scale or restart workloads.
- Attach or detach volumes.
- Snapshot create, restore, delete.
- Endpoint changes including Proxy Protocol v2 enablement.

### Runtime access events

- Starting and stopping interactive exec sessions.
- Accessing logs and events streams (including filters).
- Downloading artifacts or configuration bundles.
- Any access to support tooling that touches customer environments.

### Host and infrastructure events

- Host registration and key rotation.
- Host agent upgrades and configuration changes.
- Image fetch and verification results.
- Integrity check failures and security policy enforcement actions.

## What must not be logged

- Secret values, private keys, tokens, or raw credentials.
- Full request or response bodies for endpoints that can contain secrets.
- Workload log lines (those belong in the logging system with separate controls).

If an error message could contain secrets, the error must be redacted before it is logged.

## Event schema

Minimum required fields for each audit record.

- `timestamp`
- `event_id` (unique, sortable)
- `request_id`
- `actor_type` (user, service, node)
- `actor_id`
- `actor_org_id`
- `scopes` (set of scope strings)
- `action`
- `resource_type`
- `resource_id`
- `resource_path` (org, project, app, env ids)
- `decision` (allow, deny)
- `reason` (short, structured)
- `source_ip`
- `user_agent` (when relevant)
- `result` (ok, error)
- `error_class` (optional, no sensitive data)
- `policy_version` (optional but recommended)

## Tamper evidence and retention

### Tamper evidence

Recommended options:

- Hash chain records per partition (for example per org) and publish periodic checkpoints.
- Write once storage for long term retention where feasible.
- Separate audit log write path from the main control plane state store.

### Retention

- Default retention window (for example 90 days) with paid longer retention tiers.
- Allow customer export to their own storage.
- Support legal hold flags that prevent deletion for a specified time period.

## Access control

- Audit logs are scoped by org and project.
- Only roles with explicit `audit:read` may view audit logs.
- Only owners and admins can configure exports and retention settings.
- Support staff access should be exceptional, time limited, and audited as well.

## Correlation and usability

- Every user visible mutation response should return a request id that can be used to find the matching audit records.
- CLI and web console should provide `events` and `audit` views that show:
  - desired vs current state changes
  - who did what and when
  - links to the resources involved

## Privacy considerations

- Treat IP addresses and user agents as potentially personal data.
- Provide configuration to mask or limit exposure of such fields to non admin roles.
- Ensure audit logs do not include free form user input unless sanitized.

## Testing

- Verify audit record creation for every mutation endpoint.
- Verify deny decisions are logged for sensitive paths.
- Verify secret redaction in all code paths, including error handling.
- Chaos tests for audit log pipeline outages: platform must fail closed for sensitive actions or degrade safely with clear messaging.

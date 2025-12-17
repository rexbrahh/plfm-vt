# Incident response

Incidents are inevitable. The difference is how fast we mitigate and how well we learn.

## When to declare an incident

Declare an incident when:

- a customer-facing SLO is burning quickly
- multiple customers report similar failures
- core platform functions are impaired (deploy, ingress, secrets, volumes)
- you are unsure but suspect broad impact

It is always cheaper to declare early than to hide uncertainty.

## Severity levels

These levels drive urgency and communication cadence.

- **Sev0**: major outage, widespread customer impact, core platform unusable
- **Sev1**: significant impact, partial outage, many customers affected
- **Sev2**: limited impact, subset of customers or one region, workarounds exist
- **Sev3**: minor impact, small blast radius, no immediate action required

## Incident roles

Assign roles early.

- **IC (incident commander)**: makes decisions, keeps focus on mitigation
- **Ops lead**: executes technical actions and delegates tasks
- **Comms lead**: customer updates, status page, internal summaries
- **Scribe**: timeline notes, links, decisions, metrics snapshots

If short-staffed, IC can combine roles, but keep a written timeline.

## Incident workflow

### 1) Triage and stabilize

- Identify scope: region, edge, control plane, specific apps
- Stop the bleeding: rollback, rate limit, cordon, drain, disable feature flags
- Reduce blast radius: isolate failing hosts or edge nodes

### 2) Communicate

- Start internal incident channel and incident doc
- Post initial status update (even if uncertain) within 15 minutes for Sev0/Sev1
- Provide regular updates:
  - Sev0: every 15 minutes
  - Sev1: every 30 minutes
  - Sev2: every 60 minutes

### 3) Mitigate

- Prefer rollback to stabilize
- Prefer safe mitigations over perfect diagnosis
- Record all actions and results

### 4) Verify and resolve

- Confirm SLIs return to normal
- Confirm no hidden backlog remains (reconcile queue, Postgres lag, edge drains)
- Remove temporary mitigations carefully
- Close incident with final update

### 5) Postmortem

- Required for Sev0/Sev1, recommended for Sev2 with actionable learning
- Publish within 5 business days
- Track action items to completion

## Communications templates

### Initial update

- What is happening
- Who is affected (scope)
- What we are doing now
- Next update time

### Resolution update

- What was impacted
- When it started and ended
- What we changed to mitigate
- Where to follow along for postmortem (if applicable)

## Production freeze policy

During Sev0/Sev1:

- freeze non-essential deploys
- only ship changes that mitigate the incident or prevent recurrence
- document any production change in the incident timeline

## Escalation guidelines

Escalate to owners when:

- mitigation requires privileged access or deep system knowledge
- suspected data loss or corruption
- security risk is involved
- the incident lasts > 30 minutes without improvement (Sev0/Sev1)

Escalate to leadership for:

- Sev0 incidents
- any public communications beyond the status page
- regulatory or legal risk

## Runbook links

Common incident runbooks:

- `docs/ops/runbooks/control-plane-down.md`
- `docs/ops/runbooks/edge-partial-outage.md`
- `docs/ops/runbooks/host-degraded.md`
- `docs/ops/runbooks/postgres-failover.md`
- `docs/ops/runbooks/wireguard-partition.md`
- `docs/ops/runbooks/firecracker-failure.md`
- `docs/ops/runbooks/volume-corruption.md`

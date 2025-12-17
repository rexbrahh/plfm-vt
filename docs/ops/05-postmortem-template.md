# Postmortem template

Copy this file for each incident postmortem.

---

## Incident summary

- **Incident ID**:
- **Severity**: Sev0 / Sev1 / Sev2 / Sev3
- **Start time** (UTC):
- **End time** (UTC):
- **Duration**:
- **Primary services impacted**:
- **Customer impact summary**:

## Impact

- **Customer facing symptoms**:
- **Regions impacted**:
- **% of customers affected** (estimate):
- **Requests affected** (error rates, timeouts):
- **Data impact**:
  - Data loss: yes/no (details)
  - Data corruption: yes/no (details)
  - Durability risk: yes/no (details)

## Detection

- **How was it detected**:
  - Alert name(s):
  - Customer reports:
  - Internal observation:
- **Time to detect**:
- **Why did our alerting succeed or fail**:

## Timeline

Use precise times. Include links to dashboards, logs, PRs, and commands run.

| Time (UTC) | Actor | Action | Result |
|---|---|---|---|
| | | | |

## Root cause

- **Direct trigger**:
- **Root cause** (systems explanation):
- **Contributing factors**:
  - product / code
  - config / deployments
  - capacity
  - process / oncall
  - third party dependencies

## Mitigation and resolution

- **What stopped the bleeding**:
- **What fully resolved the incident**:
- **Rollback details** (if used):
- **Customer workarounds** (if any):

## What went well

- 

## What went poorly

- 

## Where we got lucky

- 

## Action items

Action items must be specific, owned, and dated.

| Priority | Action item | Owner | Due date | Status | Verification plan |
|---:|---|---|---|---|---|
| P0 | | | | | |
| P1 | | | | | |
| P2 | | | | | |

Priority guidance:

- **P0**: prevents recurrence of Sev0/Sev1 or reduces blast radius dramatically
- **P1**: meaningful reduction in risk or time-to-mitigate
- **P2**: nice to have, tooling, or documentation improvements

## Follow-up verification

- How will we prove this is fixed?
- What metrics should improve?
- What game day will we run?

## Appendices

- Links:
  - incident doc:
  - status page:
  - relevant PRs:
  - dashboards:
- Notes and raw logs (short excerpts only):

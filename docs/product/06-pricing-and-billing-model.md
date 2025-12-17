# Pricing and billing model

This document proposes a pricing model that maps directly to the resources we allocate and reconcile.

Exact prices can change, but billing should stay:
- understandable
- predictable
- aligned with actual costs (compute, storage, bandwidth, IPv4)

## Billing units

### Compute
Compute is billed based on allocated resources, not “best effort” usage:

- vCPU time (vCPU-seconds or vCPU-minutes)
- RAM time (GiB-seconds or GiB-minutes)

This aligns with a microVM model where we reserve capacity per instance.

### Storage
- Volume storage (GiB-month)
- Snapshot storage (GiB-month), if snapshots exist in v1 or shortly after
- Optional backup storage (GiB-month), if backups exist later

### Network
- Egress (GiB)
- Ingress is typically free (depends on provider costs, but this is a common model)

### IPv4
- Dedicated IPv4 add-on billed per allocation period (hourly or monthly)
- If IPv4 is attached to an endpoint, it should be clear what you are paying for

## Example bill components

For an org in a billing period:

- Compute = sum over instances of (vCPU_allocated * time_running) + (RAM_allocated * time_running)
- Storage = sum over volumes of (GiB_provisioned * time_provisioned)
- Egress = sum over endpoints of (GiB_egress)
- IPv4 = sum over IPv4 allocations of (time_allocated)

## Pricing UX

### Always show estimated impact before applying changes
For mutations that affect billing (scale up, volume resize, IPv4 enablement), the CLI should display:

- what will change
- the estimated new monthly cost (rough)
- how to view exact usage later

### Bills should be explainable from first principles
The user should be able to run a command that answers:

- “Why did my bill change?”
- “Which app or environment is costing me the most?”
- “How much is IPv4 costing me?”

### Credits and trials
If we offer free credits:
- credits should apply cleanly to the same line items (compute, storage, IPv4)
- the user should see remaining credits and projected burn rate

## Pricing philosophy

- **Avoid surprise**: users should not learn the cost model by receiving a bill.
- **Charge for scarcity**: IPv4 is the clearest example.
- **Stay aligned with the product**: if we encourage explicit endpoints and explicit scale, pricing should reinforce that clarity.

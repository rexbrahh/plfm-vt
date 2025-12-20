# Performance testing

Performance is a product feature here: fast CLI feedback, quick converge-to-desired, and predictable runtime behavior.

## What we measure

Customer-visible:
- CLI perceived latency (p50, p95, p99) for:
  - list, describe, status
  - manifest apply
  - release create and wait
  - logs tail start time

Platform-critical:
- reconcile convergence time from desired change to stable current state
- image pull latency and cache hit rate
- cold start time for workloads
- L4 ingress throughput and tail latency under load
- event/log stream throughput and backpressure behavior

## Performance environments

- Baseline hardware profile: a fixed, documented machine type.
- Isolate perf runs from noisy neighbors when possible.
- Record:
  - CPU model
  - kernel version
  - storage type
  - network interface details

## Tools (suggested defaults)

- Microbenchmarks:
  - Rust: criterion
- Load generation:
  - k6, vegeta, wrk
- Profiling:
  - pprof, perf, flamegraph tooling
- Network:
  - iperf3, netperf

## Methodology

- Warm up before measurement.
- Use fixed seeds and deterministic fixtures.
- Run enough iterations to detect regressions, not just single runs.
- Record raw numbers and computed percentiles.

## Regression gates

Once a baseline exists, add CI perf checks:
- Allow small variance, but fail on clear regressions.
- Gate on p95 for critical paths.
- Store historical trends (even a simple artifact archive is fine early).

## Scenario library (start small, expand)

Scenario A: Many small apps
- N apps, each with a small workload, frequent updates.

Scenario B: Secrets-heavy deploy
- Many secret bundles, frequent rotations, ensure reconcile remains stable.

Scenario C: Log-heavy workloads
- High log volume, ensure tailing remains responsive and backpressure is safe.

Scenario D: Network stress
- Many concurrent L4 connections, long-lived TCP sessions, proxy protocol toggles.

## Reporting

Each perf run should output:
- git SHA
- environment metadata
- scenario name
- p50/p95/p99
- max memory and CPU usage
- any errors or dropped events

Publish a short perf summary per release.

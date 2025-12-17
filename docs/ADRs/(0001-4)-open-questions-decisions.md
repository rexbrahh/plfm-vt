Agreed. Leaving open questions hanging is how projects drift. We’ll tighten them up one ADR at a time.

## ADR 0003: Recommendations for the open questions

### 1) Root filesystem strategy

**Recommendation (v1):**

* **Per release:** node agent builds a **read-only ext4 root disk** from the OCI image (content-addressed by image digest).
* **Per microVM instance:** attach a small **writable ext4 “scratch” disk**.
* **In guest init:** create an **overlayfs root**:

  * lowerdir: mounted read-only release root disk
  * upperdir + workdir: on the scratch disk
  * pivot_root into the overlay mount, then start the app

**Why this is the staff-engineer choice:**

* It matches container expectations (apps can write to paths like `/var`, `/tmp`, etc) without you copying whole rootfs per instance.
* It keeps caching simple (one root disk per digest) while still giving each instance isolation for writes.
* It avoids host-side overlay and filesystem-specific tricks. It works consistently on ext4 hosts.

**Explicit non-goal (for v1):**

* No qcow2 layering, dm-snapshot, composefs, or verity. You can add those later if density becomes the bottleneck.

---

### 2) Guest init: custom vs standard init

**Recommendation (v1): custom minimal init (single static binary) as PID 1.**

Responsibilities:

* mount `/proc`, `/sys`, `/dev`, and required pseudo filesystems
* set up overlay root (above)
* configure networking (enough to reach overlay and control plane)
* mount secrets file and volumes at fixed locations
* launch the entrypoint, forward signals, reap zombies, capture exit code

**Why:**

* You want a predictable boot path, tiny attack surface, and fewer moving parts.
* Systemd brings a lot of implicit behavior you will end up fighting when debugging microVM lifecycle.

---

### 3) How standardized is the guest environment

**Recommendation (v1): tight standardization on the platform side, loose inside the image.**

Standardize:

* kernel and boot args
* init binary and its contract
* fixed mount points:

  * `/run/secrets/<bundle>` (or whatever you pick) for secrets
  * `/mnt/volumes/<name>` for attached volumes
* a small set of environment variables injected by platform (release id, env name, instance id, region, etc)
* log transport contract (serial console or vsock, but one standard)

Do not standardize:

* distribution inside the image
* package manager
* shell availability

**Run as root or not:**

* **Default to root inside the guest for v1 compatibility**, but support `user` in the manifest to drop privileges.
* Rationale: “non-root by default” breaks a surprising number of images. The microVM boundary is doing the heavy isolation work in v1.

If you’re good with these, say “proceed” and I’ll do the same tightening pass for **ADR 0004 (WireGuard full mesh)** next.

You’re right. Let’s close the open loops properly, in the same “explicit, implementable” style.

## ADR 0001: Isolation microVM per environment

### Open question: Do we allow multiple process types in the same env inside one microVM, or separate microVMs per process type?

**Recommendation (v1): one process type per microVM instance.**
An environment can define multiple process types, but each process type scales into its own set of microVMs. A microVM instance runs exactly one user entrypoint (plus optional platform-managed helpers if absolutely necessary).

**Concrete model (this matters for your manifest and scheduler):**

* `env` is the unit of config, secrets, and routing ownership.
* `process_type` is the unit of “what command runs” and how it scales.
* `instance` is one microVM running one process type.

Example conceptual mapping:

* `app=foo, env=prod`

  * `process=web` -> N microVM instances
  * `process=worker` -> M microVM instances

**Why this is the staff-engineer choice:**

* **Scaling and rollouts stay clean.** You can roll `web` without touching `worker`, or vice versa.
* **Resource accounting is honest.** Memory hard caps become meaningful per instance. No “worker ate the web memory” surprise.
* **Failure containment is sharper.** If the worker crashes or OOMs, it does not kill the web server in the same VM.
* **Networking is simpler.** Ports and health checks are per instance, not per internal supervisor.
* **Debuggability improves.** Logs, exit reasons, and restart policies map 1:1 to a microVM instance.

**What we explicitly do not support in v1:**

* “Multiple unrelated user processes in one microVM” as a density optimization.
* User-defined sidecars in the same microVM.

**Compatibility carveout (platform-managed only):**

* If you need an internal adapter (example: PROXY v2 stripping) or a tiny log forwarder, it can run in the microVM, but only if:

  * It is platform-managed, not user configurable.
  * It has a stable contract and minimal resource overhead.
  * It cannot access secrets beyond what the main process can already access.

**Manifest implication (you should lock this early):**

* Manifest has a `processes` map with a default `web` process.
* Each process defines:

  * `command` (entrypoint)
  * `ports` (if any)
  * `healthcheck`
  * `resources` (cpu request, memory cap)
  * `scaling` (min, max, autoscale later)
* Scheduler operates on `(env, process_type)` and produces instances.

---

## ADR 0002: Artifact is OCI image plus manifest

### Open question: Manifest format (TOML vs YAML vs JSON)

**Recommendation (v1): TOML as the canonical authoring format.**
Additionally: CLI can output a canonical normalized JSON form for tooling, but the on-disk user-facing manifest is TOML.

**Why TOML over YAML and JSON:**

* **YAML has too many footguns** (implicit types, multiple parser behaviors, surprising edge cases). This becomes a long-term support tax.
* **JSON is too strict for humans** (no comments, noisy diffs, no trailing commas). People will hate writing it.
* **TOML is typed enough to validate reliably**, supports comments, is readable, and has consistent parsers across Go and Rust.

**Practical policy to make TOML safe:**

* Define the schema in one place (JSON Schema or an equivalent typed schema in code).
* Provide `platform validate` that:

  * rejects unknown fields by default (or has a strict mode)
  * prints precise errors with paths
* Provide `platform fmt` that produces a canonical ordering and formatting.

This prevents “creative manifests” from becoming production incidents.

---

### Open question: Multi-arch support in v1 (single-arch only vs OCI index)

**Recommendation (v1): accept OCI indexes, but record the resolved image digest per architecture.**

**Concrete behavior (no ambiguity in releases):**

* User deploys a digest that may point to:

  * a single image manifest, or
  * an OCI index (multi-arch)
* Control plane stores:

  * `release.image_ref_digest` (the digest the user provided)
  * if it is an index: `release.resolved_image_digest[arch]` once resolved
* Node agent pulls:

  * if single image: use it
  * if index: select matching `os=linux` and `arch` (and variant if needed), then pull the resolved image digest

**Why this is the right middle ground:**

* You avoid a future migration when you add arm64 hosts.
* You preserve auditability and reproducibility, because you can always say “this node ran digest X”.
* You keep v1 operationally simple because most of your fleet will be one architecture anyway.

**Hard rule for v1 correctness:**

* A “Release” used by a specific node must always be pinned to an immutable digest at execution time. Never run by tag.

---

## ADR 0004: Overlay WireGuard full mesh

### Open question: When we outgrow full mesh, what do we do?

**Recommendation: plan a two-step evolution path, with explicit thresholds.**

**v1 (full mesh):**

* Target: up to roughly 25 to 50 nodes. Past that, peer management and churn become annoying, even if technically workable.

**v2 (regional hubs, still WireGuard):**

* Introduce 2 to 3 hub nodes per region.
* Each worker node peers with the hubs (and optionally a small number of local peers).
* Hubs route overlay prefixes between nodes (control plane-managed).
* This reduces peer count from O(n²) to about O(n).

**Why hubs over jumping straight to “routed SDN”:**

* It is the smallest change that preserves your existing WireGuard identity model.
* Operationally debuggable and incremental.

---

### Open question: Node bootstrap and preventing rogue nodes

**Recommendation (v1): explicit enrollment with one-time tokens + node-generated keys + mTLS identity.**

**Bootstrap flow:**

1. Operator provisions a machine with a bootstrap config that includes:

   * control plane URL
   * a short-lived, single-use enrollment token (scoped to an org or cluster)
2. Node agent generates:

   * WireGuard keypair
   * an mTLS keypair for control plane RPC
3. Node agent calls `EnrollNode` over TLS:

   * presents enrollment token
   * submits public keys and basic hardware metadata
4. Control plane responds with:

   * node id
   * assigned overlay IPv6 addresses/prefixes
   * peer list (public keys and endpoints)
   * control plane CA bundle and the node’s signed mTLS cert (or instructions to fetch)

**Security posture:**

* Enrollment tokens are single-use and expire quickly.
* Control plane can require manual approval for new nodes if you want “human gate” early.
* Revocation is explicit: disable node, remove from peers, rotate keys.

This is enough to stop random internet hosts from joining your mesh.

---

### Open question: Overlay MTU and fragmentation

**Recommendation (v1): standardize MTU and do not break ICMPv6.**

**Concrete settings:**

* Set WireGuard interface MTU to **1420** by default.
* Set the host-side interfaces that carry workload traffic over the overlay to match (tap or veth side).
* Configure the guest network interface MTU to the same value (or slightly lower like 1400 if you see issues).

**Operational rule that prevents months of pain:**

* Do not block ICMPv6 “Packet Too Big”. If you block it, PMTUD breaks and you get mysterious hangs.

**Escape hatch:**

* Make MTU a cluster-level config so you can lower it for specific underlay environments without redeploying everything.

---

If you want, next we can take these recommendations and patch them back into the ADR files as “Open questions resolved” sections so they are truly closed and not re-litigated later.

## ADR 0009: Open questions and recommendations (PROXY Protocol v2)

### Open question 1: Default policy

Whether client identity propagation is enabled by default for public routes, or opt-in per route for compatibility.

**Recommendation (v1): opt-in per Route, default OFF.**

**Why:**

* Injecting PROXY v2 changes the byte stream and will break a lot of TCP servers if they are not configured for it.
* For raw TCP (databases, custom protocols), “true client IP” is often not required. Breaking compatibility is worse than losing it.
* For HTTP/HTTPS you will often have a front proxy (Caddy, Nginx stream, Envoy) that can be configured to accept PROXY. Those users can turn it on deliberately.

**Make it hard to misconfigure:**

* Route field: `proxy_protocol = "off" | "v2"`
* If `proxy_protocol="v2"`, require an explicit `backend_expects_proxy_protocol=true` (or equivalent) so users acknowledge it.
* Health checks should fail fast if the backend does not speak PROXY (example: the platform can do a tiny probe that verifies the server does not treat the PROXY header as payload).

---

### Open question 2: Adapter scope

Which common servers/protocols we ship a built-in adapter for, or whether we standardize “apps must support PROXY” for v1.

**Recommendation (v1): do not ship a generic “stripper adapter” inside the microVM as a platform feature. Provide docs and examples instead.**

**Why:**

* An adapter that strips PROXY v2 can only make the stream compatible. It cannot magically make the application aware of the real client IP unless the app has a way to read it (and most protocols do not).
* Shipping adapters expands your supported surface area forever. It becomes a hidden L7-ish component you own.

**What to do instead (concrete playbook):**

* For users who need client IP:

  * Recommend they run a standard front proxy inside the microVM (Caddy, Nginx, Envoy, HAProxy) that supports PROXY v2 and forwards to their app over localhost.
  * In that model, the proxy can translate client identity to whatever the app understands (HTTP headers for HTTP apps, or just logs at the proxy layer).
* For users who do not need client IP:

  * Keep `proxy_protocol=off`.

**One exception you can support later (not v1):**

* A dedicated “HTTP opt-in L7 mode” where you terminate HTTP and provide `Forwarded` headers. That is a separate product switch and does not belong in the core L4 path.

---

## ADR 0010: Open questions and recommendations (Secrets delivery file format)

### Open question 1: Exact file syntax

Env-style vs JSON vs TOML.

**Recommendation (v1): env-style key/value file with strict rules, plus a required base64 escape for non-UTF8 or multiline.**

**Concrete format contract:**

* File is UTF-8 text.
* Header line required: `# TRC_SECRETS_V1`
* Each secret is one line: `KEY=VALUE`
* `KEY` is `[A-Z0-9_]+` only.
* `VALUE` is raw text with no quoting rules except:

  * if value contains newline, null byte, or non-UTF8 bytes, it must be encoded as `base64:<...>`
* Whitespace is significant. No trimming.
* Comments allowed only as full-line starting with `#`.

**Why:**

* Every language can parse it with trivial code.
* Most secrets are single-line strings, so consumption is dead simple.
* Base64 escape gives you binary safety without forcing everything to be base64.

**Mount location recommendation:**

* `/run/secrets/platform.env` (or `/run/secrets/<bundle>.env` if you want multiple bundles later)

**Permissions recommendation:**

* Owned by root, mode `0400` by default.
* If manifest requests non-root user, allow group-readable mode `0440` with an app-specific group, but require explicit opt-in.

---

### Open question 2: Rotation semantics

Automatic refresh, on-deploy only, periodic reconciliation.

**Recommendation (v1): “rotate triggers rollout restart” as the default, with no hot-reload.**

**Concrete behavior:**

* A secret update produces a new secret bundle version event.
* Control plane marks affected `(env, process_type)` as needing restart.
* Scheduler performs a rolling restart (same as deploying a new release), but the OCI image digest does not change.
* Node agent writes the new secrets file at boot for each new instance.

**Why:**

* Hot-reloading secrets is a compatibility trap. Many apps read secrets once at startup.
* Restart semantics are easy to reason about, and they match your event-log model cleanly.

**Optional future extension (not v1):**

* `secrets.reload = "hot"` to atomically swap the file and optionally send a signal like `SIGHUP` if the user explicitly configures it.

---

### Open question 3: Provide standard libraries/helpers

**Recommendation (v1): ship two things, not language SDKs.**

1. CLI command: `platform secrets render --env prod` that emits the exact file format.
2. Tiny guest utility (optional) like `/usr/bin/platform-secrets-export` that prints `KEY=VALUE` pairs to stdout for shell use.

**Why:**

* Language SDKs become long-term maintenance obligations.
* CLI plus a stable file format is enough for almost everyone.

---

## ADR 0011: Open questions and recommendations (Local volumes, async backups)

### Open question 1: Backup backend choice and encryption model

**Recommendation (v1): S3-compatible object storage + client-side encryption per snapshot.**

**Concrete approach:**

* Backup target: any S3-compatible endpoint (could be MinIO you run, or a hosted provider later).
* Each snapshot is encrypted before upload:

  * generate a random data key per snapshot
  * encrypt snapshot with AES-256-GCM (or age if you want simplicity)
  * store the encrypted data key in control plane, wrapped by a platform master key (managed via SOPS/age or a later KMS)
* Store metadata: `volume_id`, `snapshot_id`, `created_at`, `size`, `checksum`, `encryption_key_id`.

**Why:**

* Object storage is the simplest durability primitive that does not force you to build distributed storage.
* Client-side encryption means the backup store does not need to be trusted.

---

### Open question 2: Snapshot mechanism on the host

Filesystem snapshots vs block-level snapshots.

**Recommendation (v1): LVM thin provisioning + thin snapshots for volumes (block-level).**

**Concrete host layout:**

* Put volume storage on an LVM thin pool.
* Each volume is an LV (logical volume).
* Attach LV to Firecracker as a block device.
* To snapshot:

  1. request guest to fsync and freeze (best effort)
  2. create an LVM thin snapshot
  3. stream snapshot device to backup tool
  4. delete snapshot after upload completes

**Why:**

* Works regardless of what filesystem is inside the guest.
* Snapshot creation is fast and does not require copying the full volume immediately.
* Keeps your “local volume” model intact while enabling backups and restores.

**What not to do in v1:**

* ZFS send/receive and incremental replication unless you are ready to standardize on ZFS across the fleet.
* Host filesystem CoW tricks that only work on btrfs or XFS reflink. Those become portability landmines.

---

### Open question 3: How to expose backup policies to users

Retention, frequency, opt-in.

**Recommendation (v1): cluster-level policy first, user-level policy later.**

**Concrete policy:**

* Every persistent volume is backed up on a default schedule (example: daily) unless the operator disables backups for that environment.
* Retention is fixed by the platform initially (example: keep last 14 backups).
* User-facing manifest does not yet control frequency/retention.

**Why:**

* Per-volume policy explodes scope and support burden early.
* You want one reliable backup pipeline before you offer knobs.

**Minimal user-facing surface that is acceptable in v1:**

* `backup_enabled: true|false` at the volume attachment level, default true.

---

## ADR 0012: Open questions and recommendations (CPU oversubscribe, memory hard cap)

### Open question 1: How to expose CPU requests

Cores vs millicores vs shares vs tiers.

**Recommendation (v1): user-facing “vCPU” as a decimal number, internally mapped to cgroup weights.**

**Concrete UX:**

* Manifest field: `cpu = 0.25 | 0.5 | 1 | 2 | 4 ...`
* Treat it as “relative share / requested capacity”, not a hard limit.

**Concrete runtime mapping:**

* Use cgroup v2 `cpu.weight` proportional to requested cpu.
* Do not enforce `cpu.max` by default in v1 (since CPU is soft), unless you later add an explicit `cpu_limit`.

**Scheduler implication (must be written down):**

* Define a cluster-level CPU overcommit ratio, for example `cpu_overcommit = 4.0`.
* Node allocatable cpu = `physical_cores * cpu_overcommit`.
* Placement uses requested cpu against allocatable, but it can exceed physical cores by design.

---

### Open question 2: Memory request vs limit, or single hard limit

**Recommendation (v1): single required hard cap only. No separate request.**

**Concrete UX:**

* Manifest field: `memory = "512Mi" | "2Gi"`, required.

**Runtime enforcement:**

* Enforce via cgroup v2 memory limit for the microVM process boundary.
* If exceeded: instance OOMs or is terminated, and the event should clearly state “memory limit exceeded”.

**Scheduler enforcement:**

* Sum of memory caps must not exceed `allocatable_memory`.
* `allocatable_memory = physical_memory - reserved_host_memory - safety_buffer`.

---

### Open question 3: Per-tenant fairness at scheduling time in addition to runtime quotas

**Recommendation (v1): implement explicit per-org quotas as the fairness mechanism, not complex scheduling heuristics.**

**Concrete quotas to implement first:**

* max total memory across all instances for org
* max instance count
* max public routes
* max IPv4 addresses (ties directly to paid add-on)

**Why:**

* Runtime CPU shares prevent one instance from hogging a node, but they do not stop one tenant from consuming the entire fleet.
* Quotas are simple, auditable, and match your control-plane event model.

**What not to do in v1:**

* Sophisticated fair-share scheduling algorithms. They are hard to validate and easy to game.
* Promises of CPU performance isolation equivalent to dedicated cores.

If you want, next I can take each of these and state them as “Resolved decisions” patches to the ADR text so the ADRs stop having open-ended tails.

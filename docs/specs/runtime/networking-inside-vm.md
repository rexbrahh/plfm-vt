# docs/specs/runtime/networking-inside-vm.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
Define the guest-side networking contract for workload microVMs:
- interface naming and expectations
- how the guest receives network configuration
- how the guest configures IPv6 (address, routes, DNS, MTU)
- what inbound and outbound connectivity must work in v1
- constraints and reserved behavior

This spec is normative for the guest init (PID 1) and the host agent that supplies configuration.

Locked decisions this depends on:
- IPv6-first: `docs/adr/0007-network-ipv6-first-ipv4-paid.md`
- Overlay is WireGuard: `docs/adr/0004-overlay-wireguard-full-mesh.md`
- L4 ingress, SNI passthrough: `docs/adr/0008-ingress-l4-sni-passthrough-first.md`

## Scope
This spec defines networking inside the microVM.

This spec does not define:
- host-side overlay wiring, tap setup, routing, nftables (see networking specs)
- IP allocation strategy (see `docs/specs/networking/ipam.md`)
- ingress routing rules (see `docs/specs/networking/ingress-l4.md`)
- PROXY protocol behavior (see `docs/specs/networking/proxy-protocol-v2.md`)

## Definitions
- **overlay IPv6**: the IPv6 address assigned to the instance for east-west and edge-to-backend routing.
- **gateway IPv6**: the next-hop address on the instance interface used as the default route.
- **eth0**: the primary virtio-net interface inside the guest.

## High-level contract (v1)
1) Every microVM has exactly one primary network interface: `eth0`.
2) Network configuration is static and is provided by the platform at boot time.
3) The guest must configure IPv6 on `eth0` using the provided config:
   - set MTU
   - set the IPv6 address (/128)
   - set default route via gateway
   - set DNS resolvers (if provided)
4) No DHCP is used in v1 (neither DHCPv4 nor DHCPv6).
5) The workload listens on TCP ports declared in WorkloadSpec. No guest firewall configuration is required in v1.

## Network configuration delivery
The guest init obtains network config during the vsock config handshake described in:
- `docs/specs/runtime/firecracker-boot.md`

Required network fields (v1):
- `overlay_ipv6` (string, required, /128 address)
- `gateway_ipv6` (string, required)
- `mtu` (int, optional, default 1420)
- `dns` (array of IPv6 addresses, optional, default empty)

Validation rules:
- `overlay_ipv6` must be a valid IPv6 address.
- `gateway_ipv6` must be a valid IPv6 address.
- `mtu` must be between 1280 and 9000 (IPv6 minimum MTU is 1280).
- `dns` entries must be valid IPv6 addresses.

## Addressing model (v1)
### Instance address
- The instance is assigned exactly one IPv6 address on `eth0`.
- It is configured as `/128`.

Rationale:
- Keeps routing explicit and avoids needing a full /64 in the guest.
- Works well with overlay routing and explicit per-instance identity.

### Gateway address
- The default gateway is an IPv6 address reachable on `eth0`.
- In v1 this is expected to be a link-local gateway, typically `fe80::1`, or a host-provided gateway in a routed prefix.

Important requirement:
- If `gateway_ipv6` is link-local (starts with `fe80::`), the route must be bound to `dev eth0`.

## Guest init networking steps (normative)
Guest init must perform these steps in order.

Assume:
- interface is `eth0`
- configuration is `{ overlay_ipv6, gateway_ipv6, mtu, dns[] }`

### Step 1: bring up interface and set MTU
Using iproute2 semantics (implementation may use netlink directly):
- `ip link set dev eth0 mtu <mtu>`
- `ip link set dev eth0 up`

### Step 2: assign IPv6 address
- `ip -6 addr add <overlay_ipv6>/128 dev eth0`

Rules:
- If address already exists, treat as success (idempotent).
- Do not rely on SLAAC or router advertisements.

### Step 3: add default route
- If `gateway_ipv6` is link-local:
  - `ip -6 route replace default via <gateway_ipv6> dev eth0`
- Else:
  - `ip -6 route replace default via <gateway_ipv6>`

Rule:
- Route operation must be idempotent.

### Step 4: write DNS configuration
If `dns` list is non-empty, guest init must write `/etc/resolv.conf` with IPv6 resolvers:

Example:
- `nameserver 2606:4700:4700::1111`

v1 rule:
- Do not attempt to run a DNS daemon in the guest.
- Use `/etc/resolv.conf` only.

If `dns` list is empty:
- Guest init may leave `/etc/resolv.conf` as-is, or set it to a platform default only if a platform default is explicitly configured. Do not bake public resolver addresses into the guest init binary.

### Step 5: basic sanity checks
Guest init should perform minimal checks and fail fast if networking is clearly broken:
- verify `eth0` exists and is up
- verify `overlay_ipv6` is present on eth0
- verify a default IPv6 route exists

If these checks fail, guest init must exit non-zero and log a clear error to the serial console.

## Inbound connectivity semantics
Inbound traffic from edge and from other workloads must be able to reach:
- `overlay_ipv6:<declared TCP port>`

The guest:
- must bind services to `::` or to its assigned IPv6 address
- must not assume IPv4 is present inside the guest in v1

Port binding rules:
- Only TCP is supported in v1 for ingress and service exposure.
- If a workload binds only IPv4 (`0.0.0.0`), it will not be reachable. This must be surfaced clearly in docs and tooling.

## Outbound connectivity semantics (egress)
The guest sends outbound traffic via its default IPv6 route.

The platform may implement egress in multiple ways:
- direct routing with globally routable IPv6 addresses
- NAT66 or other egress translation at the host or edge

From the guest perspective, v1 requirement is:
- outbound IPv6 connections to the public internet should work unless the platform operator explicitly restricts egress.

The guest should not attempt to manage NAT or firewall rules.

## ICMPv6 requirements
IPv6 depends on ICMPv6 for correct operation (including Path MTU Discovery).

Requirements:
- The guest must not disable ICMPv6.
- The platform must not block ICMPv6 Packet Too Big messages on paths relevant to the guest.

Guest init must not install firewall rules that drop ICMPv6.

## Prohibited behaviors (v1)
- Running DHCP clients (v4 or v6) by default.
- Accepting router advertisements and dynamically changing addressing by RA.
- Exposing a guest metadata HTTP service.
- Assuming IPv4 exists inside the guest.

## Observability requirements
Guest init must log the following to serial console at boot:
- interface up and MTU
- configured IPv6 address
- configured default route (gateway)
- configured DNS servers count (not the values if you want to reduce leakage)

Host agent should also report:
- the assigned overlay_ipv6 per instance
- readiness transitions based on health checks
- network setup failures with reason codes:
  - `network_setup_failed`

## Compliance tests (required)
Automated tests should verify:
1) Guest init sets MTU and brings up eth0.
2) Guest init assigns overlay IPv6 /128.
3) Guest init installs a default route.
4) Guest can accept inbound TCP connections to the declared port.
5) Guest can perform outbound IPv6 TCP connection (to a test endpoint) when egress is enabled.
6) Guest does not run DHCP and does not rely on RA.

## Open questions (deferred)
- Whether v1 supports an optional internal IPv4 stack inside the guest for compatibility. Default stance is no.
- Whether to support multiple interfaces per microVM (not in v1).

# IPv6 and DNS configuration plan

Status: the shared Rust DNS model, Merlin settings/diagnostics, desktop DNS controls, and explicit Merlin dual-stack routing are implemented. Compatibility remains the default; dual-stack is opt-in. This document also retains the wider roadmap below. Automatic dual-stack selection, DoQ/HTTP3 DNS, and full desktop controls for every advanced field remain follow-up work.

## Local candidate verification

- 562 core/CLI and integration tests passed; 15 live/manual tests remain ignored by default. New checks cover DNS configuration migration, private error messages, node bootstrap separation, wire-cache generation changes and byte limits, real IPv6 HTTP/SOCKS connections, and IPv6 Fake-IP identity.
- Six Web UI suites passed, including the new structured DNS form's required fields, stopped saves, stale results, apply locking and failure recovery.
- 26 Merlin stability scenarios, dedicated IPv6 staged-routing/rollback checks, Software Center installation checks and RPC transport checks passed using isolated fixtures.
- Production core/CLI Clippy, desktop compilation, formatting and ARM64 musl release compilation passed. Existing dependency/toolchain warnings remain.
- A complete local HND v8 candidate package was assembled and compared with the built binary and current source files. Archive ownership is generic; local home paths, private runtime configuration and tests are excluded.
- The default remains compatibility mode. Local command mocks do not prove kernel-module support or LAN reachability; a WAN change, reboot, or multi-hour soak is still separate from this code review.

## Source findings

| Area | Existing capability | Gap |
| --- | --- | --- |
| IPv6 | Address parsing, AAAA resolution and best-effort desktop TUN routing exist | Merlin transparent routing is IPv4-only. Its firewall rejects external forwarded LAN IPv6 while proxying, and its LAN resolver suppresses AAAA/HTTPS records for IPv4 fallback. This is protection, not dual-stack proxy support. |
| DNS upstreams | The resolver supports direct UDP, DoH, proxy TCP and proxy TLS | `AppConfig` has no shared DNS settings. Router-only `dns_direct_server` accepts one IP at port 53; `dns_direct_domains` supplies suffix overrides. Other paths construct defaults separately. |
| DNS runtime | Shared outbound changes can update resolver behavior | Every resolution path needs an audit: HTTP/SOCKS routing, TUN-intercepted queries, the LAN listener, node bootstrap, health checks and rule tracing must apply the intended configuration consistently. |
| UI | DNS interception can be toggled; raw YAML is editable | Users need structured upstream settings, validation, effective-state reporting and a DNS query diagnostic. |

Relevant source: `crates/sockrocket-core/src/config/model.rs`, `crates/sockrocket-core/src/dns/`, `crates/sockrocket-core/src/proxy/tun.rs`, `crates/sockrocket-cli/src/main.rs`, `merlin/scripts/iptables.sh`, and `merlin/webui/sockrocket.asp`.

## Phase 1: shared DNS configuration and Merlin settings

- Introduce a versioned, typed DNS configuration with stable upstream IDs. Keep transport, endpoint, TLS server name, bootstrap IPs and connection route as separate fields. Distinguish direct resolvers, proxy resolvers and node/bootstrap resolution; do not treat the direct group as an automatic failure fallback for proxy-only names.
- Expose upstream lists with automatic defaults and custom values. First support the existing UDP, DoH, proxy TCP and proxy TLS capabilities; add direct TCP/TLS and proxied DoH explicitly where needed. DoQ and HTTP/3 DNS are later additions rather than aliases for existing transports.
- Provide a simple settings view: direct DNS, proxy DNS, node bootstrap DNS, address strategy and Test/Save actions. Put cache capacity, query timeout, TTL limits and domain overrides in an advanced section. Explain required versus optional fields and preserve unknown configuration fields.
- Resolve the DNS server's hostname without recursively depending on that same DNS server or a proxy node whose hostname requires it. Keep certificate verification enabled for encrypted DNS. Reject resolver loops through the local listener/dnsmasq path.
- Migrate legacy `dns_direct_server` and `dns_direct_domains` without losing settings. Explicit new configuration wins; invalid input reports the field and reason without silently reverting to defaults. Normalize endpoint forms, including bracketed IPv6 addresses and ports.
- Publish an immutable validated configuration generation to every resolver consumer. Keep caches and pooled connections isolated by the effective generation and outbound; preserve required Fake-IP mappings for active connections during a change. Reject or atomically roll back failed changes and report saved versus applied states accurately.
- Use bounded DNS concurrency, connection pools, cache sizes, retries and deadlines. Never silently send failed proxy DNS queries to the direct group. New queries use the latest policy; policy changes invalidate inappropriate positive, negative and stale cached answers.

DNS transport configuration and DNS interception are separate controls. Saving DNS settings must not start a stopped proxy or change the existing explicit Start/Stop behavior.

## Phase 2: complete dual-stack routing

- Keep three policies explicit: IPv4 compatibility, automatic dual-stack after capability checks, and strict dual-stack requiring successful setup. Preserve the current compatibility policy for existing Merlin installations until dual-stack is accepted.
- Separate DNS answer strategy from forwarding capability. Do not return usable AAAA answers to LAN clients until IPv6 forwarding, rule evaluation and cleanup are ready. Explain effective capability and any fallback in status.
- Implement Merlin IPv6 policy routing and owned firewall chains, with node/bootstrap bypasses and private/link-local/multicast handling. Preserve router administration, ICMPv6, neighbor discovery, router advertisements, DHCPv6 and path-MTU discovery. Do not disable the router's IPv6 service globally.
- Ensure the real-address IPv6 path reaches the same source-device, domain/IP, priority and final-rule decisions as IPv4. The current built-in China database is IPv4; dual-stack must define and validate IPv6 domestic classification rather than assuming `geoip:CN` covers it.
- Verify TCP and UDP reply paths for direct and proxied IPv6. Establish an explicit strategy for Fake-IP and DNS-to-domain mapping on IPv6; do not synthesize IPv6 Fake-IP addresses without a complete allocation, reverse mapping and return path.
- Add bounded IPv4/IPv6 connection racing where applicable. A broken IPv6 route must not force every connection to wait for a long timeout before trying IPv4. IPv6-only upstreams need a clear failure on IPv4-only networks.
- Journal only application-owned routes, firewall jumps and resolver changes. Stop, failed startup, crash recovery, upgrade and reboot recovery must remove those changes while preserving user and firmware settings. Handle WAN prefix/default-route changes without retaining stale bypasses.

## Phase 3: diagnosis and acceptance

- Show configured policy and effective runtime state separately: IPv4/IPv6 availability, active DNS group and transport, response type, elapsed time, cache hit, configuration generation and degraded reason.
- Extend the routing explanation with the resolver selection and resulting A/AAAA decision. A configuration trace remains distinct from an actual LAN forwarding test.
- Add an on-demand DNS test with actionable timeout, TLS, bootstrap, invalid-response and no-outbound errors. Do not persist query history by default. Shareable diagnostics redact credentials, private resolver paths/query tokens, subscription links, device addresses and node endpoints.
- Validate DNS configuration round trips, legacy migration, invalid endpoints, bootstrap loops, same-group failover, cross-group isolation, truncated UDP fallback, cancellation, generation changes and cache isolation with synthetic fixtures.
- Validate IPv4-only, native dual-stack, broken IPv6, IPv6-only destinations and WAN prefix changes. Test TCP/UDP and DNS from real LAN devices as well as the router; router-origin probes alone are insufficient.
- Test normal Stop, failed Start, service restart, abrupt process termination, upgrade and router reboot with active traffic. Verify the system's prior resolver and forwarding behavior after cleanup.
- Run a multi-hour mixed domestic/international workload including video, DNS failures and node changes; compare latency percentiles, DNS failure rates, connection errors, memory and descriptor growth against the IPv4 baseline.

Deliver and accept DNS configuration first, then dual-stack routing, then desktop UI parity. Publish and deploy through complete Software Center packages only after the corresponding acceptance gates; a new switch or a successful compilation does not establish support.

## References

- [Current project capability boundaries](network-capabilities.md).
- [sing-box DNS](https://sing-box.sagernet.org/configuration/dns/): separate servers, rules, address strategy and cache controls.
- [Xray DNS](https://xtls.github.io/en/config/dns.html): explicit upstream selection, query strategies and fallback policy. These are design references; their configuration schemas are not automatically supported by this project.

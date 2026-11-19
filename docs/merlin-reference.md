# Merlin technical reference

Reference for AsusWRT-Merlin deployments: fake-IP DNS, TUN, iptables, and deploy. Use placeholders in `.local/router.env` (`ROUTER_HOST`, `ROUTER_PORT`, `ROUTER_USER`, `ROUTER_PASS`) — never commit real credentials.

---

## 1. Goals and constraints

**Goal:** LAN clients need no per-app proxy settings; domestic sites stay direct (optionally hardware-NAT accelerated); international sites use the selected node.

**DNS-unlock providers:** Some exits resolve domains on the server. Client-side DNS must not replace the hostname with a real IP before dialing. Hence:

- SOCKS/HTTP works (hostname passed to proxy).
- Plain TUN with client DNS → wrong dial targets.
- **fake-IP:** DNS returns synthetic addresses; TUN maps them back to domain names for outbound dial.

### 1.1 Request path (HTTPS example)

```
Browser → DNS www.example.com
  → dnsmasq :53 (LAN forced to local dnsmasq)
  → Sockrocket DNS 127.0.0.1:5300
  → Split DNS: not in domestic suffix list → primary group → fake-IP 198.18.0.x (TTL ~5s)

Browser → TCP 198.18.0.x:443
  → PREROUTING / SOCKROCKET_MANGLE (private ranges RETURN, optional CN ipset bypass)
  → MARK 0x4765 → policy table 5370 → dev sockrocket-tun → ipstack
  → Fake IP → domain lookup → router rules → proxy outbound (dial by domain)

Domestic example (e.g. example.cn / curated China suffixes):
  → Fallback resolvers → real IP, cached in domestic direct table
  → Eligible IPs can bypass TUN via the CN ipset. Inside TUN, the router evaluates the configured rules.

DNS Fallback suffixes and routing China-direct suffixes share one list
(`china_domain_suffix_list`) as a fast path. For everything else, Fake-IP
mode runs **resolve-then-geoip**: clean primary upstream (DoT via node) →
if any address in an A or dual-stack AAAA answer matches `geoip:CN`,
return the real answer and attach a domestic hint to each matching address;
otherwise allocate a fake-IP so TUN still dials by name for unlock exits.
```

### 1.2 Processes on router

| Process | Role |
|---------|------|
| `sockrocket-cli config.yaml --tun` | TUN, DNS :5300, SOCKS :1080, HTTP :1087 |
| `sockrocket-cli api 18188` | JSON API for Web UI (separate process) |
| dnsmasq | LAN DNS; upstream `127.0.0.1#5300` |

Watchdog cron restarts the daemon; `sockrocket.sh start` rebuilds iptables/ipset after crashes.

---

## 2. DNS (`sockrocket-core/src/dns/`)

### Groups

- **Primary** handles other names. With Fake-IP and CN lookup enabled, it classifies the upstream answer before choosing real or synthetic addresses.
- **Fallback:** domestic suffix list + **server_domains** (node hostnames always fallback).

Priority: server domain match → longest suffix → primary.

### fake-IP (`dns/fakeip.rs`)

- The pool holds up to 8,192 names. IPv4 tokens come from **198.18.0.0/16**. Dual-stack IPv6 tokens encode the same allocation. Synthetic answers have a five-second TTL.
- Merlin dual-stack mode supports both A and AAAA. In IPv4 compatibility mode, the client DNS listener suppresses AAAA answers.
- Evicted mapping → drop flow; short TTL refreshes.

### Domestic direct table

A bounded cache of 16,384 addresses associates explicit fallback domains and individual CN addresses from primary answers with their recent names. Entries follow the answer TTL, capped at 30 seconds. If two live names share an address, TUN uses the destination IP for routing until the ambiguity expires.

### Proxy DNS pool

A four-slot TCP/TLS pool reuses upstream connections for DNS queries.

### Anti-poisoning

DoT via proxy for primary; transaction ID check; node domains via bootstrap UDP resolvers.

---

## 3. TUN and iptables

- Device `sockrocket-tun`, IPv4 `10.10.0.2`, IPv6 ULA `fd9a:4c2e:17b0::2/64`.
- ipstack TCP idle **3600s** (long-lived push/chat).
- **SOCKROCKET_MANGLE:** private/LAN RETURN; optional CN ipset bypass for eligible connections; then MARK → table **5370** (not 100 — Merlin aliases 100 to WAN).
- **rp_filter=2** on tun and all; **FORWARD** accept from tun.
- **Dead route trap:** after manual kill, `ip route replace default dev sockrocket-tun table 5370`.
- UDP, including QUIC, follows the selected outbound. Its node must support the destination and UDP transport. Individual oversized responses are discarded within the existing association.

### TUN routing order

1. Restore Fake-IP tokens to their domain. Keep the real destination available for IP and port rules on real-address flows.
2. Use a live, unambiguous DNS attribution when available. Anonymous TCP/443 can use an applicable SNI rule.
3. Evaluate the device and global policy in its configured order. The built-in domestic hint participates alongside the other built-in rules.

---

## 4. Rules (`router/`)

Types: `domain`, `domain-suffix`, `domain-keyword`, `ip-cidr`, `geoip`, `final`. Actions: `direct`, `proxy`, `reject`. User non-final rules first, then built-in China-direct (`geoip:CN`), then `final`. Rule changes on router may require daemon restart (API may auto-restart). Reject shows as timeout to clients.

---

## 5. Health check

`health_check:` — `enabled`, `interval_secs` (≥5), `failure_threshold`, `auto_switch`. Probes `www.gstatic.com/generate_204` through current node; on failure switches to best latency node and persists `active_node` in `config.yaml`. State: `/tmp/sockrocket_health.json`.

On start the monitor runs two probes about 3s apart, then settles into `interval_secs`. Merlin template uses `failure_threshold: 2`. Each failed check is retried after 2s; only two consecutive failed rounds trigger a replacement search. The active global node is checked again after the search and retained if it recovered. Healthy nodes are never replaced merely for lower latency; there is no automatic switch back.

### TUN / DNS toggles

`transparent_proxy` and `dns_hijack` are independent. Fake-IP exists only while the daemon is started with `--tun`.

| State | TUN | DNS hijack | Answers |
|-------|-----|------------|---------|
| Proxy on | on | forced on | Fake-IP, TTL 5s |
| Proxy off | off | stays on | Real IPs, short TTL, no Fake-IP |
| DNS off | either | off | ISP DNS (long TTL) |

Turning transparent proxy off does not clear `dns_hijack` and does not pull subscriptions. `proxy-off` drops iptables, restarts without `--tun` when needed, then reinstalls the dnsmasq hijack if `dns_hijack` is still true. Subscription fetch runs only for `update-subs` / the daily cron (`SOCKROCKET_REFRESH_SUBS=1`).

Router check (restores TUN+DNS on exit): `scripts/verify_tun_toggle.sh`.

---

## 6. CN ipset direct (optional)

`proxy_mode` (`rule` | `global` | `direct`, default `rule` when omitted): same routing modes as the Mac client. Web UI shows a Rule / Global / Direct control. `set_proxy_mode` restarts the daemon; Global also clears `cn_ipset_direct`.

`cn_ipset_direct: true` in the fresh-install template (existing explicit settings are preserved). When enabled, domestic IPv4 destinations in `sockrocket_cn` bypass TUN. With dual-stack capture active, domestic IPv6 destinations in `sockrocket_cn6` also bypass TUN. Both sets use the built-in GeoIP CN prefixes. Devices with matching source rules still enter TUN. **Side effect:** per-domain rules for CN-resolved IPs do not apply on that path. Use the “国内 IP 绕过 TUN” switch in the Web UI network settings. With transparent proxy off, the preference takes effect when TUN starts.

---

## 7. Files and ports

```
/jffs/addons/sockrocket/
├── sockrocket-cli
├── config.yaml
├── scripts/ sockrocket.sh, iptables.sh, sockrocket_api.sh, dnsmasq.conf.template
├── webui/sockrocket.asp
└── logs, pid files
/jffs/configs/dnsmasq.d/sockrocket.conf
```

Ports: DNS **5300** (localhost), SOCKS **1080**, HTTP **1087**, API **18188**.

Router-only keys (read from YAML in CLI): `dns_port`, `transparent_proxy`, `dns_hijack`, `cn_ipset_direct`, legacy `mode: tun`. Shared with desktop: `proxy_mode`.

---

## 8. Deploy

Prefer a package from [GitHub Releases](https://github.com/sockrockets/sockrocket/releases)
(`sockrocket-merlin-<platform>.tar.gz`). To replace only the binary on an
already-installed router:

For a locally built ARM64 binary, first follow the
[local build and allocator verification guide](./merlin-local-build.md).
`bash tools/build-merlin-musl.sh` produces the binary and matching symbols;
packaging or uploading an existing binary does not validate its allocator.

Preferred path (uploads binary + WebUI, including Softcenter
`Module_sockrocket.asp`, then restarts safely):

```bash
# shellcheck source=/dev/null
. .local/router.env
./tools/merlin_deploy.sh
# FORCE_RESTART=1 ./tools/merlin_deploy.sh   # when binary md5 matches but daemon must reload
```

Manual binary-only swap:

```bash
# shellcheck source=/dev/null
. .local/router.env
scp -O -P "$ROUTER_PORT" sockrocket-cli-merlin-aarch64 \
  "${ROUTER_USER}@${ROUTER_HOST}:/jffs/addons/sockrocket/sockrocket-cli.new"
ssh -p "$ROUTER_PORT" "${ROUTER_USER}@${ROUTER_HOST}" '
  cd /jffs/addons/sockrocket
  sh scripts/sockrocket.sh stop
  mv sockrocket-cli.new sockrocket-cli && chmod +x sockrocket-cli
  sh scripts/sockrocket.sh start
  sh scripts/sockrocket.sh api-restart
'
```

SSH password helper: `tools/ssh_askpass.sh` with `.local.example/` (askpass +
`PreferredAuthentications=password`). Convert scripts to LF before scp
(`tr -d '\r'`). Free JFFS space — remove stale `.new` files. After deploy,
confirm `sockrocket.sh status` shows running.

Debug: `RUST_LOG=debug` to `/tmp/sockrocket-debug.log`; restore with `sockrocket.sh start`. Verify: `sh scripts/iptables.sh verify`.

### Crash dumps

Watchdog may collect dmesg tail, log tail, optional core under `/tmp/` (copy before reboot).

### Busybox quirks

No `command -v`, `sysctl`, or `timeout`; use `/proc`, awk; explicit `PATH`; ash 32-bit shift limits in iptables helpers; `nvram get` empty vs unset.

---

## 9. Known limits

1. TUN UDP sessions require a UDP-capable outbound. Response payloads must fit the link MTU.
2. Rare daemon segfaults — use crash history + core.  
3. Reject = timeout UX.  
4. Node latency tests include cold QUIC handshake (conservative numbers).  
5. CN lookup uses the bundled prefix list. An explicit direct-domain policy can also cover recent DNS answers outside that list. IP-only connections depend on the prefix data and configured routing rules.

---

## 10. Roadmap (cost / benefit)

| Item | Notes |
|------|-------|
| Auto-update CN ipset | Medium |
| Reject as RST | UX polish |
| Router throughput measurements | Compare IPv4 and IPv6 TCP/UDP on supported hardware |

Performance baselines in lab vary by node; treat latency tables as order-of-magnitude only.

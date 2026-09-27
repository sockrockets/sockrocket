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
  → PREROUTING / SOCKROCKET_MANGLE (private ranges RETURN, optional UDP/443 RETURN, optional cn ipset RETURN)
  → MARK 0x4765 → policy table 5370 → dev sockrocket-tun → ipstack
  → Fake IP → domain lookup → router rules → proxy outbound (dial by domain)

Domestic example (e.g. example.cn in suffix list):
  → Fallback resolvers → real IP, cached in domestic direct table
  → Traffic may RETURN before TUN (cn ipset) or hit direct table inside TUN
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

- **Primary:** default; fake-IP for A records in TUN mode; desktop may use DoT via proxy.
- **Fallback:** domestic suffix list + **server_domains** (node hostnames always fallback).

Priority: server domain match → longest suffix → primary.

### fake-IP (`dns/fakeip.rs`)

- Pool **198.18.0.0/16**, 8192 LRU entries, ~5s TTL (dnsmasq `min-cache-ttl=5`).
- Primary-group **A** only; AAAA → NODATA (push clients to A + domain dial).
- Evicted mapping → drop flow; short TTL refreshes.

### Domestic direct table

Stores real IPs from fallback answers (LRU ~16384) so geoip gaps still direct correctly.

### Proxy DNS pool

Four-slot TCP/TLS pool; mostly idle on router fake-IP (international A answered locally).

### Anti-poisoning

DoT via proxy for primary; transaction ID check; node domains via bootstrap UDP resolvers.

---

## 3. TUN and iptables

- Device `sockrocket-tun`, IPv4 `10.10.0.2`, IPv6 ULA `fd9a:4c2e:17b0::2/64`.
- ipstack TCP idle **3600s** (long-lived push/chat).
- **SOCKROCKET_MANGLE:** private/LAN RETURN; UDP/443 RETURN (QUIC bypass); optional `sockrocket_cn` ipset RETURN; then MARK → table **5370** (not 100 — Merlin aliases 100 to WAN).
- **rp_filter=2** on tun and all; **FORWARD** accept from tun.
- **Dead route trap:** after manual kill, `ip route replace default dev sockrocket-tun table 5370`.
- UDP over TUN is limited (QUIC exempt at iptables); full UDP relay is future work.

### TUN routing order

1. Fake IP → domain → domain-only rules → dial.  
2. Real IP with known domain → user rules then direct.  
3. In domestic table → direct.  
4. Full rules + geoip → default proxy.

---

## 4. Rules (`router/`)

Types: `domain`, `domain-suffix`, `domain-keyword`, `ip-cidr`, `geoip`, `final`. Actions: `direct`, `proxy`, `reject`. User rules prepend built-in China-direct set. Rule changes on router may require daemon restart (API may auto-restart). Reject shows as timeout to clients.

---

## 5. Health check

`health_check:` — `enabled`, `interval_secs` (≥5), `failure_threshold`, `auto_switch`. Probes `www.gstatic.com/generate_204` through current node; on failure switches to best latency node and persists `active_node` in `config.yaml`. State: `/tmp/sockrocket_health.json`.

---

## 6. CN ipset direct (optional)

`cn_ipset_direct: false` by default. When on, `sockrocket_cn` hash:net (from `--dump-cn-cidrs`) lets domestic IPs skip TUN for hardware NAT. **Side effect:** per-domain rules for CN-resolved IPs do not apply on that path. Toggle via API / `iptables.sh ipset-on|off`.

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

Router-only keys (read from YAML in CLI): `dns_port`, `transparent_proxy`, `dns_hijack`, `cn_ipset_direct`, legacy `mode: tun`.

---

## 8. Deploy

Prefer a package from [GitHub Releases](https://github.com/sockrockets/sockrocket/releases)
(`sockrocket-merlin-<platform>.tar.gz`). To replace only the binary on an
already-installed router:

```bash
# shellcheck source=/dev/null
. .local/router.env
scp -O -P "$ROUTER_PORT" sockrocket-cli-linux-aarch64 \
  "${ROUTER_USER}@${ROUTER_HOST}:/jffs/addons/sockrocket/sockrocket-cli.new"
ssh -p "$ROUTER_PORT" "${ROUTER_USER}@${ROUTER_HOST}" '
  cd /jffs/addons/sockrocket
  sh scripts/sockrocket.sh stop
  mv sockrocket-cli.new sockrocket-cli && chmod +x sockrocket-cli
  sh scripts/sockrocket.sh start
  sh scripts/sockrocket.sh api-restart
'
```

SSH password helper: `tools/ssh_askpass.sh` with `.local.example/`. Convert scripts to LF before scp (`tr -d '\r'`). Free JFFS space — remove stale `.new` files.

Debug: `RUST_LOG=debug` to `/tmp/sockrocket-debug.log`; restore with `sockrocket.sh start`. Verify: `sh scripts/iptables.sh verify`.

### Crash dumps

Watchdog may collect dmesg tail, log tail, optional core under `/tmp/` (copy before reboot).

### Busybox quirks

No `command -v`, `sysctl`, or `timeout`; use `/proc`, awk; explicit `PATH`; ash 32-bit shift limits in iptables helpers; `nvram get` empty vs unset.

---

## 9. Known limits

1. UDP semantics through TUN (QUIC bypass at firewall).  
2. Rare daemon segfaults — use crash history + core.  
3. Reject = timeout UX.  
4. Node latency tests include cold QUIC handshake (conservative numbers).  
5. Static CN CIDR list; gaps covered by TUN direct table.

---

## 10. Roadmap (cost / benefit)

| Item | Notes |
|------|-------|
| Hysteria2 UDP + TUN UDP sessions | High effort; games/WebRTC |
| Router hot-reload (ArcSwap) | Low effort when touching rules |
| Auto-update CN ipset | Medium |
| Reject as RST | UX polish |
| TUN regression tests | Medium term |

Performance baselines in lab vary by node; treat latency tables as order-of-magnitude only.

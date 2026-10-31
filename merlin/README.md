# Sockrocket — AsusWRT-Merlin plugin

Transparent proxy integration for **AsusWRT-Merlin** routers.

## Features

- **Transparent proxy** — iptables/policy routing for LAN TCP (no client config)
- **DNS** — dnsmasq forwards to Sockrocket DNS listener (leak reduction)
- **Subscriptions** — manage URLs from Web UI
- **Node selection** — switch nodes and run latency tests
- **Routing rules** — search and filter, edit individual rules and numeric priorities, paste mixed rule types, and manage groups. The list follows effective priority, with final rules after built-in China-direct routing. Invalid batch input is rejected without partial saves; stale edit snapshots cannot overwrite newer changes. Applying changes restarts a running proxy.
- **Routing explanation** — query a site/IP, optional source-device IP and destination port using the core matcher. See the first matching rule, evaluation steps and proxy/direct/reject outcome; open the exact matching rule or filter all rules in a diagram stage. The diagram sends traffic into the first stage; later stages are reached when earlier stages miss. Optional DNS or a supplied real destination IP verifies IP/GeoIP conditions. Unresolved conditions remain provisional; stale input/configuration results cannot edit rules. Queries are not persisted. This explains saved policy for a new connection, not live traffic or endpoint reachability; TUN hints, connection caches and kernel bypass can affect actual traffic.
- **Node groups** — manual uses one member. Fallback tries members in order, remembers a success for five minutes, then starts at the first member again; UDP stays on the first member. URL test races handshakes and reuses the winner for five minutes. A group carries traffic only in rule mode, after a rule exit names it. Switching a member does not change the global node.
- **Rule exits** — a rule folder can use the global node, one saved node, or a node group. Direct and reject rules in a mixed folder stay unchanged. A missing pinned node fails closed.
- **Traffic** — Overview counts bytes since this process started. Subscriptions show the last three calendar months per node, including manual nodes and Direct. A subscription total is the sum of nodes that currently belong to it. Subscription addresses are not shown.
- **Optional rule presets** — seven bundled groups: AI services, overseas video, Telegram web, developer tools, Apple direct, Microsoft direct and domestic sites. Preview domains, add disabled rules, then explicitly enable a group. Imports skip existing global domain-suffix selectors without changing their state, action or priority; new rules start at priority −10. Importing disabled rules does not restart the proxy. Preset contents and per-rule/group changes persist in the normal configuration; opening the catalog never writes configuration. Existing upgrades do not inject or enable presets automatically.
- **Modes** — TUN, SOCKS5, or system-style proxy (see `config.yaml`)
- **Watchdog** — restarts crashed daemon; optional subscription cron
- **IPv6 policy** — compatibility is the default and does not require ISP IPv6. It keeps the IPv4 proxy, answers LAN DNS with IPv4 only, and rejects forwarded external IPv6 only when firmware IPv6 is already on. Dual-stack is optional after the firmware already has an IPv6 address and a LAN prefix. If dual-stack cannot finish, the IPv4 proxy and DNS hijack stay in place. Saving Network & DNS reapplies the firewall without restarting a running proxy. Firmware IPv6 enablement is not changed.
- **Web UI** — `/ext/sockrocket/sockrocket.asp`

Preset coverage is a curated starting point, not a complete service endpoint list. Shared authentication/CDNs may need additional rules; Telegram client data-center IPs still follow IP rules or the default route. Public endpoint references: [ChatGPT](https://help.openai.com/en/articles/9247338-network-recommendations-for-chatgpt-errors-on-web-and-apps), [Apple](https://support.apple.com/en-us/101555), [Microsoft 365](https://learn.microsoft.com/en-us/microsoft-365/enterprise/urls-and-ip-address-ranges?view=o365-worldwide), [GitHub](https://docs.github.com/en/get-started/using-github/allowing-access-to-githubs-services-from-a-restricted-network), [Telegram](https://core.telegram.org/api/datacenter).

## Platforms (koolcenter)

Packages are named `sockrocket-merlin-<platform>.tar.gz` with a matching `.valid` tag:

| Platform | CPU binary | SoC family |
|----------|------------|------------|
| `arm` | armv7 softfloat | BCM4708/4709 |
| `hnd` | armv7 hardfloat | BCM675x |
| `hnd_v8` | aarch64 | BCM490x / BCM491x |
| `qca` | armv7 hardfloat | IPQ807x |
| `mtk` | aarch64 | MT798x |
| `ipq32` | armv7 hardfloat | IPQ53xx 32-bit userspace |
| `ipq64` | aarch64 | IPQ53xx 64-bit userspace |

Unlike fancyss’s single `fancyss_hnd` tree, Sockrocket uses **`hnd_v8` for BCM490x** and **`hnd` for BCM675x**.

| Platform | Example models |
|----------|----------------|
| `hnd_v8` | RT-AC86U, RT-AX86U / PRO, RT-AX88U / PRO, GT-AX11000 / PRO, GT-AX6000, RT-AX68U, GT-AXE11000 / 16000, ZenWiFi Pro XT12 |
| `hnd` | TUF-AX3000 / 5400, RT-AX58U, RT-AX82U, ZenWiFi XT8 / XD4, RT-AX56U, RT-AX55 / 57 |
| `arm` | RT-AC68U, RT-AC88U, RT-AC3100, RT-AC5300, RT-AC1900P |
| `qca` | RT-AX89X |
| `mtk` | TUF-AX4200 / 6000, RT-AX59U, ZenWiFi BD4 |

Full model tables: [docs/merlin.md](../docs/merlin.md).

The [Merlin compatibility notes](../docs/architecture/merlin-compatibility.md) distinguish implemented behavior from device and network configurations that still require router validation. In particular, transparent interception currently targets the primary LAN, not separate Merlin 3006 Guest Network Pro / SDN bridges.

Requires Merlin **384+**, JFFS custom scripts, SSH.

For contributors building ARM64 packages locally, use the
[Merlin local build guide](../docs/merlin-local-build.md). It documents the
compiler/linker separation and allocator checks required before packaging.

## Install

```sh
sh install.sh              # auto-detect CPU arch
sh install.sh hnd_v8       # platform tag (optional)
sh install.sh aarch64      # CPU arch
sh install.sh --update     # binary only, keep config
```

Offline install; no network required during `install.sh`.

## Uninstall

```sh
sh uninstall.sh
sh uninstall.sh --force
```

## Configuration

`/jffs/addons/sockrocket/config.yaml` (from template on install):

```yaml
mode: "tun"
dns_port: 5300

subscriptions:
  - name: "My provider"
    url: "https://example.com/subscribe?token=xxx"

nodes:
  - name: "my-server"
    server: "proxy.example.com"
    port: 443
    # protocol / transport fields per core schema
```

Reload after edits:

```sh
/jffs/addons/sockrocket/scripts/sockrocket.sh reload
```

## Subscription compatibility

Use **auto** for format detection; explicit **base64** is also accepted by the router API. Failed refreshes retain the saved URL and existing nodes. URI-list imports report the number of skipped entries. The Web UI starts one background job and polls its progress; closing or refreshing the page does not cancel it. Duplicate launches resume the active job. A failed download or worker exit reports a result without discarding saved subscriptions. Progress and job history stay in a private directory; request files are removed once claimed and at most eight job results are retained. An explicit manual Stop is preserved when the job finishes. Each launch has a client-generated request ID: recovery queries that exact job, and replaying the same ID returns the existing result. The software-center bridge discards inherited HTTP content lengths before forwarding its JSON body. A startup failure retains the original error and reports only the failed stage, error kind and OS code, without private URLs or local paths.

The downloader races three direct workers and three local SOCKS workers (at most six concurrent requests). Each worker makes an initial request and at most three retries for HTTP or transport failures, within one shared 30-second deadline. Only a response containing usable nodes can win; an HTTP error or HTML block page on one route does not cancel a valid download on the other. The losing request and its retry timers are cancelled. The 8 MiB response limit and private error handling remain in effect.

Hysteria2 share links support `security=tls`, `sni`, `insecure`, `alpn`, `pinSHA256` (or `pcs`), `upmbps`, `downmbps`, and `fastopen`. Bandwidth values accept non-negative decimal numbers (Mbps by default) and `bps`/`Kbps`/`Mbps`/`Gbps` suffixes; Clash numeric YAML fields are accepted too. Values round up to whole bytes per second and are bounded to 1,000,000 Mbps; 0 selects automatic/unlimited behavior. Upload payloads share one budget across TCP/UDP sessions using a token bucket with up to 16 KiB of initial credit and cancellation refunds. A UDP packet larger than the bucket charges debt that subsequent sends must repay. QUIC uses BBR, and download capacity is advertised to the server using `Hysteria-CC-RX`. Fast Open permits writes before the destination response; reads still validate that response and its timeout.

AnyTLS accepts `security=tls` and SHA-256 certificate pins through `pcs` or `pinSHA256`. A pin always checks the leaf certificate, including when `insecure=1`; a mismatch fails closed. Pins, Hysteria2 bandwidth and Fast Open settings survive saved configuration, share-link and Clash export/import. Conflicting pins and malformed parameters are rejected. Hysteria2 obfuscation and port hopping remain unsupported.

sing-box TLS settings that cannot yet be enforced, including `certificate_public_key_sha256` (SPKI pins), custom trust certificates, client certificates, ECH and version constraints, cause the node to be rejected. They are never silently dropped, including with `insecure=true`. This differs from the supported leaf-certificate pins in URI/Clash imports.

VLESS `xtls-rprx-vision` runs natively over TLS 1.3 or REALITY/TCP, including Vision padding and independent read/write raw-copy transitions. Non-HTTPS UDP uses XUDP through the Vision connection; the standard flow rejects UDP/443 to permit client TCP fallback. Unknown flows and Vision over WebSocket are rejected. The reference interoperability test covers ordinary TCP, TLS 1.3 data transfer, half-close and UDP without changing host routes.

**Exit**, available at the top of every plugin page, returns to Software Center while the proxy keeps its current state. Use **Stop service** on the Overview page to stop the proxy and run network cleanup; saved subscriptions and rules are preserved.

A fresh installation starts only the management API and leaves the proxy stopped.
Choose **Start service** explicitly after adding nodes. Boot and delayed install
retries preserve this stopped state; upgrades preserve the existing choice.
Software Center's enabled state follows the actual proxy daemon, separately from
the installed marker and management API. Stopping or a daemon exit clears it.

## Per-device routing

In **Routing rules**, set **Device source IP / subnet** to a client's fixed DHCP address, or a CIDR network for a device group. Leave it empty for a global rule. The destination condition still applies: two clients can send the same video domain to different actions (`proxy`, `direct`, or `reject`). A device-scoped final rule applies only to the remaining traffic from that device, after shared destination rules and built-in domestic routing.

In rule mode, matching order is:

1. Enabled device destination rules.
2. Global destination rules.
3. Built-in domestic / private-network direct routing.
4. Device final rules for the matching source.
5. Global final rules, or default proxy.

Within each phase, higher priority wins; ties keep configuration order. Overlapping source networks do not automatically prefer the longest prefix. Global and direct modes bypass this policy entirely.

For example, replace the documentation-only address below with the device's actual fixed LAN IP:

```yaml
rules:
  - source: "192.0.2.10/32"
    group: "Living room TV"
    rule_type: "domain-suffix"
    pattern: "video.example.invalid"
    target: "proxy"
    enabled: true
    priority: 100
```

Settings survive service and router restarts. Applying changes restarts the service; existing hardware-accelerated direct sessions may require reconnecting the client application. Bulk add accepts a shared source selector; native export preserves different selectors with `SOURCE,192.0.2.10/32,DOMAIN-SUFFIX,video.example.invalid,PROXY`. This prefix is Sockrocket syntax, not a portable Clash rule. Group editing retains the prefix; remove a device restriction with the individual rule editor by clearing its source.

The flow overview can preview a client's IP locally without changing configuration. It includes applicable overlapping subnets. A device final ends evaluation for that client after domestic and shared rules; it does not hide those earlier rules.

Merlin's transparent path captures the main IPv4 LAN. Router/private destinations keep the existing kernel bypass. Compatibility mode keeps the IPv6 forward guard and does not intercept LAN IPv6. Dual-stack mode adds IPv6 policy routing and DNS interception for global on-link prefixes, and leaves queries already addressed to the router's own IPv6 addresses alone; that path is not accepted on a live router yet. Changing the IPv6 mode updates DNS in the running process and reapplies the firewall without exiting, so a rejected combination does not restart-loop the LAN into bypass. When the active node cannot carry UDP, QUIC still reaches a proxy group selected by an unsourced rule when that group can carry UDP; other proxied QUIC gets port unreachable so clients fall back to TCP. IPv6 source matching is available in the core for supported ingress paths. HTTP/SOCKS identify the socket peer; NAT or an intermediate proxy can hide the original device. MAC discovery, per-device DNS resolvers, and different proxy nodes per device are not included. DNS/ICMP handling is unchanged; these are routing policies, not a replacement for router firewall access controls.

CN hardware bypass remains enabled for other clients. Sources with device policies enter the userspace router before the CN bypass, so the policy is not silently skipped. Enabling many such devices can increase router CPU load.

## Service commands

```sh
SR=/jffs/addons/sockrocket/scripts/sockrocket.sh
$SR start|stop|restart|reload|status|log|update-subs|watchdog
```

## iptables helper

```sh
IPT=/jffs/addons/sockrocket/scripts/iptables.sh
$IPT start|stop|status
```

## Layout on router

```
/jffs/addons/sockrocket/
├── sockrocket-cli
├── config.yaml
├── scripts/   sockrocket.sh, iptables.sh, dnsmasq.conf.template
└── webui/sockrocket.asp

/jffs/configs/dnsmasq.d/sockrocket.conf
/www/ext/sockrocket/sockrocket.asp
/www/cgi-bin/sockrocket.cgi  → sockrocket-cli (CGI/API)
```

API and config I/O are implemented in Rust (serde YAML); router-specific keys are preserved.

## Hooks (install.sh)

| File | When |
|------|------|
| `/jffs/scripts/firewall-start` | Re-apply firewall rules |
| `/jffs/scripts/nat-start` | Restore DNS interception after NAT rebuild |
| `/jffs/scripts/wan-event` | Restore policy route and DNS after WAN connects (PPPoE, DHCP, static IP) |
| `/jffs/scripts/wan-start` | Legacy Merlin WAN connection fallback |
| `/jffs/scripts/dhcpc-event` | Re-pin DNS after DHCPv4 lease changes; refresh IPv6 policy and DNS after DHCPv6 / RA updates |
| `/jffs/scripts/dnsmasq.postconf` | On Merlin 3006, give only the main LAN dnsmasq a private Sockrocket upstream; SDN DNS retains the firmware's shared upstream file |
| `/jffs/scripts/services-start` | Start Sockrocket |
| `/jffs/scripts/service-event` | Service restart hook |

Run the full offline `install.sh` to install updated hook scripts; `--update` replaces the binary only.

## Troubleshooting

```sh
/jffs/addons/sockrocket/scripts/sockrocket.sh log
/jffs/addons/sockrocket/scripts/iptables.sh status
iptables -t mangle -L SOCKROCKET_MANGLE -n -v
cat /jffs/configs/dnsmasq.d/sockrocket.conf
nslookup example.com 127.0.0.1
QUERY_STRING="action=status" /www/cgi-bin/sockrocket.cgi
```

**LAN offline** — `$IPT stop` to isolate iptables vs daemon.

**DNS not proxied** — match `dns_port` in config and dnsmasq snippet; `reload`.

**Subscription fetch fails** — test with `curl -v` from router (HTTPS CA bundle).

**Rules missing after reboot** — re-run `install.sh` or restore hooks in `firewall-start` / `services-start`.

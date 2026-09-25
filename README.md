# Sockrocket

<p align="center">
  <strong><a href="README.md">English</a> · <a href="README.vi.md">Tiếng Việt</a> · <a href="README.zh.md">中文</a></strong>
</p>

<p align="center">
  <img src="crates/sockrocket-gui/assets/logo-banner.svg" alt="Sockrocket Logo" width="360"/>
</p>

<p align="center">
  <a href="https://github.com/sockrockets/sockrocket/actions/workflows/ci.yml"><img src="https://github.com/sockrockets/sockrocket/actions/workflows/ci.yml/badge.svg" alt="CI"/></a>
  <a href="https://github.com/sockrockets/sockrocket/releases"><img src="https://github.com/sockrockets/sockrocket/actions/workflows/release.yml/badge.svg" alt="Release"/></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-blue.svg" alt="License"/></a>
</p>

**Sockrocket** is a Rust proxy client with a native GUI, a CLI, and a first-class **AsusWRT-Merlin** plugin. One stack from your laptop to the whole LAN.

---

## Why Sockrocket

### Advantages

- **Desktop and router, same protocols** — Shadowsocks, VMess, VLess, Trojan, TUIC, Hysteria2 (Reality, WebSocket, gRPC, ShadowTLS, …). What works in the GUI works on Merlin.
- **Through-proxy latency** — Node tests measure real exit-side RTT (warm path), not a fake TCP ping to `server:port`. Ranking matches what browsing feels like.
- **Subscriptions that just work** — Clash / V2Ray / SingBox URLs, auto format detect, update, dedupe.
- **Routing you can live with** — Domain / GeoIP / CIDR rules; **system proxy** for typical apps; **TUN** when something ignores the OS proxy.
- **Merlin without a zoo of binaries** — One musl `sockrocket-cli`, Web UI, iptables/DNS helpers, watchdog. Hot-reload node switch without tearing down listeners.
- **Light, open, private** — Apache-2.0; no account; no telemetry; EN / VI / 中文 UI.

### Compared with similar tools

Comparisons are about **fit**, not “better at everything”. Features change quickly — verify against current releases.

| | Sockrocket | Clash / Meta family | sing-box | Merlin plugins (e.g. fancyss / passwall-style) |
|--|------------|---------------------|----------|-----------------------------------------------|
| **Primary UX** | Native GUI + CLI + Merlin Web UI | Config / dashboard clients vary | CLI / 3rd-party UIs | Router Web UI only |
| **Codebase** | One Rust tree (GUI, CLI, router) | Core + many frontends | Core + ecosystems | Shell + many foreign bins |
| **Protocols** | SS / VMess / VLess / Trojan / TUIC / Hy2 | Broad (depends on core) | Very broad | Depends on bundled cores |
| **Latency test** | Through-proxy warm HTTP probe | Often TCP or mixed | Depends on UI | Often TCP / script ping |
| **Router story** | Official Merlin package, same engine | Usually run core via scripts | Scripts / containers | Mature, often multi-core |
| **LAN-wide proxy** | Merlin TUN + DNS hijack | Via router install of core | Via router install | Yes (main focus) |
| **License / model** | Apache-2.0, no SaaS | Open cores; UIs vary | Open | Mostly open / community |
| **Best when** | You want one product on PC **and** Merlin | You already live in Clash YAML | You need maximum protocol surface | You only care about the router |

**Pick Sockrocket if** you want a single maintained client for daily desktop use *and* Asus Merlin LAN proxy, with honest through-proxy node tests.  
**Pick Clash Meta / sing-box if** you need a specific ecosystem feature or rule dialect Sockrocket does not cover yet.  
**Pick classic Merlin suites if** you only run the router and already depend on their UI/scripting.

---

## Download (desktop)

[GitHub Releases](https://github.com/sockrockets/sockrocket/releases):

| Platform | GUI | CLI |
|----------|-----|-----|
| Linux x86_64 | `sockrocket-linux-x86_64.tar.gz` | `sockrocket-cli-linux-x86_64` |
| Linux aarch64 | `sockrocket-linux-aarch64.tar.gz` | `sockrocket-cli-linux-aarch64` |
| macOS Intel | `sockrocket-macos-x86_64.dmg` | `sockrocket-cli-macos-x86_64` |
| macOS Apple Silicon | `sockrocket-macos-aarch64.dmg` | `sockrocket-cli-macos-aarch64` |
| Windows x86_64 | `sockrocket-windows-x86_64.zip` | `sockrocket-cli-windows-x86_64.exe` |

```bash
# macOS — open the DMG, drag Sockrocket to Applications
open sockrocket-macos-aarch64.dmg

# Windows — extract ZIP, run Sockrocket.exe
# Linux — extract, then install (app menu + icons) or run portable:
tar xzf sockrocket-linux-x86_64.tar.gz
cd Sockrocket-*-linux-x86_64
./install.sh          # or: ./sockrocket
```

**macOS Gatekeeper:** Builds are not Apple-notarized unless release signing secrets are configured. If macOS blocks the app:

```bash
xattr -cr /Applications/Sockrocket.app
open /Applications/Sockrocket.app
```

Or: right-click → **Open** → **Open**. Or System Settings → Privacy & Security → **Open Anyway**.

Default listeners: **SOCKS5** `127.0.0.1:1080` · **HTTP** `127.0.0.1:1087`

---

## Use the GUI

1. Start the app.
2. **Subscriptions** — paste Clash / V2Ray / SingBox URL → Update. Or add a node manually.
3. **Nodes** — **Test** / **Test all** (through-proxy warm latency), then click a node to select it.
4. **Connect** — status shows connected; apps use the local proxy.
5. **Settings** (optional):
   - **System Proxy** — OS-level proxy for browsers and most apps.
   - **TUN** — capture traffic from apps that ignore system proxy (needs admin / capability).
6. **Language** — bottom-left status bar: EN → VI → 中文 (saved).

```bash
curl -x socks5://127.0.0.1:1080 https://www.google.com -I
```

Tips:

- Prefer **Test all** before picking a node; numbers are comparable warm through-proxy RTT.
- After switching nodes, wait a moment and re-test if you care about the warm figure.
- Keep subscriptions updated; stale nodes fail probes and waste time.

---

## Use the CLI

```bash
sockrocket-cli --init config.yaml
# Edit subscriptions: / nodes: / active_node, then:
sockrocket-cli config.yaml
```

Stop with `Ctrl+C`. Same ports as the GUI.

```yaml
listen_addr: "127.0.0.1"
socks_port: 1080
http_port: 1087
active_node: 0

subscriptions:
  - name: "my-sub"
    url: "https://example.com/subscribe"
    format: "auto"    # clash | v2ray | singbox | auto

rules:
  - rule_type: "geoip"
    pattern: "CN"
    target: "direct"
```

```bash
curl -x socks5://127.0.0.1:1080 https://www.google.com -I
curl -x http://127.0.0.1:1087 https://www.google.com -I
```

Full field reference: [docs/configuration.md](docs/configuration.md).

---

## AsusWRT-Merlin (LAN-wide proxy)

Sockrocket on the router proxies **phones, TVs, IoT, guests** — no client on each device. Same protocol engine as desktop.

### What you get

- Transparent proxy (TUN + iptables / policy routing)
- Optional DNS hijack (dnsmasq → Sockrocket DNS) to reduce pollution / leaks
- Web UI: nodes, subscriptions, latency test, toggles, logs
- Watchdog + cron subscription refresh
- Hot-reload when changing `active_node` (listeners stay up)

```
LAN clients → iptables / TUN → sockrocket-cli
            → rules (GeoIP, domain, CIDR) → proxy node or direct
```

### Requirements

- AsusWRT-Merlin **388.x+** recommended  
- **JFFS** + **custom scripts** enabled (Administration → System)  
- Package tag matching your SoC (same naming style as fancyss)

```bash
uname -m
# armv7l  → arm / hnd / qca / ipq32
# aarch64 → hnd_v8 / mtk / ipq64
```

### Platform packages

| Platform | Package | CPU | SoC family |
|----------|---------|-----|------------|
| `arm` | `sockrocket-merlin-arm.tar.gz` | armv7sf | BCM4708/4709 |
| `hnd` | `sockrocket-merlin-hnd.tar.gz` | armv7hf | BCM675x |
| `hnd_v8` | `sockrocket-merlin-hnd_v8.tar.gz` | aarch64 | BCM490x / BCM491x |
| `qca` | `sockrocket-merlin-qca.tar.gz` | armv7hf | IPQ807x |
| `mtk` | `sockrocket-merlin-mtk.tar.gz` | aarch64 | MT798x |
| `ipq32` / `ipq64` | `sockrocket-merlin-ipq32\|64.tar.gz` | armv7hf / aarch64 | IPQ53xx |

**Note:** koolshare `fancyss_hnd` covers both 490x and 675x; Sockrocket splits them into `hnd_v8` vs `hnd`. Wrong pick = softcenter reject or “Exec format error”.

```bash
uname -m
# armv7l  → arm / hnd / qca / ipq32
# aarch64 → hnd_v8 / mtk / ipq64
```

### Model → platform (common Asus Merlin)

Full tables (more SKUs, SoC notes): **[docs/merlin.md](docs/merlin.md)**. Co-branded editions share the base model’s platform.

| Platform | Models (examples) |
|----------|-------------------|
| **`hnd_v8`** | RT-AC86U, GT-AC2900, GT-AC5300, RT-AX88U, RT-AX88U_PRO, RAX80, GT-AX11000, GT-AX11000_PRO, RT-AX92U, RT-AX68U, **RT-AX86U**, **RT-AX86U_PRO**, RT-AX86S, GT-AXE11000, GT-AXE16000, GT-AX6000, ZenWiFi Pro XT12 |
| **`hnd`** | TUF-AX3000 / V2, TUF-AX5400, RT-AX58U / V2, RAX50, RT-AX82U / V2, ZenWiFi XT8 / XD4, RT-AX56U / V2, RT-AX57, RT-AX55 |
| **`arm`** | RT-AC68U, RT-AC66U_B1, RT-AC1900P, RT-AC87U, RT-AC88U, RT-AC3100, RT-AC3200, RT-AC5300 |
| **`qca`** | RT-AX89X / RT-AC89X |
| **`mtk`** | TUF-AX4200, TUF-AX6000, RT-AX59U, ZenWiFi BD4 |
| **`ipq32` / `ipq64`** | IPQ53xx boards — choose by `uname -m` (`armv7l` → ipq32, `aarch64` → ipq64) |

### Install

**Option A — koolcenter / softcenter**  
Upload the matching `sockrocket-merlin-*.tar.gz` in the software center and install.

**Option B — SSH**

```bash
# From your PC (replace host / package name)
scp sockrocket-merlin-hnd_v8.tar.gz admin@<router-lan-ip>:/tmp/

ssh admin@<router-lan-ip>
cd /tmp
tar -xzf sockrocket-merlin-hnd_v8.tar.gz
sh sockrocket/install.sh          # or: sh sockrocket/install.sh hnd_v8
```

Archive root is always `sockrocket/` (module name). Install copies binary, scripts, Web UI, and hooks `services-start`.

### First-time setup (Web UI)

1. Open `http://<router-lan-ip>/ext/sockrocket/sockrocket.asp` (or softcenter entry).
2. **Subscriptions** — add URL → update → wait for nodes.
3. **Nodes** — Test all → select a working node.
4. Enable **transparent proxy** and/or **DNS hijack** as needed.
5. Confirm phones on Wi‑Fi can reach foreign sites without a local client.

Or edit `/jffs/addons/sockrocket/config.yaml`:

```yaml
listen_addr: "0.0.0.0"
socks_port: 1080
http_port: 1087
dns_port: 5300

subscriptions:
  - name: "My subscription"
    url: "https://example.com/subscribe?token=xxx"
    format: "auto"

active_node: 0

rules:
  - rule_type: "ip-cidr"
    pattern: "192.168.0.0/16"
    target: "direct"
  - rule_type: "geoip"
    pattern: "CN"
    target: "direct"
```

### Service control

```bash
/jffs/addons/sockrocket/scripts/sockrocket.sh status
/jffs/addons/sockrocket/scripts/sockrocket.sh start|stop|restart
/jffs/addons/sockrocket/scripts/sockrocket.sh log
/jffs/addons/sockrocket/scripts/sockrocket.sh update-subs
```

Watchdog (every 5 minutes) and optional daily subscription cron are installed with the plugin.

### Merlin tips

- Switching nodes in the UI **hot-reloads** the outbound; avoid full restart unless the binary or listen ports change.
- Node **Test** uses the same through-proxy warm probe as desktop — comparable across nodes.
- Keep JFFS free space; rotate logs if you enable verbose tracing.
- TUN module must load for transparent mode (`ensure-tun` / watchdog helps after reboot).

### Uninstall

```bash
sh /jffs/addons/sockrocket/uninstall.sh
```

### Merlin troubleshooting

| Symptom | Check |
|---------|--------|
| No proxy effect | `sockrocket.sh status`; iptables `SOCKROCKET_*` chains; log |
| Dead after reboot | TUN loaded? `services-start` contains sockrocket? |
| Web UI 404 | `/www/ext/sockrocket/sockrocket.asp`, CGI symlink executable |
| Empty nodes | Subscription URL / update; `config.yaml` `nodes:` |
| DNS broken on LAN | DNS hijack toggle; `dnsmasq.d/sockrocket.conf`; fail-open if daemon down |

More detail: [docs/merlin.md](docs/merlin.md) · [docs/merlin-reference.md](docs/merlin-reference.md).

---

## Languages

| Where | Switch |
|-------|--------|
| README | [English](README.md) · [Tiếng Việt](README.vi.md) · [中文](README.zh.md) |
| GUI | Bottom-left status bar |
| Website | Top-right **EN / VI / 中文** |

CLI messages are English.

---

## More docs

| Doc | For |
|-----|-----|
| [Getting started](docs/getting-started.md) | First-run checklist |
| [Configuration](docs/configuration.md) | Config fields & routing |
| [Protocols](docs/protocols.md) | Protocol / transport details |
| [Merlin](docs/merlin.md) | Router install (short) |
| [Merlin reference](docs/merlin-reference.md) | Router internals |
| [Docs index](docs/README.md) | Everything else |

---

## License

[Apache License 2.0](LICENSE)

## AI training

This project **opts out** of use as AI / ML training data. See [`robots.txt`](robots.txt), [`ai.txt`](ai.txt), [`.well-known/tdmrep.json`](.well-known/tdmrep.json), [`.aiignore`](.aiignore).

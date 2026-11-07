# Sockrocket for AsusWRT-Merlin routers

<p align="center">
  <img src="crates/sockrocket-gui/assets/logo-banner.svg" alt="Sockrocket" width="320" />
</p>
## Open-source Rust · Small download · Low memory use

Sample offline installers weighed about 6.4 MiB. On an RT-AX86U Pro with 153–172 active sessions, the proxy used 29–37 MiB of resident memory. Install on the router to share nodes, rules and DNS across your home.

<p align="center"><strong>English</strong> · <a href="README.vi.md">Tiếng Việt</a> · <a href="README.zh.md">中文</a></p>
<p align="center">
  <a href="https://sockrockets.github.io/sockrocket/">Website</a> · <a href="https://t.me/sockrocket">Telegram · @sockrocket</a> ·
  <a href="https://github.com/sockrockets/sockrocket/releases/latest">Download</a> ·
  <a href="docs/getting-started.md">Get started</a> ·
  <a href="docs/routing.md">Routing</a> ·
  <a href="docs/merlin.md">Merlin guide</a> ·
  <a href="https://github.com/sockrockets/sockrocket/issues">Report a bug</a>
</p>

## Download for your router

Match the full model and hardware revision, download the linked package, then install it in Software Center. These are packaging targets. The architecture refers to the firmware userspace.

| Router models | Firmware architecture | Offline installer |
| --- | --- | --- |
| RT-AC68U, RT-AC68U V4, RT-AC66U B1, RT-AC1900, RT-AC1900P, RT-AC87U, Netgear R6300 V2, RT-AC88U, RT-AC3100, RT-AC3200, RT-AC5300 | ARMv7 soft-float | [sockrocket-merlin-arm.tar.gz](https://github.com/sockrockets/sockrocket/releases/latest/download/sockrocket-merlin-arm.tar.gz) |
| RT-BE92U, RT-BE82U, RT-BE58 Go, GS7 Pro, GS7 Pro Max, TUF-BE3600, TUF-BE3600 V2, TUF-AX5400 V2, ZenWiFi ET8, ZenWiFi XD6, ZenWiFi XT9, TUF-AX3000, TUF-AX3000 V2, TUF-AX5400, RT-AX58U, RT-AX58U V2, RAX70, RT-AX1800, RAX50, RT-AX82U, RT-AX82U V2, ZenWiFi XT8, ZenWiFi XT8 V2, ZenWiFi XD4, ZenWiFi XD4 Plus, RT-AX56U, RT-AX56U V2, RT-AX57, RT-AX55 | ARMv7 hard-float | [sockrocket-merlin-hnd.tar.gz](https://github.com/sockrockets/sockrocket/releases/latest/download/sockrocket-merlin-hnd.tar.gz) |
| RT-AC86U, GT-AC2900, GT-AC5300, RT-AX88U, RT-AX88U PRO, RAX80, GT-AX11000, GT-AX11000 PRO, RT-AX92U, RT-AX68U, RT-AX86U, RT-AX86U PRO, RT-AX86S, GT-AXE11000, GT-AXE16000, GT-AX6000, ZenWiFi Pro XT12, RT-BE88U, RT-BE7200, RT-BE86U, RT-BE96U, GT-BE96, GT-BE98, GT-BE98 Pro, GT-BE19000AI | AArch64 | [sockrocket-merlin-hnd_v8.tar.gz](https://github.com/sockrockets/sockrocket/releases/latest/download/sockrocket-merlin-hnd_v8.tar.gz) |
| RT-AX89X | ARMv7 hard-float | [sockrocket-merlin-qca.tar.gz](https://github.com/sockrockets/sockrocket/releases/latest/download/sockrocket-merlin-qca.tar.gz) |
| GS7, GS7 Air, GS-BE7200X, TX-AX6000, ZenWiFi BT8P, RT-AX57 Go, TUF-AX4200Q, TUF-AX4200, TUF-AX6000, RT-AX59U | AArch64 | [sockrocket-merlin-mtk.tar.gz](https://github.com/sockrockets/sockrocket/releases/latest/download/sockrocket-merlin-mtk.tar.gz) |
| ZenWiFi BD4 | ARMv7 hard-float | [sockrocket-merlin-ipq32.tar.gz](https://github.com/sockrockets/sockrocket/releases/latest/download/sockrocket-merlin-ipq32.tar.gz) |
| TUF-BE6500 | AArch64 | [sockrocket-merlin-ipq64.tar.gz](https://github.com/sockrockets/sockrocket/releases/latest/download/sockrocket-merlin-ipq64.tar.gz) |

![Actual Merlin management interface with nodes, addresses, SSIDs and credentials hidden](docs/images/readme/merlin.jpg)

## What it does

| Capability | Purpose |
| --- | --- |
| Native protocols | Shadowsocks, VMess, VLESS, Trojan, TUIC, Hysteria2, AnyTLS and WireGuard |
| Nodes and subscriptions | Import node links or Clash / V2Ray / sing-box subscriptions; update, search, deduplicate and test latency |
| Routing | Rule, Global and Direct. An exit is the global node, one node, or a node group. See [Routing](docs/routing.md) |
| Traffic capture | System proxy for compatible apps; TUN for broader capture |
| Connections and traffic | Overview counts bytes since this process started. Subscriptions show the last three calendar months per node and per subscription |
| Shared engine | Desktop GUI, CLI and Merlin WebUI |
| Languages | English, Tiếng Việt and 中文 |

Protocol combinations have limits: ShadowTLS does not provide UDP; native VMess and WireGuard UDP have per-packet size limits. VLESS Vision supports TLS 1.3/REALITY over TCP and XUDP; the standard flow rejects UDP/443. VLESS XHTTP supports packet-up, stream-up and stream-one, HTTP/1.1–3, REALITY and split download transports; stream-one requires HTTP/2 or HTTP/3. gRPC fails explicitly. See the [protocol guide](docs/protocols.md). Sockrocket does not provide proxy nodes or subscription services.

## New protocols: AnyTLS and WireGuard

> **Version scope:** these capabilities describe the current source. Packages on GitHub Releases follow their respective release notes.

| Protocol | Capabilities | Current limits |
| --- | --- | --- |
| AnyTLS v2 | Native TLS, session reuse, TCP and UDP-over-TCP v2; share link, Clash and sing-box import | Certificates verified by default; UDP is carried over TCP |
| WireGuard | Native userspace TCP / UDP, preshared keys and tunnel DNS; single-peer profile, Clash and sing-box import | One peer per node; UDP must fit the MTU; no IP fragmentation or profile scripts |

Desktop, CLI and Merlin share these implementations and existing local configuration persistence. The WireGuard protocol path creates no additional OS interface and changes no system routes or DNS settings; application-level TUN capture is controlled separately. Both protocols have passed interoperability tests against independent servers.

See the [protocol guide](docs/protocols.md) for formats, limits and the remaining transport work.

## Interface previews

### Keep sources separate from node selection

Subscriptions manage where nodes come from; node selection chooses the current exit. Latency tests run through the proxy. Milliseconds describe response latency during the test, not download bandwidth.

![Nodes design preview with fictional nodes, subscription sources and latency](docs/images/readme/nodes.png)

### Inspect and edit routing rules

Review match conditions, targets and enabled states. Rule order and priority affect the result. A rule group is a folder of rules. A node group is an exit. Only Rule mode reads either of them.

![Routing design preview showing match conditions, targets and edit actions](docs/images/readme/rules.png)

### Work with multiple rules as text

Bulk paste and group editing help migrate existing rules. Check parsing, targets and conflict handling before saving. The example domain `example.com` is reserved for documentation.

![Bulk rule design preview: text, validation and confirmation](docs/images/readme/rules-bulk.png)

## Where a rule sends traffic

Global mode uses the node selected at the top of the page. Direct does not use a proxy. Rule mode is the only mode that reads rules.

| Exit | What it follows |
| --- | --- |
| Global node | The node selected at the top, including a health auto-switch |
| One node | That node only. Changing the top selection leaves this rule where it is |
| Node group | That group's members and strategy. Switching a member does not change the global node |

A manual group uses one node. Fallback tries members in list order and, for five minutes, starts again at the last success; UDP stays on the first member. URL test keeps the fastest recent handshake for five minutes, and list order does not pick the winner. The group does nothing until a rule's exit is that group. Full steps: [Routing](docs/routing.md).

## On the Merlin pages

The desktop app shares this core. These controls live in the router Web UI.

| Page | What you can do |
| --- | --- |
| Overview | Start turns DNS interception and transparent proxy on together. Either can be turned off while the proxy is running. Traffic here is since this process started |
| Nodes | Use one node, or a node group. A missing pinned node fails closed and does not fall back to the global node. Health auto-switch moves only the global node |
| Subscriptions | Update subscriptions. The last three calendar months include manual nodes and Direct. A subscription total is the sum of nodes that currently belong to it. Addresses are not shown |
| Routing rules | Set a rule folder's exit, preview and append rules from a URL, check where a site would match without opening a connection, or add presets. Presets start disabled: AI, overseas video, Telegram web, developer tools, Apple, Microsoft and domestic sites. They are common domains, not every endpoint |
| Device default | A final exit for one device or prefix. Other devices keep the shared rules. Use a DHCP reservation. Inherit removes that source's final rule and leaves destination exceptions |
| Network & DNS | Compatibility is the default and does not need ISP IPv6. Dual-stack is for firmware that already has an IPv6 address and a LAN prefix. Saving reapplies the firewall without restarting a running proxy |

## Download and connect

Each filename below is that file on the [latest release](https://github.com/sockrockets/sockrocket/releases/latest). The [website download table](https://sockrockets.github.io/sockrocket/#download) uses the same names and resolves them to the current release when the page opens. Router packages are listed there by firmware platform.

| System | Desktop package |
| --- | --- |
| macOS · Apple Silicon | [sockrocket-macos-aarch64.dmg](https://github.com/sockrockets/sockrocket/releases/latest/download/sockrocket-macos-aarch64.dmg) |
| macOS · Intel | [sockrocket-macos-x86_64.dmg](https://github.com/sockrockets/sockrocket/releases/latest/download/sockrocket-macos-x86_64.dmg) |
| Windows · x86_64 | [sockrocket-windows-x86_64.zip](https://github.com/sockrockets/sockrocket/releases/latest/download/sockrocket-windows-x86_64.zip) |
| Linux · x86_64 | [sockrocket-linux-x86_64.tar.gz](https://github.com/sockrockets/sockrocket/releases/latest/download/sockrocket-linux-x86_64.tar.gz) |
| Linux · ARM64 | [sockrocket-linux-aarch64.tar.gz](https://github.com/sockrockets/sockrocket/releases/latest/download/sockrocket-linux-aarch64.tar.gz) |

1. Install and open the app. On macOS, drag it into Applications first. On Windows, extract the ZIP; on Linux, follow the package's install or portable-run instructions.
2. Add your own subscription under **Subscriptions**, or import a link under **Nodes**.
3. Update the subscription and select a node. Run a latency test if needed.
4. Click **Connect** in **Overview**, wait for reachability verification, then check the system proxy state.
5. Enable **TUN** only when broader capture is needed, and complete the operating system's administrator prompt.

Before upgrading, quit the old client and its elevated background instance. Running two versions can occupy the same ports and compete for TUN routes.

## AsusWRT-Merlin

**Small Merlin packages and low memory use**

In the v0.0.19 packaging test, the offline installer was 6.36 MiB for ARMv7 and 6.43 MiB for ARM64, about 49% smaller after removing a duplicate executable. In a separate RT-AX86U Pro test with 153–172 active sessions, the proxy process used 29–37 MiB of resident memory (RSS). Release size varies by build, and memory use varies with workload. [Router memory measurements](docs/architecture/tls-failure-investigation-2026-11-06.md)

The router plugin includes a WebUI, transparent proxy, DNS capture and service checks, sharing the engine across LAN devices.


1. If the menu has no Software Center, download a Merlin or official-mod build for your model from [fw.koolcenter.com](https://fw.koolcenter.com/) and flash it. Upstream Merlin does not include Software Center.
2. Find your exact model and firmware architecture in the [model-to-package table](docs/merlin.md#model--platform). Each release also includes `merlin-models.md`.
3. Follow the [Merlin guide](docs/merlin.md) to configure JFFS / custom scripts and install the package.
4. Configure subscriptions locally on the router. Verify the node before enabling DNS capture or transparent proxy. To send some traffic through a node group, stay in Rule mode and set that rule's exit to the group.
5. Back up configuration before updates and verify manual stop / network recovery before relying on the setup.

```sh
/jffs/addons/sockrocket/scripts/sockrocket.sh status
/jffs/addons/sockrocket/scripts/sockrocket.sh stop
/jffs/addons/sockrocket/scripts/sockrocket.sh start
```

Platform selection, service control and recovery: [Merlin guide](docs/merlin.md) · [Technical reference](docs/merlin-reference.md).

## System proxy or TUN?

| Mode | Use it for | Considerations |
| --- | --- | --- |
| System proxy | Browsers and apps that follow OS proxy settings | Some apps ignore these settings |
| TUN | Broader application traffic capture | Requires administrator privileges and TUN support; stop other TUN / VPN clients first |
| CLI listener | Terminal tools or manually configured apps | Listens locally by default; configure the calling app |

Default local listeners: SOCKS5 `127.0.0.1:1080` and HTTP `127.0.0.1:1087`. These are loopback listeners, not remote nodes.

## Command line

CLI downloads are on the same release page and start with `sockrocket-cli-`.

```sh
sockrocket-cli --init config.yaml
# Edit the local config with your own subscription or nodes.
sockrocket-cli config.yaml
```

Stop with `Ctrl+C`. See the [configuration reference](docs/configuration.md) for fields and safe examples. Never commit a configuration containing real credentials.

## Troubleshooting

| Symptom | Start here |
| --- | --- |
| Connect fails to start | Check old background instances, occupied local ports and the error log |
| TUN will not start | Check administrator approval, driver support and another TUN owning the default route |
| Connected, but websites fail | Check node reachability, system proxy / TUN, DNS and routing separately |
| Subscription fails or returns no nodes | Inspect the URL, format, server response and parser result locally |
| Only some websites fail | Check matched rules, the direct exit and DNS; avoid multiple clients capturing traffic |
| A node group is unused | Overview must be Rule mode, and a rule exit must be Group · name. Global and Direct ignore groups |
| Traffic looks empty after a restart | Overview counters start again with the process. The three-month table is on Subscriptions |

## Community & contact

| | |
| --- | --- |
| [Telegram · @sockrocket](https://t.me/sockrocket) | Release announcements and project updates |
| [GitHub Discussions](https://github.com/sockrockets/sockrocket/discussions) | Usage questions and feature ideas |
| [GitHub Issues](https://github.com/sockrockets/sockrocket/issues) | Reproducible bug reports |
| [Website](https://sockrockets.github.io/sockrocket/) | Product overview, downloads and docs |

## Privacy and feedback

Configuration, backups, and runtime logs stay on the machine and use private file permissions. The management API listens on localhost only. The default log level stays at info unless a diagnostic log file is set. See [Privacy](docs/privacy-protection.md).

Desktop design illustrations use fictional data; the Merlin capture is sanitized before export. Before sharing an Issue, log or screenshot, remove full subscription URLs and query strings, server addresses, passwords, UUIDs, tokens, QR codes, personal paths and account details. QR codes and share links can contain complete credentials. Commits and tags use the project identity. A `noreply` address is not personal information.

- [Issues](https://github.com/sockrockets/sockrocket/issues): bug reports.
- [Discussions](https://github.com/sockrockets/sockrocket/discussions): questions and usage.
- [Documentation index](docs/README.md): routing, configuration, protocols, architecture and development.

## License

[Apache License 2.0](LICENSE). See [ai.txt](ai.txt), [robots.txt](robots.txt) and the [TDM declaration](.well-known/tdmrep.json) for the project's AI / ML training-use statements.

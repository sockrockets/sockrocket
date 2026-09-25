# Sockrocket — AsusWRT-Merlin plugin

Transparent proxy integration for **AsusWRT-Merlin** routers.

## Features

- **Transparent proxy** — iptables/policy routing for LAN TCP (no client config)
- **DNS** — dnsmasq forwards to Sockrocket DNS listener (leak reduction)
- **Subscriptions** — manage URLs from Web UI
- **Node selection** — switch nodes and run latency tests
- **Modes** — TUN, SOCKS5, or system-style proxy (see `config.yaml`)
- **Watchdog** — restarts crashed daemon; optional subscription cron
- **Web UI** — `/ext/sockrocket/sockrocket.asp`

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

Requires Merlin **384+**, JFFS custom scripts, SSH.

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
| `/jffs/scripts/services-start` | Start Sockrocket |
| `/jffs/scripts/service-event` | Service restart hook |

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

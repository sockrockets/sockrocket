# Merlin router plugin

Run Sockrocket on **AsusWRT-Merlin** for LAN-wide transparent proxy without per-device client setup. Same protocols as desktop: Shadowsocks, VMess, VLess, TUIC, Trojan, Hysteria2.

---

## Requirements

- Asus router with **AsusWRT-Merlin** (388.x+ recommended; koolshare / official Merlin forks with software center)
- **JFFS** and **custom scripts** enabled (Administration → System → Persistent JFFS2)
- Offline package whose `.valid` platform tag matches your SoC (same naming style as [fancyss](https://github.com/hq450/fancyss))

```bash
uname -m
# armv7l  → arm / hnd / qca / ipq32
# aarch64 → hnd_v8 / mtk / ipq64
```

---

## Platforms and packages

| Platform | Package | CPU binary | SoC family |
|----------|---------|------------|------------|
| `arm` | `sockrocket-merlin-arm.tar.gz` | armv7 softfloat | BCM4708 / BCM4709 |
| `hnd` | `sockrocket-merlin-hnd.tar.gz` | armv7 hardfloat | BCM6750 / BCM6755 (`axhnd.675x`) |
| `hnd_v8` | `sockrocket-merlin-hnd_v8.tar.gz` | aarch64 | BCM4906 / BCM4908 / BCM4912 / BCM4916 |
| `qca` | `sockrocket-merlin-qca.tar.gz` | armv7 hardfloat | Qualcomm IPQ807x |
| `mtk` | `sockrocket-merlin-mtk.tar.gz` | aarch64 | MediaTek Filogic / MT798x |
| `ipq32` | `sockrocket-merlin-ipq32.tar.gz` | armv7 hardfloat | Qualcomm IPQ53xx **32-bit** userspace |
| `ipq64` | `sockrocket-merlin-ipq64.tar.gz` | aarch64 | Qualcomm IPQ53xx **64-bit** userspace |

**vs fancyss:** koolshare’s `fancyss_hnd` covers both BCM490x (ARMv8) and BCM675x (ARMv7) in one tree. Sockrocket **splits** them into `hnd_v8` (aarch64) and `hnd` (armv7hf) so the binary matches the CPU. Picking the wrong one is the most common install failure.

---

## Model → platform

Co-branded / limited editions (Gundam, Call of Duty, Haiyan, …) use the **same** platform as the base SKU. When a model exists as both 梅林改 and 官改, the Sockrocket package tag is the same; only the firmware/software-center build differs.

### `hnd_v8` — BCM490x / BCM491x (aarch64)

| Model | Typical SoC |
|-------|-------------|
| RT-AC86U | BCM4906 |
| GT-AC2900 | BCM4906 |
| GT-AC5300 | BCM4908 |
| RT-AX88U | BCM4908 |
| RT-AX88U_PRO | BCM4912 |
| RAX80 | BCM4908 |
| GT-AX11000 | BCM4908 |
| GT-AX11000_PRO | BCM4912 |
| RT-AX92U | BCM4906 |
| RT-AX68U | BCM4906 |
| RT-AX86U | BCM4908 |
| RT-AX86U_PRO | BCM4908 |
| RT-AX86S | BCM4908 |
| GT-AXE11000 | BCM4908 |
| GT-AXE16000 | BCM4916 |
| GT-AX6000 | BCM4912 |
| ZenWiFi Pro XT12 | BCM4912 |

### `hnd` — BCM675x (armv7hf)

| Model | Typical SoC |
|-------|-------------|
| TUF-AX3000 | BCM6750 |
| TUF-AX3000_V2 | BCM6755 |
| TUF-AX5400 | BCM6750 |
| RT-AX58U | BCM6750 |
| RT-AX58U_V2 | BCM6755 |
| RAX50 | BCM6750 |
| RT-AX82U | BCM6750 |
| RT-AX82U_V2 | BCM6755 |
| ZenWiFi XT8 | BCM6755 |
| ZenWiFi XT8_V2 | BCM6755 |
| ZenWiFi XD4 | BCM6755 |
| ZenWiFi XD4_Plus | BCM6755 |
| RT-AX56U | BCM6755 |
| RT-AX56U_V2 | BCM6755 |
| RT-AX57 | BCM6750 |
| RT-AX55 | BCM6750 |

### `arm` — BCM4708 / BCM4709 (armv7sf)

For koolshare Merlin **384/386** (and similar) on classic AC series — not stock Asus-only firmware without software center.

| Model | Typical SoC |
|-------|-------------|
| RT-AC68U / RT-AC68U_V4 | BCM4708 |
| RT-AC66U_B1 | BCM4708 |
| RT-AC1900 / RT-AC1900P | BCM4708 |
| RT-AC87U | BCM4708 |
| RT-AC88U | BCM4709 |
| RT-AC3100 | BCM4709 |
| RT-AC3200 | BCM4709 |
| RT-AC5300 | BCM4709 |

### `qca` — IPQ807x

| Model | Typical SoC |
|-------|-------------|
| RT-AX89X / RT-AC89X | IPQ8074 |

### `mtk` — MediaTek Filogic / MT798x (aarch64)

| Model | Typical SoC |
|-------|-------------|
| TUF-AX4200 | MT7986 |
| TUF-AX6000 | MT7986 |
| RT-AX59U | MT7986 |
| ZenWiFi BD4 | MT7981 |

Newer Filogic SKUs: if softcenter rejects the offline package, check which `.valid` tag your firmware’s software center expects (some forks use custom tags).

### `ipq32` / `ipq64` — IPQ53xx

| Platform | Use when |
|----------|----------|
| `ipq32` | `uname -m` is `armv7l` on an IPQ53xx board |
| `ipq64` | `uname -m` is `aarch64` on an IPQ53xx board |

Prefer `uname -m` over the marketing model name — the same chassis can ship 32-bit or 64-bit userspace depending on the firmware port.

---

## Install

### 1. Download

From [GitHub Releases](https://github.com/sockrockets/sockrocket/releases), get `sockrocket-merlin-<platform>.tar.gz` for your row in the tables above.

### 2. Upload

```bash
scp sockrocket-merlin-hnd_v8.tar.gz admin@<router-lan-ip>:/tmp/
```

### 3. Install on router

```bash
ssh admin@<router-lan-ip>
cd /tmp
tar -xzf sockrocket-merlin-hnd_v8.tar.gz
sh sockrocket/install.sh            # auto-detect, or:
sh sockrocket/install.sh hnd_v8     # force platform tag
```

Archive root is always `sockrocket/` (koolcenter module name).

---

## Configure

```bash
vi /jffs/addons/sockrocket/config.yaml
```

```yaml
listen_addr: "0.0.0.0"
socks_port: 1080
http_port: 1087

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

Or use the Web UI: `http://<router-lan-ip>/ext/sockrocket/sockrocket.asp`

---

## Service control

```bash
/jffs/addons/sockrocket/scripts/sockrocket.sh start|stop|restart|status|log|update-subs
```

Boot hook: `/jffs/scripts/services-start` (installed by `install.sh`).

---

## Transparent proxy (summary)

```
LAN clients → iptables mangle/nat → sockrocket (HTTP/SOCKS/DNS/TUN)
           → rules (GeoIP, domain) → proxy node or direct
```

Details: [Merlin technical reference](./merlin-reference.md).

---

## Uninstall

```bash
sh /jffs/addons/sockrocket/uninstall.sh
```

---

## Troubleshooting

| Issue | Checks |
|-------|--------|
| Softcenter rejects package | Wrong platform `.valid` — re-check model table / `uname -m` |
| No proxy effect | `sockrocket.sh status`; `iptables -t nat -L SOCKROCKET_PREROUTING -n`; logs |
| No boot start | `grep sockrocket /jffs/scripts/services-start` |
| Web UI 404 | `/www/ext/sockrocket/sockrocket.asp`, CGI executable |
| Empty nodes | Subscription URL / update; `nodes:` in `config.yaml` |
| Binary won’t run | `hnd` vs `hnd_v8` mismatch (675x vs 490x) is the usual cause |

---

## See also

- [merlin-reference.md](./merlin-reference.md) — iptables, DNS, watchdog internals  
- [fancyss README](https://github.com/hq450/fancyss) — upstream platform / firmware notes  
- [koolcenter](https://www.koolcenter.com/) — Merlin / 官改 firmware downloads  

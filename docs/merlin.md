# Merlin router plugin

**Small packages and low memory use**

In the v0.0.19 packaging test, the offline installer was 6.36 MiB for ARMv7 and 6.43 MiB for ARM64, about 49% smaller after removing a duplicate executable. In a separate RT-AX86U Pro test with 153–172 active sessions, the proxy process used 29–37 MiB of resident memory (RSS). Release size varies by build, and memory use varies with workload. [Router memory measurements](architecture/tls-failure-investigation-2026-11-06.md)

Run Sockrocket on **AsusWRT-Merlin** for LAN-wide transparent proxy without per-device client setup. Same protocols as desktop: Shadowsocks, VMess, VLess, TUIC, Trojan, Hysteria2.

---

## Requirements

- Asus router with **AsusWRT-Merlin** (388.x+ recommended; koolshare / official Merlin forks with software center)

Upstream Merlin from [asuswrt-merlin.net](https://www.asuswrt-merlin.net/download) does not include Software Center. Flash a build that does, from [fw.koolcenter.com](https://fw.koolcenter.com/):

| Line | Directory |
|------|-----------|
| Merlin 3004.388 | [KoolCenter_梅林改版固件_3004.388](https://fw.koolcenter.com/KoolCenter_梅林改版固件_3004.388/) |
| Merlin 3006.102 | [KoolCenter_梅林改版固件_3006.102](https://fw.koolcenter.com/KoolCenter_梅林改版固件_3006.102/) |
| Asus official-mod | [KoolCenter_华硕官改固件](https://fw.koolcenter.com/KoolCenter_华硕官改固件/) |
| Older 3004.386 | [KoolCenter_梅林改版固件_3004.386](https://fw.koolcenter.com/KoolCenter_梅林改版固件_3004.386/) |

Open the folder named like the model on Administration → Firmware Upgrade. Co-branded editions without their own folder use the base model. Download the newest stable file in that folder, not anything under `alpha` or `beta`. Upgrade from the router WebUI and wait for reboot. Software Center appears after that. Official-mod and Merlin builds use the same Sockrocket platform package.
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
| `hnd` | `sockrocket-merlin-hnd.tar.gz` | armv7 hardfloat | BCM675x / BCM676x with 32-bit userspace |
| `hnd_v8` | `sockrocket-merlin-hnd_v8.tar.gz` | aarch64 | BCM4906 / BCM4908 / BCM4912 / BCM4916 |
| `qca` | `sockrocket-merlin-qca.tar.gz` | armv7 hardfloat | Qualcomm IPQ807x |
| `mtk` | `sockrocket-merlin-mtk.tar.gz` | aarch64 | MediaTek Filogic / MT798x |
| `ipq32` | `sockrocket-merlin-ipq32.tar.gz` | armv7 hardfloat | Qualcomm IPQ53xx **32-bit** userspace |
| `ipq64` | `sockrocket-merlin-ipq64.tar.gz` | aarch64 | Qualcomm IPQ53xx **64-bit** userspace |

The legacy ARM installer accepts `arm384` or `arm386` in `.valid`. The current packer writes `arm386`; the original v0.0.19 release wrote `arm`. The Broadcom HND installer recognizes the substring `hnd` for both HND packages, so choose the ABI for the firmware from the model table.

---

## Model → platform

[完整中文型号与安装包对照](merlin-models.md) · The same guide ships with the packages.

Co-branded / limited editions (Gundam, Call of Duty, Haiyan, …) use the **same** platform as the base SKU. When a model exists as both 梅林改 and 官改, the Sockrocket package tag is the same; only the firmware/software-center build differs.

### `hnd_v8` — BCM490x / BCM491x (aarch64)

| Model | Typical SoC | Sockrocket installation package |
|-------|-------------|---------------------------------|
| RT-AC86U | BCM4906 | `sockrocket-merlin-hnd_v8.tar.gz` |
| GT-AC2900 | BCM4906 | `sockrocket-merlin-hnd_v8.tar.gz` |
| GT-AC5300 | BCM4908 | `sockrocket-merlin-hnd_v8.tar.gz` |
| RT-AX88U | BCM4908 | `sockrocket-merlin-hnd_v8.tar.gz` |
| RT-AX88U_PRO | BCM4912 | `sockrocket-merlin-hnd_v8.tar.gz` |
| RAX80 | BCM4908 | `sockrocket-merlin-hnd_v8.tar.gz` |
| GT-AX11000 | BCM4908 | `sockrocket-merlin-hnd_v8.tar.gz` |
| GT-AX11000_PRO | BCM4912 | `sockrocket-merlin-hnd_v8.tar.gz` |
| RT-AX92U | BCM4906 | `sockrocket-merlin-hnd_v8.tar.gz` |
| RT-AX68U | BCM4906 | `sockrocket-merlin-hnd_v8.tar.gz` |
| RT-AX86U | BCM4908 | `sockrocket-merlin-hnd_v8.tar.gz` |
| RT-AX86U_PRO | BCM4912 | `sockrocket-merlin-hnd_v8.tar.gz` |
| RT-AX86S | BCM4906 | `sockrocket-merlin-hnd_v8.tar.gz` |
| GT-AXE11000 | BCM4908 | `sockrocket-merlin-hnd_v8.tar.gz` |
| GT-AXE16000 | BCM491x | `sockrocket-merlin-hnd_v8.tar.gz` |
| GT-AX6000 | BCM4912 | `sockrocket-merlin-hnd_v8.tar.gz` |
| ZenWiFi Pro XT12 | BCM4912 | `sockrocket-merlin-hnd_v8.tar.gz` |

### Additional models — explicit package selection

These package assignments target KoolCenter / ASUSGO software-center firmware. They describe architecture compatibility, not completed on-device Sockrocket testing. BCM6764/6765/6766 firmware uses the 32-bit `hnd` package; Wi-Fi 7 does not imply `hnd_v8`. Upstream Merlin alone does not provide the software center, including on GT-BE19000AI.

| Model / firmware name | SoC / firmware | Sockrocket installation package |
|-----------------------|----------------|---------------------------------|
| RT-BE88U / RT-BE7200 | BCM4916 | `sockrocket-merlin-hnd_v8.tar.gz` |
| RT-BE86U | BCM4916 | `sockrocket-merlin-hnd_v8.tar.gz` |
| RT-BE96U / GT-BE96 / GT-BE98 / GT-BE98 Pro | BCM4916 | `sockrocket-merlin-hnd_v8.tar.gz` |
| RT-BE92U | BCM6765 | `sockrocket-merlin-hnd.tar.gz` |
| RT-BE82U | BCM6766 | `sockrocket-merlin-hnd.tar.gz` |
| RT-BE58 Go | BCM6764 | `sockrocket-merlin-hnd.tar.gz` |
| GT-BE19000AI | BCM4916 | `sockrocket-merlin-hnd_v8.tar.gz` |
| GS7 (ROG 魔盒) | MT7988D | `sockrocket-merlin-mtk.tar.gz` |
| GS7 Air (ROG 魔盒 Air) | MediaTek | `sockrocket-merlin-mtk.tar.gz` |
| GS-BE7200X (ROG 魔盒 X) | MediaTek | `sockrocket-merlin-mtk.tar.gz` |
| GS7 Pro (ROG 魔盒 Pro) | BCM6766 | `sockrocket-merlin-hnd.tar.gz` |
| GS7 Pro Max (ROG 魔盒 Pro Max) | BCM6766 | `sockrocket-merlin-hnd.tar.gz` |
| TUF-BE3600 | BCM6764L | `sockrocket-merlin-hnd.tar.gz` |
| TUF-BE3600 V2 | BCM6764 | `sockrocket-merlin-hnd.tar.gz` |
| TUF-BE6500 | IPQ5322, 64-bit firmware | `sockrocket-merlin-ipq64.tar.gz` |
| TX-AX6000 | MT7986A | `sockrocket-merlin-mtk.tar.gz` |
| ZenWiFi BT8P | MT7988D | `sockrocket-merlin-mtk.tar.gz` |
| TUF-AX5400 V2 | BCM6750 | `sockrocket-merlin-hnd.tar.gz` |
| RT-AX57 Go | MT7981 | `sockrocket-merlin-mtk.tar.gz` |
| ZenWiFi ET8 | BCM6755 | `sockrocket-merlin-hnd.tar.gz` |
| ZenWiFi XD6 | BCM6750 | `sockrocket-merlin-hnd.tar.gz` |
| ZenWiFi XT9 | BCM6756 | `sockrocket-merlin-hnd.tar.gz` |

Research sources (2026-11-04): [fancyss 3.0 model and architecture table](https://github.com/hq450/fancyss/blob/3.0/README.md), [software-center platform selection](https://github.com/koolshare/rogsoft/blob/master/softcenter/softcenter/scripts/ks_tar_install.sh), [ASUSGO model documentation](https://doc.asusgo.com/zh/guide/asus/gs7_pro.html), [GS7 Pro Max](https://doc.asusgo.com/zh/guide/asus/gs7_promax.html), [BE82U](https://doc.asusgo.com/zh/guide/asus/rt-be82u.html), [BE58 Go](https://doc.asusgo.com/zh/guide/asus/rt-be58_go.html), [BE92U](https://doc.asusgo.com/zh/guide/asus/rt-be92u.html), and [TUF-BE3600 V2](https://doc.asusgo.com/zh/guide/asus/tuf-be3600_v2.html). Package names are mapped to Sockrocket's own build targets, not copied from another plugin's download links.

### `hnd` — BCM675x (armv7hf)

| Model | Typical SoC | Sockrocket installation package |
|-------|-------------|---------------------------------|
| TUF-AX3000 | BCM6750 | `sockrocket-merlin-hnd.tar.gz` |
| TUF-AX3000_V2 | BCM6756 | `sockrocket-merlin-hnd.tar.gz` |
| TUF-AX5400 | BCM6750 | `sockrocket-merlin-hnd.tar.gz` |
| RT-AX58U | BCM6750 | `sockrocket-merlin-hnd.tar.gz` |
| RT-AX58U_V2 | BCM6755 | `sockrocket-merlin-hnd.tar.gz` |
| RAX70 | BCM6755 | `sockrocket-merlin-hnd.tar.gz` |
| RT-AX1800 | BCM6755 | `sockrocket-merlin-hnd.tar.gz` |
| RAX50 | BCM6750 | `sockrocket-merlin-hnd.tar.gz` |
| RT-AX82U | BCM6750 | `sockrocket-merlin-hnd.tar.gz` |
| RT-AX82U_V2 | BCM6755 | `sockrocket-merlin-hnd.tar.gz` |
| ZenWiFi XT8 | BCM6755 | `sockrocket-merlin-hnd.tar.gz` |
| ZenWiFi XT8_V2 | BCM6755 | `sockrocket-merlin-hnd.tar.gz` |
| ZenWiFi XD4 | BCM6755 | `sockrocket-merlin-hnd.tar.gz` |
| ZenWiFi XD4_Plus | BCM6755 | `sockrocket-merlin-hnd.tar.gz` |
| RT-AX56U | BCM6755 | `sockrocket-merlin-hnd.tar.gz` |
| RT-AX56U_V2 | BCM6755 | `sockrocket-merlin-hnd.tar.gz` |
| RT-AX57 | BCM6750 | `sockrocket-merlin-hnd.tar.gz` |
| RT-AX55 | BCM6750 | `sockrocket-merlin-hnd.tar.gz` |

### `arm` — BCM4708 / BCM4709 (armv7sf)

For koolshare Merlin **384/386** (and similar) on classic AC series — not stock Asus-only firmware without software center.

| Model | Typical SoC | Sockrocket installation package |
|-------|-------------|---------------------------------|
| RT-AC68U / RT-AC68U_V4 | BCM4708 | `sockrocket-merlin-arm.tar.gz` |
| RT-AC66U_B1 | BCM4708 | `sockrocket-merlin-arm.tar.gz` |
| RT-AC1900 / RT-AC1900P | BCM4708 | `sockrocket-merlin-arm.tar.gz` |
| RT-AC87U | BCM4708 | `sockrocket-merlin-arm.tar.gz` |
| Netgear R6300 V2 | BCM4708 | `sockrocket-merlin-arm.tar.gz` |
| RT-AC88U | BCM4709 | `sockrocket-merlin-arm.tar.gz` |
| RT-AC3100 | BCM4709 | `sockrocket-merlin-arm.tar.gz` |
| RT-AC3200 | BCM4709 | `sockrocket-merlin-arm.tar.gz` |
| RT-AC5300 | BCM4709 | `sockrocket-merlin-arm.tar.gz` |

### `qca` — IPQ807x

| Model | Typical SoC | Sockrocket installation package |
|-------|-------------|---------------------------------|
| RT-AX89X | IPQ8074 | `sockrocket-merlin-qca.tar.gz` |

### `mtk` — MediaTek Filogic / MT798x (aarch64)

| Model | Typical SoC | Sockrocket installation package |
|-------|-------------|---------------------------------|
| TUF-AX4200Q | MT7986A | `sockrocket-merlin-mtk.tar.gz` |
| TUF-AX4200 | MT7986 | `sockrocket-merlin-mtk.tar.gz` |
| TUF-AX6000 | MT7986 | `sockrocket-merlin-mtk.tar.gz` |
| RT-AX59U | MT7986 | `sockrocket-merlin-mtk.tar.gz` |


Newer Filogic SKUs: if softcenter rejects the offline package, check which `.valid` tag your firmware’s software center expects (some forks use custom tags).

### `ipq32` / `ipq64` — IPQ53xx

| Platform | Use when |
|----------|----------|
| `ipq32` | 32-bit ARM userspace on an IPQ53xx board |
| `ipq64` | 64-bit AArch64 userspace on an IPQ53xx board |

ZenWiFi BD4 (IPQ5322, 32-bit firmware): **`sockrocket-merlin-ipq32.tar.gz`**. TUF-BE6500 (IPQ5322, 64-bit firmware): **`sockrocket-merlin-ipq64.tar.gz`**.

Match the exact model, hardware revision and firmware userspace. A 64-bit kernel can run 32-bit userspace.

---

## Install

Preferred path for koolcenter / softcenter users is **offline install in the Software Center**. SSH is optional.

### 1. Download

From [GitHub Releases](https://github.com/sockrockets/sockrocket/releases), get `sockrocket-merlin-<platform>.tar.gz` for your row in the tables above. Checksums: `SHA256SUMS-merlin.txt`.

### 2. Software Center (recommended)

1. Open the router Web UI → **Software Center** (koolcenter / softcenter).
2. Use **Offline install** / upload package.
3. Select the matching `sockrocket-merlin-*.tar.gz` → install.
4. Open the **Sockrocket** tile. To open the same page directly, visit `/Module_sockrocket.asp` on the router’s management origin. On standard Merlin, use `/ext/sockrocket/sockrocket.asp`. Both paths are case-sensitive.

Wrong platform `.valid` → softcenter rejects the package or the binary fails with *Exec format error*. Re-check `uname -m` and the model tables above (`hnd` vs `hnd_v8` is the usual mistake).

Archive root is always `sockrocket/` (koolcenter module name).

### 3. SSH (optional)

```bash
# From your PC (replace host / package name)
scp sockrocket-merlin-hnd_v8.tar.gz admin@<router-lan-ip>:/tmp/

ssh admin@<router-lan-ip>
cd /tmp
tar -xzf sockrocket-merlin-hnd_v8.tar.gz
sh sockrocket/install.sh            # auto-detect, or:
sh sockrocket/install.sh hnd_v8     # force platform tag
```

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
    priority: 50
  - rule_type: "geoip"
    pattern: "CN"          # ~7456 built-in China IPv4 CIDRs
    target: "direct"
    priority: 40
  - rule_type: "dst-port"  # optional
    pattern: "443"
    target: "direct"
    priority: 30
  - rule_type: "final"
    pattern: "*"
    target: "proxy"
    priority: 0
```

Higher `priority` wins first; equal values keep list order. Or use the Web UI
(`Routing rules` page): set priority, ↑↓ reorder, types including `dst-port`.

Web UI: `http://<router-lan-ip>/ext/sockrocket/sockrocket.asp`

Node groups and pinned nodes apply only in rule mode. Create the group on the Nodes page, then set a routing rule's exit to that group. See [Routing](./routing.md).

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

Software Center → Sockrocket → uninstall, **or**:

```bash
sh /jffs/addons/sockrocket/uninstall.sh --force
# softcenter entry point (installed by install.sh):
#   /koolshare/scripts/uninstall_sockrocket.sh
```

Uninstall stops the daemon/API, removes iptables/DNS hooks, deletes
`/jffs/addons/sockrocket`, softcenter files, and all `sockrocket*` dbus keys.
If softcenter ever says it could not find the plugin uninstall script, run the
path above manually — otherwise only registration is cleared and files remain.

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
- [fw.koolcenter.com](https://fw.koolcenter.com/) — Merlin and 官改 firmware that include Software Center  

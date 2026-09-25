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

**Sockrocket** 是 Rust 代理客户端：原生 GUI、CLI，以及一等公民的 **华硕梅林（AsusWRT-Merlin）** 插件。从电脑到整网 LAN，同一套栈。

---

## 为什么选 Sockrocket

### 优势

- **桌面与路由器，同一协议集** — Shadowsocks、VMess、VLess、Trojan、TUIC、Hysteria2（含 Reality、WebSocket、gRPC、ShadowTLS 等）。GUI 能用的，梅林也能用。
- **真实经节点测速** — 测的是出口侧暖路径 RTT，不是对 `server:port` 的假 TCP ping，排序更接近上网体感。
- **订阅开箱即用** — Clash / V2Ray / SingBox 链接，自动识别格式，更新与去重。
- **分流好用** — 域名 / GeoIP / CIDR；**系统代理**覆盖常见应用；**TUN** 兜底不走系统代理的程序。
- **梅林不靠一堆杂二进制** — 单个 musl `sockrocket-cli` + Web UI + iptables/DNS + 看门狗；切换节点热重载，监听端口不拆。
- **轻量、开源、隐私** — Apache-2.0；无账号；无遥测；EN / VI / 中文界面。

### 和同类产品对比

对比看的是**适不适合你**，不是「全面碾压」。功能迭代快，请以各项目当前版本为准。

| | Sockrocket | Clash / Meta 系 | sing-box | 梅林插件（如 fancyss / passwall 类） |
|--|------------|-----------------|----------|--------------------------------------|
| **主交互** | 原生 GUI + CLI + 梅林 Web UI | 配置 / 各类面板客户端 | CLI / 第三方 UI | 主要是路由器 Web UI |
| **代码形态** | 一套 Rust（GUI、CLI、路由） | 内核 + 众多前端 | 内核 + 生态 | Shell + 多个外部核心 |
| **协议** | SS / VMess / VLess / Trojan / TUIC / Hy2 | 广（取决于核心） | 极广 | 取决于捆绑核心 |
| **延迟测试** | 经节点暖路径 HTTP 探测 | 常见 TCP 或混用 | 取决于 UI | 常见 TCP / 脚本 ping |
| **路由器** | 官方梅林包，与桌面同引擎 | 多靠脚本跑核心 | 脚本 / 容器 | 成熟，多为多核心方案 |
| **整网代理** | 梅林 TUN + DNS 劫持 | 需在路由安装核心 | 同左 | 是（主战场） |
| **许可 / 模式** | Apache-2.0，无 SaaS | 开源核心；UI 不一 | 开源 | 多为社区开源 |
| **更适合** | 既要 PC **又要** 梅林整网 | 已深度使用 Clash YAML | 要最大协议面 | 只关心路由器 |

**选 Sockrocket**：日常桌面 + 华硕梅林整网，希望一套维护、测速可信。  
**选 Clash Meta / sing-box**：需要 Sockrocket 尚未覆盖的生态或规则方言。  
**选传统梅林套件**：只跑路由，且已依赖其 UI/脚本生态。

---

## 下载（桌面）

[GitHub Releases](https://github.com/sockrockets/sockrocket/releases)：

| 平台 | GUI | CLI |
|------|-----|-----|
| Linux x86_64 | `sockrocket-linux-x86_64.tar.gz` | `sockrocket-cli-linux-x86_64` |
| Linux aarch64 | `sockrocket-linux-aarch64.tar.gz` | `sockrocket-cli-linux-aarch64` |
| macOS Intel | `sockrocket-macos-x86_64.dmg` | `sockrocket-cli-macos-x86_64` |
| macOS Apple Silicon | `sockrocket-macos-aarch64.dmg` | `sockrocket-cli-macos-aarch64` |
| Windows x86_64 | `sockrocket-windows-x86_64.zip` | `sockrocket-cli-windows-x86_64.exe` |

```bash
# macOS — 打开 DMG，把 Sockrocket 拖到「应用程序」
open sockrocket-macos-aarch64.dmg

# Windows — 解压 ZIP，运行 Sockrocket.exe
# Linux — 解压后安装（开始菜单图标）或直接运行：
tar xzf sockrocket-linux-x86_64.tar.gz
cd Sockrocket-*-linux-x86_64
./install.sh          # 或: ./sockrocket
```

**macOS 提示「无法验证…恶意软件」：** 未配置 Apple 签名/公证时，Gatekeeper 可能拦截。可：

```bash
xattr -cr /Applications/Sockrocket.app
open /Applications/Sockrocket.app
```

或：右键 → **打开** → **打开**；或 系统设置 → 隐私与安全性 → **仍要打开**。

默认监听：**SOCKS5** `127.0.0.1:1080` · **HTTP** `127.0.0.1:1087`

---

## 使用 GUI

1. 启动应用。
2. **订阅** — 粘贴 Clash / V2Ray / SingBox 链接 → 更新；或手动加节点。
3. **节点** — **Test** / **Test all**（经节点暖路径延迟），再点选节点。
4. **连接** — 状态显示已连接，应用走本地代理。
5. **设置**（可选）：
   - **系统代理** — 浏览器等跟随系统。
   - **TUN** — 劫持不认系统代理的应用（需管理员权限）。
6. **语言** — 左下角状态栏：EN → VI → 中文（会保存）。

```bash
curl -x socks5://127.0.0.1:1080 https://www.google.com -I
```

建议：

- 选节点前先 **Test all**，数字同为暖路径、可横向比较。
- 切换节点后若关心暖延迟，稍等再测一次。
- 定期更新订阅，失效节点会拖垮测速体验。

---

## 使用 CLI

```bash
sockrocket-cli --init config.yaml
# 编辑 subscriptions: / nodes: / active_node 后：
sockrocket-cli config.yaml
```

`Ctrl+C` 停止。端口与 GUI 相同。

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

字段说明见 [docs/configuration.md](docs/configuration.md)。

---

## 华硕梅林（整网代理）

在路由器上跑 Sockrocket，**手机、电视、IoT、访客 Wi‑Fi** 都能走代理，终端不必装客户端。协议引擎与桌面一致。

### 你能得到什么

- 透明代理（TUN + iptables / 策略路由）
- 可选 DNS 劫持（dnsmasq → Sockrocket DNS），减轻污染与泄漏
- Web UI：节点、订阅、测速、开关、日志
- 看门狗 + 定时更新订阅
- 切换 `active_node` **热重载**（监听不拆）

```
局域网设备 → iptables / TUN → sockrocket-cli
           → 规则（GeoIP、域名、CIDR）→ 代理节点或直连
```

### 环境要求

- AsusWRT-Merlin **388.x+** 推荐  
- 开启 **JFFS** 与 **自定义脚本**（系统管理 → 系统设置）  
- 包平台标签与 SoC 一致（命名风格同 fancyss）

```bash
uname -m
# armv7l  → arm / hnd / qca / ipq32
# aarch64 → hnd_v8 / mtk / ipq64
```

### 平台包

| 平台 | 包名 | CPU | SoC |
|------|------|-----|-----|
| `arm` | `sockrocket-merlin-arm.tar.gz` | armv7sf | BCM4708/4709 |
| `hnd` | `sockrocket-merlin-hnd.tar.gz` | armv7hf | BCM675x |
| `hnd_v8` | `sockrocket-merlin-hnd_v8.tar.gz` | aarch64 | BCM490x / BCM491x |
| `qca` | `sockrocket-merlin-qca.tar.gz` | armv7hf | IPQ807x |
| `mtk` | `sockrocket-merlin-mtk.tar.gz` | aarch64 | MT798x |
| `ipq32` / `ipq64` | `sockrocket-merlin-ipq32\|64.tar.gz` | armv7hf / aarch64 | IPQ53xx |

**注意：** koolshare 的 `fancyss_hnd` 同时覆盖 490x 与 675x；Sockrocket 拆成 `hnd_v8` 与 `hnd`。选错会导致软件中心拒装或 Exec format error。

```bash
uname -m
# armv7l  → arm / hnd / qca / ipq32
# aarch64 → hnd_v8 / mtk / ipq64
```

### 型号 → 平台（常见华硕梅林）

完整表见 **[docs/merlin.md](docs/merlin.md)**。联名版与基础版用同一 platform。

| 平台 | 型号（示例） |
|------|----------------|
| **`hnd_v8`** | RT-AC86U、GT-AC2900、GT-AC5300、RT-AX88U、RT-AX88U_PRO、RAX80、GT-AX11000、GT-AX11000_PRO、RT-AX92U、RT-AX68U、**RT-AX86U**、**RT-AX86U_PRO**、RT-AX86S、GT-AXE11000、GT-AXE16000、GT-AX6000、ZenWiFi Pro XT12 |
| **`hnd`** | TUF-AX3000 / V2、TUF-AX5400、RT-AX58U / V2、RAX50、RT-AX82U / V2、ZenWiFi XT8 / XD4、RT-AX56U / V2、RT-AX57、RT-AX55 |
| **`arm`** | RT-AC68U、RT-AC66U_B1、RT-AC1900P、RT-AC87U、RT-AC88U、RT-AC3100、RT-AC3200、RT-AC5300 |
| **`qca`** | RT-AX89X / RT-AC89X |
| **`mtk`** | TUF-AX4200、TUF-AX6000、RT-AX59U、ZenWiFi BD4 |
| **`ipq32` / `ipq64`** | IPQ53xx — 按 `uname -m`（`armv7l`→ipq32，`aarch64`→ipq64） |

### 安装

**方式 A — 软件中心离线安装（推荐）**

1. 从 [GitHub Releases](https://github.com/sockrockets/sockrocket/releases) 下载对应平台的 `sockrocket-merlin-*.tar.gz`。
2. 路由器 Web UI → **软件中心** → **离线安装** / 上传插件。
3. 选择该 tar.gz 并安装。
4. 打开 **Sockrocket** 图标，或访问 `http://<路由器局域网IP>/ext/sockrocket/sockrocket.asp`。

平台选错（尤其是 `hnd` vs `hnd_v8`）会导致软件中心拒装或 *Exec format error*。以 `uname -m` 与上表为准。

**方式 B — SSH（可选）**

```bash
# 在电脑上（改主机与包名）
scp sockrocket-merlin-hnd_v8.tar.gz admin@<路由器局域网IP>:/tmp/

ssh admin@<路由器局域网IP>
cd /tmp
tar -xzf sockrocket-merlin-hnd_v8.tar.gz
sh sockrocket/install.sh          # 或: sh sockrocket/install.sh hnd_v8
```

压缩包根目录固定为 `sockrocket/`（模块名）。安装会写入二进制、脚本、Web UI，并挂钩 `services-start`。

### 首次配置（Web UI）

1. 打开 `http://<路由器局域网IP>/ext/sockrocket/sockrocket.asp`（或软件中心入口）。
2. **订阅** — 添加 URL → 更新 → 等待节点出现。
3. **节点** — 全量测速 → 选可用节点。
4. 按需开启 **透明代理**、**DNS 劫持**。
5. 用手机连 Wi‑Fi，确认无需本机客户端也能访问外网。

也可编辑 `/jffs/addons/sockrocket/config.yaml`：

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

### 服务控制

```bash
/jffs/addons/sockrocket/scripts/sockrocket.sh status
/jffs/addons/sockrocket/scripts/sockrocket.sh start|stop|restart
/jffs/addons/sockrocket/scripts/sockrocket.sh log
/jffs/addons/sockrocket/scripts/sockrocket.sh update-subs
```

插件会安装每 5 分钟看门狗，以及可选的每日订阅更新 cron。

### 梅林使用建议

- UI 里切节点是 **热重载**；除非换二进制或改监听端口，尽量少整进程重启。
- 节点 **Test** 与桌面一样走经节点暖探测，结果可横向比。
- 留意 JFFS 空间；开详细日志时注意轮转。
- 透明模式依赖 TUN 模块（重启后看门狗 / `ensure-tun` 可辅助拉起）。

### 卸载

```bash
sh /jffs/addons/sockrocket/uninstall.sh
```

### 梅林排障

| 现象 | 排查 |
|------|------|
| 像没代理 | `sockrocket.sh status`；iptables `SOCKROCKET_*`；日志 |
| 重启后失效 | TUN 是否加载？`services-start` 是否含 sockrocket？ |
| Web UI 404 | `/www/ext/sockrocket/sockrocket.asp`、CGI 是否可执行 |
| 没有节点 | 订阅 URL / 更新；`config.yaml` 的 `nodes:` |
| 局域网 DNS 异常 | DNS 劫持开关；`dnsmasq.d/sockrocket.conf`；守护进程挂掉应 fail-open |

更多：[docs/merlin.md](docs/merlin.md) · [docs/merlin-reference.md](docs/merlin-reference.md)。

---

## 社区

- **Telegram**（发布通知）：[t.me/sockrocket](https://t.me/sockrocket)
- **GitHub Discussions**（提问 / 反馈）：[Discussions](https://github.com/sockrockets/sockrocket/discussions)
- **Issues**（缺陷）：[Issues](https://github.com/sockrockets/sockrocket/issues)

请勿在公开讨论中粘贴订阅链接、token 或可识别个人信息。

## 多语言

| 位置 | 切换 |
|------|------|
| README | [English](README.md) · [Tiếng Việt](README.vi.md) · [中文](README.zh.md) |
| GUI | 左下角状态栏 |
| 官网 | 右上角 **EN / VI / RU / 中文** |

CLI 提示为英文。

---

## 更多文档

| 文档 | 用途 |
|------|------|
| [入门指南](docs/getting-started.md) | 首次运行 |
| [配置说明](docs/configuration.md) | 配置字段与分流 |
| [协议说明](docs/protocols.md) | 协议与传输 |
| [梅林](docs/merlin.md) | 路由安装（精简） |
| [梅林参考](docs/merlin-reference.md) | 路由内部细节 |
| [文档索引](docs/README.md) | 全部文档 |

---

## 许可

[Apache License 2.0](LICENSE)

## AI 训练

本项目 **拒绝** 将内容用于 AI / ML 训练。详见 [`robots.txt`](robots.txt)、[`ai.txt`](ai.txt)、[`.well-known/tdmrep.json`](.well-known/tdmrep.json)、[`.aiignore`](.aiignore)。

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

**Sockrocket** là client proxy viết bằng Rust: GUI native, CLI, và plugin **AsusWRT-Merlin** hạng nhất. Một stack từ máy tính đến cả LAN.

---

## Vì sao chọn Sockrocket

### Ưu điểm

- **Desktop và router, cùng bộ giao thức** — Shadowsocks, VMess, VLess, Trojan, TUIC, Hysteria2 (Reality, WebSocket, gRPC, ShadowTLS, …). GUI dùng được thì Merlin cũng dùng được.
- **Đo độ trễ qua proxy thật** — Đo RTT warm phía exit, không phải TCP ping giả tới `server:port`. Thứ hạng gần với cảm giác duyệt web.
- **Subscription sẵn dùng** — URL Clash / V2Ray / SingBox, tự nhận format, cập nhật và khử trùng.
- **Định tuyến thực tế** — Domain / GeoIP / CIDR; **system proxy** cho app thường; **TUN** khi app bỏ qua proxy hệ thống.
- **Merlin không cần “vườn thú” binary** — Một `sockrocket-cli` musl + Web UI + iptables/DNS + watchdog. Đổi node hot-reload, không gỡ listener.
- **Nhẹ, mở, riêng tư** — Apache-2.0; không tài khoản; không telemetry; UI EN / VI / 中文.

### So với công cụ tương tự

So sánh theo **độ phù hợp**, không phải “thắng mọi mặt”. Tính năng đổi nhanh — hãy đối chiếu bản release hiện tại.

| | Sockrocket | Họ Clash / Meta | sing-box | Plugin Merlin (fancyss / passwall-style) |
|--|------------|-----------------|----------|------------------------------------------|
| **UX chính** | GUI + CLI + Web UI Merlin | Config / dashboard đa dạng | CLI / UI bên thứ ba | Chủ yếu Web UI router |
| **Mã nguồn** | Một cây Rust (GUI, CLI, router) | Core + nhiều frontend | Core + hệ sinh thái | Shell + nhiều binary ngoài |
| **Giao thức** | SS / VMess / VLess / Trojan / TUIC / Hy2 | Rộng (tùy core) | Rất rộng | Tùy core đóng gói |
| **Đo latency** | Probe HTTP warm qua proxy | Thường TCP hoặc lẫn | Tùy UI | Thường TCP / script ping |
| **Router** | Gói Merlin chính thức, cùng engine | Thường chạy core bằng script | Script / container | Trưởng thành, đa core |
| **Proxy cả LAN** | TUN Merlin + DNS hijack | Cài core trên router | Tương tự | Có (trọng tâm) |
| **Giấy phép** | Apache-2.0, không SaaS | Core mở; UI khác nhau | Mở | Chủ yếu cộng đồng |
| **Hợp khi** | Cần PC **và** Merlin | Đã quen Clash YAML | Cần bề mặt giao thức tối đa | Chỉ quan tâm router |

**Chọn Sockrocket** nếu bạn muốn một sản phẩm cho desktop hằng ngày *và* proxy LAN trên Asus Merlin, với đo node trung thực.  
**Chọn Clash Meta / sing-box** nếu cần tính năng / dialect quy tắc Sockrocket chưa có.  
**Chọn bộ Merlin cổ điển** nếu chỉ chạy router và đã phụ thuộc UI/script của họ.

---

## Tải xuống (desktop)

[GitHub Releases](https://github.com/sockrockets/sockrocket/releases):

| Nền tảng | GUI | CLI |
|----------|-----|-----|
| Linux x86_64 | `sockrocket-linux-x86_64.tar.gz` | `sockrocket-cli-linux-x86_64` |
| Linux aarch64 | `sockrocket-linux-aarch64.tar.gz` | `sockrocket-cli-linux-aarch64` |
| macOS Intel | `sockrocket-macos-x86_64.dmg` | `sockrocket-cli-macos-x86_64` |
| macOS Apple Silicon | `sockrocket-macos-aarch64.dmg` | `sockrocket-cli-macos-aarch64` |
| Windows x86_64 | `sockrocket-windows-x86_64.zip` | `sockrocket-cli-windows-x86_64.exe` |

```bash
# macOS — mở DMG, kéo Sockrocket vào Applications
open sockrocket-macos-aarch64.dmg

# Windows — giải nén ZIP, chạy Sockrocket.exe
# Linux — giải nén, cài (icon menu) hoặc chạy portable:
tar xzf sockrocket-linux-x86_64.tar.gz
cd Sockrocket-*-linux-x86_64
./install.sh          # hoặc: ./sockrocket
```

**macOS Gatekeeper:** Bản phát hành chưa notarize trừ khi đã cấu hình secrets ký. Nếu bị chặn:

```bash
xattr -cr /Applications/Sockrocket.app
open /Applications/Sockrocket.app
```

Hoặc: chuột phải → **Open** → **Open**. Hoặc System Settings → Privacy & Security → **Open Anyway**.

Cổng mặc định: **SOCKS5** `127.0.0.1:1080` · **HTTP** `127.0.0.1:1087`

---

## Dùng GUI

1. Khởi động ứng dụng.
2. **Subscriptions** — dán URL Clash / V2Ray / SingBox → Update. Hoặc thêm nút thủ công.
3. **Nodes** — **Test** / **Test all** (độ trễ warm qua proxy), rồi chọn nút.
4. **Connect** — trạng thái đã kết nối; app dùng proxy cục bộ.
5. **Settings** (tùy chọn):
   - **System Proxy** — proxy cấp OS cho trình duyệt / hầu hết app.
   - **TUN** — bắt traffic app bỏ qua system proxy (cần quyền admin).
6. **Ngôn ngữ** — góc dưới trái: EN → VI → 中文 (được lưu).

```bash
curl -x socks5://127.0.0.1:1080 https://www.google.com -I
```

Gợi ý:

- Nên **Test all** trước khi chọn nút; số liệu cùng kiểu warm, so sánh được.
- Sau khi đổi nút, đợi một chút rồi đo lại nếu cần số warm.
- Cập nhật subscription thường xuyên; nút chết làm probe chậm / fail.

---

## Dùng CLI

```bash
sockrocket-cli --init config.yaml
# Sửa subscriptions: / nodes: / active_node, rồi:
sockrocket-cli config.yaml
```

Dừng bằng `Ctrl+C`. Cùng cổng với GUI.

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

Tham chiếu đầy đủ: [docs/configuration.md](docs/configuration.md).

---

## AsusWRT-Merlin (proxy cả LAN)

Chạy Sockrocket trên router để **điện thoại, TV, IoT, Wi‑Fi khách** đi proxy — không cần cài client từng máy. Cùng engine giao thức với desktop.

### Bạn nhận được gì

- Transparent proxy (TUN + iptables / policy routing)
- DNS hijack tùy chọn (dnsmasq → Sockrocket DNS) giảm nhiễm / rò
- Web UI: nút, subscription, đo latency, toggle, log
- Watchdog + cron cập nhật subscription
- Hot-reload khi đổi `active_node` (listener giữ nguyên)

```
Thiết bị LAN → iptables / TUN → sockrocket-cli
             → quy tắc (GeoIP, domain, CIDR) → nút proxy hoặc direct
```

### Yêu cầu

- AsusWRT-Merlin **388.x+** khuyến nghị  
- Bật **JFFS** + **custom scripts** (Administration → System)  
- Tag gói khớp SoC (cách đặt tên giống fancyss)

```bash
uname -m
# armv7l  → arm / hnd / qca / ipq32
# aarch64 → hnd_v8 / mtk / ipq64
```

### Bảng gói nền tảng

| Nền tảng | Gói | CPU | SoC |
|----------|-----|-----|-----|
| `arm` | `sockrocket-merlin-arm.tar.gz` | armv7sf | BCM4708/4709 |
| `hnd` | `sockrocket-merlin-hnd.tar.gz` | armv7hf | BCM675x |
| `hnd_v8` | `sockrocket-merlin-hnd_v8.tar.gz` | aarch64 | BCM490x / BCM491x |
| `qca` | `sockrocket-merlin-qca.tar.gz` | armv7hf | IPQ807x |
| `mtk` | `sockrocket-merlin-mtk.tar.gz` | aarch64 | MT798x |
| `ipq32` / `ipq64` | `sockrocket-merlin-ipq32\|64.tar.gz` | armv7hf / aarch64 | IPQ53xx |

**Lưu ý:** `fancyss_hnd` của koolshare gộp cả 490x và 675x; Sockrocket tách thành `hnd_v8` và `hnd`. Chọn sai → softcenter từ chối hoặc Exec format error.

```bash
uname -m
# armv7l  → arm / hnd / qca / ipq32
# aarch64 → hnd_v8 / mtk / ipq64
```

### Model → platform (Asus Merlin phổ biến)

Bảng đầy đủ: **[docs/merlin.md](docs/merlin.md)**. Bản collab dùng cùng platform với model gốc.

| Platform | Model (ví dụ) |
|----------|----------------|
| **`hnd_v8`** | RT-AC86U, GT-AC2900, GT-AC5300, RT-AX88U, RT-AX88U_PRO, RAX80, GT-AX11000, GT-AX11000_PRO, RT-AX92U, RT-AX68U, **RT-AX86U**, **RT-AX86U_PRO**, RT-AX86S, GT-AXE11000, GT-AXE16000, GT-AX6000, ZenWiFi Pro XT12 |
| **`hnd`** | TUF-AX3000 / V2, TUF-AX5400, RT-AX58U / V2, RAX50, RT-AX82U / V2, ZenWiFi XT8 / XD4, RT-AX56U / V2, RT-AX57, RT-AX55 |
| **`arm`** | RT-AC68U, RT-AC66U_B1, RT-AC1900P, RT-AC87U, RT-AC88U, RT-AC3100, RT-AC3200, RT-AC5300 |
| **`qca`** | RT-AX89X / RT-AC89X |
| **`mtk`** | TUF-AX4200, TUF-AX6000, RT-AX59U, ZenWiFi BD4 |
| **`ipq32` / `ipq64`** | IPQ53xx — theo `uname -m` (`armv7l`→ipq32, `aarch64`→ipq64) |

### Cài đặt

**Cách A — Cài offline trên Software Center (khuyến nghị)**

1. Tải `sockrocket-merlin-*.tar.gz` khớp nền tảng từ [GitHub Releases](https://github.com/sockrockets/sockrocket/releases).
2. Web UI router → **Software Center** → **Offline install** / tải gói lên.
3. Chọn tar.gz và cài.
4. Mở ô **Sockrocket**, hoặc `http://<ip-lan-router>/ext/sockrocket/sockrocket.asp`.

Sai platform (nhất là `hnd` vs `hnd_v8`) → softcenter từ chối hoặc *Exec format error*. Tin `uname -m` và bảng trên.

**Cách B — SSH (tuỳ chọn)**

```bash
# Từ máy tính (đổi host / tên gói)
scp sockrocket-merlin-hnd_v8.tar.gz admin@<ip-lan-router>:/tmp/

ssh admin@<ip-lan-router>
cd /tmp
tar -xzf sockrocket-merlin-hnd_v8.tar.gz
sh sockrocket/install.sh          # hoặc: sh sockrocket/install.sh hnd_v8
```

Thư mục gốc archive luôn là `sockrocket/` (tên module). Install chép binary, script, Web UI và móc `services-start`.

### Thiết lập lần đầu (Web UI)

1. Mở `http://<ip-lan-router>/ext/sockrocket/sockrocket.asp` (hoặc mục softcenter).
2. **Subscriptions** — thêm URL → update → đợi nút.
3. **Nodes** — Test all → chọn nút ổn.
4. Bật **transparent proxy** và/hoặc **DNS hijack** tùy nhu cầu.
5. Thử điện thoại Wi‑Fi: không cần client vẫn ra được mạng ngoài.

Hoặc sửa `/jffs/addons/sockrocket/config.yaml`:

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

### Điều khiển dịch vụ

```bash
/jffs/addons/sockrocket/scripts/sockrocket.sh status
/jffs/addons/sockrocket/scripts/sockrocket.sh start|stop|restart
/jffs/addons/sockrocket/scripts/sockrocket.sh log
/jffs/addons/sockrocket/scripts/sockrocket.sh update-subs
```

Watchdog (mỗi 5 phút) và cron subscription hàng ngày được cài cùng plugin.

### Mẹo Merlin

- Đổi nút trên UI là **hot-reload**; tránh restart đầy đủ trừ khi đổi binary / cổng listen.
- **Test** nút dùng cùng probe warm qua proxy như desktop — so sánh ngang hàng.
- Giữ dung lượng JFFS; bật log verbose thì chú ý xoay log.
- Chế độ transparent cần module TUN (`ensure-tun` / watchdog hỗ trợ sau reboot).

### Gỡ cài

```bash
sh /jffs/addons/sockrocket/uninstall.sh
```

### Xử lý sự cố Merlin

| Hiện tượng | Kiểm tra |
|------------|----------|
| Không như có proxy | `sockrocket.sh status`; chuỗi iptables `SOCKROCKET_*`; log |
| Chết sau reboot | TUN đã load? `services-start` có sockrocket? |
| Web UI 404 | `/www/ext/sockrocket/sockrocket.asp`, CGI executable |
| Không có nút | URL subscription / update; `nodes:` trong `config.yaml` |
| DNS LAN lỗi | Toggle DNS hijack; `dnsmasq.d/sockrocket.conf`; fail-open khi daemon chết |

Chi tiết: [docs/merlin.md](docs/merlin.md) · [docs/merlin-reference.md](docs/merlin-reference.md).

---

## Cộng đồng

- **Telegram** (thông báo): [t.me/sockrocket](https://t.me/sockrocket)
- **GitHub Discussions** (hỏi đáp / góp ý): [Discussions](https://github.com/sockrockets/sockrocket/discussions)
- **Issues** (lỗi): [Issues](https://github.com/sockrockets/sockrocket/issues)

Không đăng link subscription, token, hoặc thông tin nhận dạng cá nhân trên kênh công khai.

## Ngôn ngữ

| Nơi | Cách chuyển |
|-----|-------------|
| README | [English](README.md) · [Tiếng Việt](README.vi.md) · [中文](README.zh.md) |
| GUI | Góc dưới trái thanh trạng thái |
| Website | Góc trên phải **EN / VI / RU / 中文** |

Thông báo CLI bằng tiếng Anh.

---

## Tài liệu thêm

| Tài liệu | Dùng khi |
|----------|----------|
| [Bắt đầu](docs/getting-started.md) | Lần chạy đầu |
| [Cấu hình](docs/configuration.md) | Trường config & định tuyến |
| [Giao thức](docs/protocols.md) | Protocol / transport |
| [Merlin](docs/merlin.md) | Cài router (rút gọn) |
| [Merlin reference](docs/merlin-reference.md) | Nội bộ router |
| [Mục lục docs](docs/README.md) | Toàn bộ |

---

## Giấy phép

[Apache License 2.0](LICENSE)

## Huấn luyện AI

Dự án này **từ chối** dùng làm dữ liệu huấn luyện AI / ML. Xem [`robots.txt`](robots.txt), [`ai.txt`](ai.txt), [`.well-known/tdmrep.json`](.well-known/tdmrep.json), [`.aiignore`](.aiignore).

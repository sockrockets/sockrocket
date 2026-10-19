# Architecture

## Stack

| Area | Choice | Notes |
|------|--------|-------|
| GUI | gpui + gpui-component | GPU UI, cross-platform |
| Async | tokio | Network I/O |
| Shadowsocks | shadowsocks crate | AEAD ciphers |
| VMess / VLESS | In-tree | AEAD, TLS/Reality transports |
| TUIC v5 | quinn + protocol layer | QUIC |
| TLS | rustls / craftls | No OpenSSL for TCP paths |
| Config | serde_yaml / serde_json | Clash, V2Ray, subscriptions |
| Subscriptions | reqwest | HTTP fetch |
| System proxy | sysproxy-rs | User-level OS proxy |

## Crate layout

```
sockrocket/
├── sockrocket-core/
│   ├── config/       # model, clash, v2ray, subscription, watch
│   ├── proxy/        # socks5, http, outbounds, tun, transport
│   ├── router/       # rules, geoip
│   └── dns/          # resolver, fake-ip
├── sockrocket-gui/   # GPUI app
└── sockrocket-cli/   # Headless / router daemon
```

## Layer diagram

```
┌──────────────── UI ────────────────┐
│  sockrocket-gui    sockrocket-cli  │
└──────────────┬─────────────────────┘
               ▼
┌────────── sockrocket-core ─────────┐
│ Config │ Local SOCKS5/HTTP │ Router│
│        └──── Outbound factory ────┤
│              SS VMess VLESS TUIC Hy2│
│              Transport: TLS WS QUIC │
└────────────────────────────────────┘
               ▼
         Remote proxy nodes
```

## Design notes

### No admin by default

SOCKS5/HTTP on `127.0.0.1` plus optional system proxy. TUN mode needs elevated privileges and is opt-in.

### Upstream compatibility

Client targets common server stacks: VMess AEAD, VLESS (Vision), Shadowsocks (incl. 2022), TUIC v5, Trojan, Hysteria2; transports TCP, TLS, WebSocket, QUIC, Reality, ShadowTLS.

### Config import

| Format | Detection |
|--------|-----------|
| Clash YAML | `proxies` |
| V2Ray JSON | `outbounds` |
| Base64 lines | URI per line |
| Single URIs | `ss://`, `vmess://`, etc. |

### Platforms

GPUI: Windows, macOS, Linux. CLI: servers and Merlin routers. rustls reduces native TLS deps.

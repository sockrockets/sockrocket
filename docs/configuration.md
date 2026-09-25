# Configuration reference

> New users should start with [Getting started](./getting-started.md).

## Internal YAML format

All imported configs normalize to Sockrocket’s internal YAML schema.

```yaml
listen_addr: "127.0.0.1"
socks_port: 1080
http_port: 1087
active_node: 0

subscriptions:
  - name: "My Subscription"
    url: "https://example.com/subscribe"
    format: auto

nodes:
  - name: "HK-SS"
    server: "proxy.example.com"
    port: 8388
    protocol:
      type: shadowsocks
      cipher: aes-256-gcm
      password: "your-password"
      udp: true

  - name: "JP-VMess"
    server: "proxy.example.com"
    port: 443
    protocol:
      type: vmess
      uuid: "b0e80a62-8a51-47f0-91f1-f0f7faf8d9d4"
      alter_id: 0
      cipher: auto
      udp: true
    transport:
      type: tls
      tls:
        sni: "example.com"
        alpn: ["h2", "http/1.1"]

rules:
  - rule_type: domain-suffix
    pattern: "google.com"
    target: proxy
  - rule_type: ip-cidr
    pattern: "192.168.0.0/16"
    target: direct
  - rule_type: geoip
    pattern: CN
    target: direct
```

---

## Import formats

### Clash YAML

Sockrocket reads the `proxies` list. Clash `type` maps to `protocol.type`:

| Clash | Sockrocket |
|-------|------------|
| `ss` | `shadowsocks` |
| `vmess` | `vmess` |
| `vless` | `vless` |
| `tuic` | `tuic` |
| `trojan` | `trojan` |
| `hysteria2` | `hysteria2` |

### V2Ray JSON

Standard `outbounds` JSON is parsed into nodes.

### Base64 subscription (V2Ray / generic)

1. HTTP GET subscription URL  
2. Base64-decode body  
3. Split lines; each line is a URI  

Supported prefixes: `ss://`, `vmess://`, `vless://`, `tuic://`, `trojan://`, `hysteria2://` / `hy2://`

### Auto detection (`format: auto`)

1. YAML with `proxies` → Clash  
2. JSON with `outbounds` → V2Ray  
3. Base64 with `://` → line URIs  
4. Otherwise try plain URI lines  

---

## Default config paths

| Platform | Path |
|----------|------|
| Windows | `%APPDATA%\sockrocket\config.yaml` |
| macOS | `~/Library/Application Support/sockrocket/config.yaml` |
| Linux | `~/.config/sockrocket/config.yaml` |

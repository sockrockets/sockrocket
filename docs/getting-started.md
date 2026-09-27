# Getting started

## Install

### Pre-built binaries

Download from [GitHub Releases](https://github.com/sockrockets/sockrocket/releases):

| Platform | CLI | GUI |
|----------|-----|-----|
| Linux x86_64 | `sockrocket-cli-linux-x86_64` | `sockrocket-linux-x86_64.tar.gz` |
| Linux aarch64 | `sockrocket-cli-linux-aarch64` | `sockrocket-linux-aarch64.tar.gz` |
| macOS x86_64 | `sockrocket-cli-macos-x86_64` | `sockrocket-macos-x86_64.dmg` |
| macOS arm64 | `sockrocket-cli-macos-aarch64` | `sockrocket-macos-aarch64.dmg` |
| Windows x86_64 | `sockrocket-cli-windows-x86_64.exe` | `sockrocket-windows-x86_64.zip` |

---

## Configuration

### Generate a default config

```bash
sockrocket-cli --init config.yaml
```

### Minimal example

```yaml
listen_addr: "127.0.0.1"
socks_port: 1080
http_port: 1087

subscriptions:
  - name: "My subscription"
    url: "https://example.com/subscribe?token=xxx"
    format: "auto"

nodes:
  - name: "My-SS"
    server: "your-server.com"
    port: 8388
    protocol:
      type: shadowsocks
      cipher: aes-256-gcm
      password: "your-password"

active_node: 0
```

---

## Run

### CLI

```bash
sockrocket-cli config.yaml
```

Listens on:

- **SOCKS5** — `127.0.0.1:1080`
- **HTTP proxy** — `127.0.0.1:1087`

Stop with `Ctrl+C`.

### GUI

```bash
sockrocket-gui
```

1. **Config** — paste a subscription URL and **Import**
2. **Nodes** — pick a node
3. **Home** — **Connect**

---

## Verify connectivity

```bash
curl -x socks5://127.0.0.1:1080 https://www.google.com -I
curl -x http://127.0.0.1:1087 https://www.google.com -I
```

---

## Next steps

- [Configuration](./configuration.md)
- [Protocols](./protocols.md)
- [Development](./development.md)

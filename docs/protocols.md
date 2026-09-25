# Protocol support

## Proxy protocols

### Shadowsocks

Lightweight AEAD proxy. Ciphers include `aes-128-gcm`, `aes-256-gcm`, `chacha20-ietf-poly1305`, and SS2022 variants.

URI:

```
ss://aes-256-gcm:password@proxy.example.com:8388#MyServer
```

Implementation: `shadowsocks` crate.

### VMess

V2Ray AEAD protocol. Ciphers: `aes-128-gcm`, `chacha20-poly1305`, `none` (use with TLS).

Share links use Base64 JSON in `vmess://` URIs. Implemented in-tree (AEAD framing).

### VLESS

Lightweight UUID auth; encryption usually from TLS or Reality.

```
vless://uuid@host:443?encryption=none&security=tls&sni=example.com&type=ws&path=/path#name
```

Common query keys: `security` (`tls`, `reality`), `type` (`tcp`, `ws`, `grpc`), `flow` (`xtls-rprx-vision`).

### TUIC v5

QUIC-based; 0-RTT, UDP relay, migration.

```
tuic://uuid:password@host:443?congestion_control=bbr&alpn=h3#name
```

Implementation: `quinn` + TUIC framing.

### Trojan

Password over TLS mimicking HTTPS.

```
trojan://password@host:443?sni=example.com&type=tcp#name
```

### Hysteria2

QUIC-based, tuned for lossy links.

```
hysteria2://password@host:443?sni=example.com#name
```

---

## Transports

| Transport | Used by |
|-----------|---------|
| TCP | All |
| TLS | VMess, VLESS, Trojan, SS |
| WebSocket | VMess, VLESS, SS |
| QUIC | TUIC, Hysteria2 |
| Reality | VLESS |
| Vision | VLESS flow control |

## Implementation priority (historical)

| Protocol | Priority |
|----------|----------|
| Shadowsocks, VMess, VLESS | P0 |
| TUIC, Trojan | P1 |
| Hysteria2 | P2 |

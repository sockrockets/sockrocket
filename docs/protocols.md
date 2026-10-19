# Protocol support

Sockrocket uses one native Rust core shared by the desktop client, CLI and Merlin plugin. No external proxy executable is required at runtime.

## Protocols

| Protocol | TCP | UDP | Notes |
| --- | --- | --- | --- |
| Shadowsocks | Yes | Yes | AEAD and SS2022; ShadowTLS wrapping is TCP only |
| VMess | Yes | Yes | AEAD only (`alterId: 0`); native UDP has a per-packet size limit |
| VLESS | Yes | Yes | Raw TCP, TLS, WebSocket or REALITY; nonempty `flow` is rejected |
| Trojan | Yes | Yes | TLS by default |
| TUIC v5 | Yes | Yes | Native QUIC |
| Hysteria2 | Yes | Yes | Native QUIC; obfuscation, certificate pinning and port hopping are not implemented |
| AnyTLS v2 | Yes | Yes | Native TLS sessions with bounded reuse; UDP-over-TCP v2 |
| WireGuard | Yes | Yes | Native userspace, one peer per node; UDP payload must fit tunnel MTU |

UDP availability also depends on the node's `udp` option. Certificate verification is enabled by default. TLS, WebSocket and REALITY are transports, not separate VPN protocols.

## AnyTLS

Import `anytls://` links, Clash YAML or sing-box JSON. TLS options include SNI, ALPN and an explicit insecure override. Passwords are decoded once and may contain escaped punctuation.

```text
anytls://EXAMPLE_PASSWORD@vpn.example.invalid:443?sni=tls.example.invalid#Example
```

The implementation authenticates using the standard v2 wire format, negotiates server settings and SYN acknowledgements, isolates server padding policies, and reuses a bounded set of idle TLS sessions. Sessions have increasing stream IDs, bounded packet queues and idle expiry. UDP uses connected UoT v2 framing and preserves datagram boundaries. One-shot latency probes do not retain idle sessions.

## WireGuard

Import a single-peer WireGuard profile, Clash node, sing-box outbound/endpoint, or Sockrocket's `wireguard://` share format. Desktop import and Merlin's manual node form use the same validated model. Credentials and subscription data use the existing private configuration storage.

Required fields are the private key, peer public key, peer host/port and one local CIDR address per required IP family. Optional fields include a preshared key, allowed IP networks, tunnel DNS IPs, MTU and persistent keepalive. The default MTU is 1280. Target-host DNS requests travel inside the encrypted tunnel; only the peer hostname uses bootstrap resolution.

WireGuard runs inside the process using BoringTun and smoltcp. It creates no host interface and changes no host routes, DNS settings or system proxy preferences. The outgoing UDP socket uses the application's physical-interface binding to avoid routing the tunnel into itself. The last flow releases the userspace endpoint and its polling task.

Limits: one peer per node, at most two local addresses (one per family), four DNS IPs, 64 allowed networks, eight active endpoints per process and 128 admitted flows per endpoint. IP fragmentation is not implemented: outgoing UDP payloads above `MTU - 28` for IPv4 or `MTU - 48` for IPv6 return an explicit error. Nonstandard reserved bytes, AmneziaWG, multi-peer profiles, fixed local listen ports, OS interface requests and profile hooks are rejected. Shell hooks are never executed. `wireguard://` is an application share format rather than a universal WireGuard standard.

## Transport boundaries and remaining work

Raw TCP, TLS, WebSocket and VLESS REALITY are implemented. Requested gRPC, XHTTP, HTTPUpgrade and other unsupported transports are rejected during conversion; they are never silently changed to raw TCP. VLESS Vision is not implemented: a node requesting `xtls-rprx-vision` fails explicitly instead of connecting without the requested flow. Legacy VMess with nonzero `alterId` is also rejected.

Further protocol work must retain the native Rust core and pass independent interoperability tests before being listed as supported. Current remaining candidates include VLESS Vision, gRPC/XHTTP transports, Hysteria2 obfuscation and broader WireGuard profile compatibility.

## Validation

Synthetic regressions cover URI/YAML/JSON conversion, persistence, malformed options, TLS settings, stream framing, UDP boundaries, concurrent flows and resource release. Optional independent interoperability tests use a checksum-verified official sing-box executable only as a loopback test server; it is not shipped or invoked by the product:

```sh
SOCKROCKET_REFERENCE_SING_BOX=/path/to/verified/sing-box \
  cargo test -p sockrocket-core --lib reference_ -- --ignored
```

Tests do not require personal subscriptions, node credentials or changes to host routes. AnyTLS TCP/UDP and WireGuard concurrent TCP/UDP are verified against the independent server. Zero-length WireGuard UDP is tested between native endpoints separately because the reference server does not relay empty UDP payloads.

References: [AnyTLS protocol](https://github.com/anytls/anytls-go/blob/main/docs/protocol.md), [UoT v2](https://sing-box.sagernet.org/configuration/shared/udp-over-tcp/), [WireGuard endpoint format](https://sing-box.sagernet.org/configuration/endpoint/wireguard/), [BoringTun](https://github.com/cloudflare/boringtun).

TUIC share links accept `insecure`, `allow_insecure` and `allowInsecure` aliases. A `pcs` pin is a SHA-256 hash of the leaf certificate in hexadecimal (optional colon/hyphen separators). When present, that exact leaf and its TLS handshake signature must verify, even if insecure mode is also selected. Pins survive local saves, share links and Clash exports. The pin is stored as `extra.tuic_certificate_sha256` in native node configurations.

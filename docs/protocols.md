# Protocol support

Sockrocket uses one native Rust core shared by the desktop client, CLI and Merlin plugin. No external proxy executable is required at runtime.

## Protocols

| Protocol | TCP | UDP | Notes |
| --- | --- | --- | --- |
| Shadowsocks | Yes | Yes | AEAD and SS2022; ShadowTLS wrapping is TCP only |
| VMess | Yes | Yes | AEAD only (`alterId: 0`); native UDP has a per-packet size limit |
| VLESS | Yes | Yes | Raw TCP, TLS, WebSocket, XHTTP (HTTP/1.1–3) or REALITY; Vision over TLS 1.3/REALITY TCP, XUDP (standard flow excludes UDP/443) |
| Trojan | Yes | Yes | TLS by default; REALITY over raw TCP |
| TUIC v5 | Yes | Yes | Native QUIC datagrams, reliable QUIC streams, UDP-over-stream v1/v2 |
| Hysteria2 | Yes | Yes | Native QUIC; certificate pinning, ALPN, bandwidth limits and Fast Open; Salamander, Gecko and UDP destination port hopping |
| AnyTLS v2 | Yes | Yes | Native TLS sessions with bounded reuse; UDP-over-TCP v2 |
| WireGuard | Yes | Yes | Native userspace, up to 64 peers per node; UDP payload must fit tunnel MTU |

UDP availability also depends on the node's `udp` option. Certificate verification is enabled by default. TLS, WebSocket and REALITY are transports, not separate VPN protocols.

## Known outbound parameter gaps

| Protocol | Remaining limits or unverified settings |
| --- | --- |
| Shadowsocks | The current Mihomo plugin importer handles ShadowTLS. Other SIP003 plugin configurations need their own implementation and wire tests. |
| VMess | AES-GCM and ChaCha20-Poly1305 support global padding and authenticated chunk lengths. Legacy nonzero alterId is outstanding. |
| VLESS | Xray VLESS Encryption is outstanding. |
| Trojan | REALITY is limited to raw TCP. sing-box UDP-only outbound policy is enforced. TLS gRPC and HTTPUpgrade carry TCP and UDP in sing-box interoperability tests. Xray TunMulti is also tested. |
| TUIC v5 | Native, reliable QUIC-stream and UDP-over-stream v1/v2 are covered. Heartbeat timing is configurable. 0-RTT remains outstanding. |
| Hysteria2 | Salamander and Gecko are tested against sing-box. Hopping changes the UDP destination port. |
| AnyTLS | Configurable idle check intervals, timeouts, minimum idle counts and disabled reuse are supported. Minimum idle count is bounded by the 64-session per-node capacity. |
| WireGuard | Multi-peer longest-prefix routing, a fixed local listen port and per-peer reserved bytes are supported. AmneziaWG and OS interface hooks are outside the userspace implementation. |

## Local interoperability tests

Run these tests with separately installed reference binaries. They use loopback
listeners, synthetic credentials and local echo services.

```sh
SOCKROCKET_REFERENCE_SING_BOX=/path/to/sing-box \
SOCKROCKET_REFERENCE_MIHOMO=/path/to/mihomo \
SOCKROCKET_REFERENCE_XRAY=/path/to/xray \
cargo test --locked -p sockrocket-core --lib --test reference_protocols reference_ -- --ignored --test-threads=1
```

The sing-box cases cover SS AES-128-GCM, VMess AEAD, Trojan/TLS, TUIC v5,
VLESS Vision, AnyTLS, Hysteria2 and WireGuard. The Xray cases exercise VLESS
XHTTP and Trojan gRPC TunMulti. The cases in `reference_protocols` import a sing-box outbound,
establish a TCP stream and check two UDP request/reply pairs of different lengths.

The HTTPUpgrade client passes raw bytes after a validated HTTP 101. The gRPC
client opens one flow-controlled HTTP/2 connection per stream and supports
`service_name`, `authority` and Xray's `multiMode`. Connection reuse and
gRPC idle-ping configuration are outstanding. VMess padding and authenticated
lengths have independent sing-box TCP and UDP cases, including both settings
together. The Hysteria2 hopping case forwards through two UDP ports and checks
that the same application connection carries traffic through both.

TUIC relay tests run with sing-box and Mihomo. Each mode exchanges TCP
data and both short and 8 KiB UDP payloads. Native TUIC fragments large
datagrams. Reliable TUIC carries each packet on a separate unidirectional
stream. UDP-over-stream uses a bidirectional stream with v1 or v2 framing.
Mihomo YAML without an explicit UDP-over-stream version selects v1, and
sing-box's `udp_over_stream` selects v2. The relay selection survives local
saves, share links and Mihomo YAML exports. The heartbeat timer accepts sing-box
Go-style durations and Mihomo millisecond intervals, with millisecond resolution.
The client sends the TUIC heartbeat command on live datagram-capable connections.

VMess reference cases exercise both TCP and UDP with `auto`, `aes-128-gcm`,
`chacha20-poly1305`, `none` and `zero`. Zero streams forward the TCP payload
directly after the authenticated VMess header and retain packet framing for UDP.
All five modes exchanged TCP and UDP payloads with sing-box 1.14.3 and Mihomo
1.19.31. One initial Mihomo AES run closed early and five full reruns passed.
Xray 26.10.31 accepted auto, AES and ChaCha. Its VMess inbound rejects the
wire security identifier used by both none and zero. See the [Xray security
type definition](https://github.com/XTLS/Xray-core/blob/v26.10.31/common/protocol/headers.proto).

Xray JSON import accepts flat and single-entry legacy VMess, VLESS and
Shadowsocks settings. VMess and VLESS short user IDs map to Xray's UUIDv5.
Xray WireGuard import supports local addresses, peer keys, endpoint,
allowed IPs, persistent keepalive, MTU and DNS. The WireGuard reference case
uses this configuration format with a sing-box peer. Hysteria2 sing-box import
preserves integer `up_mbps` and `down_mbps` through the native bandwidth settings.

## VLESS Vision

Vision is a native Rust implementation; no external core is launched by the application. An independent sing-box reference server test verifies TCP, TLS 1.3 data transfer, half-close, XUDP and cleanup. Outer TLS reads stop at record boundaries to preserve coalesced raw data at a direct-copy transition. The leaf-certificate/authentication policy of the selected TLS or REALITY transport is retained.

Run the opt-in reference test with a separately verified sing-box executable:

```sh
SOCKROCKET_REFERENCE_SING_BOX=/path/to/sing-box cargo test -p sockrocket-core --lib reference_vision -- --ignored
```

Hysteria2 and AnyTLS share links accept `pcs`/`pinSHA256` leaf-certificate SHA-256 pins; a supplied pin is enforced even with `insecure=1`. Pins survive saves, share links and Clash export/import. Unimplemented sing-box TLS security options (including SPKI public-key pins) reject the node instead of weakening its certificate policy.

Hysteria2 also supports `alpn`, `upmbps`, `downmbps` and `fastopen`; see the [Merlin guide](../merlin/README.md#subscription-compatibility) for bandwidth behavior.

## Trojan configuration interoperability

Trojan over REALITY/TCP can be imported from a share URI, Mihomo YAML,
sing-box JSON, or Xray JSON. Xray import accepts flat outbound settings and
the legacy single-entry `servers` array. The formats map to the same native
password, SNI, public key, short ID and client fingerprint. URI passwords are
percent-decoded once on import and encoded on export. A sing-box Trojan
outbound enables both TCP and UDP by default; `network: "tcp"` disables UDP.
A UDP-only Trojan outbound rejects application TCP requests and forwards UDP through its authenticated tunnel. Sockrocket preserves this policy on export through the `network` share parameter and the `sockrocket-udp-only` YAML extension.

The `trojan_import_parity` regression checks subscription format detection,
native configuration equality and share-link round trips. These checks cover
configuration conversion; server handshake and traffic tests are separate.

## AnyTLS

Import `anytls://` links, Clash YAML or sing-box JSON. TLS options include SNI, ALPN and an explicit insecure override. Passwords are decoded once and may contain escaped punctuation.

```text
anytls://EXAMPLE_PASSWORD@vpn.example.invalid:443?sni=tls.example.invalid#Example
```

The implementation authenticates using the standard v2 wire format, negotiates server settings and SYN acknowledgements, isolates server padding policies, and reuses a bounded set of idle TLS sessions. Sessions have increasing stream IDs, bounded packet queues and idle expiry. UDP uses connected UoT v2 framing and preserves datagram boundaries. One-shot latency probes do not retain idle sessions. The idle check interval and
timeout follow the reference client's 30-second default for values up to five
seconds. Cleanup preserves the newest minimum idle sessions without proactively
opening sockets. Mihomo's `disable-reuse` skips session recycling. Native
configuration and share URIs preserve millisecond durations. Mihomo YAML uses
whole seconds and rounds fractional seconds up on export.

## WireGuard

Import a WireGuard profile, Clash node, sing-box outbound/endpoint, or Sockrocket's `wireguard://` share format. Desktop import and Merlin's manual node form use the same validated model. Credentials and subscription data use the existing private configuration storage.

Required fields are the private key, peer public key, peer host/port and one local CIDR address per required IP family. Optional fields include a preshared key, allowed IP networks, tunnel DNS IPs, MTU and persistent keepalive. The default MTU is 1280. Target-host DNS requests travel inside the encrypted tunnel; only the peer hostname uses bootstrap resolution.

WireGuard runs inside the process using BoringTun and smoltcp. It creates no host interface and changes no host routes, DNS settings or system proxy preferences. The outgoing UDP socket uses the application's physical-interface binding to avoid routing the tunnel into itself. The last flow releases the userspace endpoint and its polling task.

Limits: up to 64 peers per node, at most two local addresses (one per family), four DNS IPs, 64 allowed networks per peer, eight active endpoints per process and 128 admitted flows per endpoint. IP fragmentation is not implemented: outgoing UDP payloads above `MTU - 28` for IPv4 or `MTU - 48` for IPv6 return an explicit error. AmneziaWG, OS interface requests and profile hooks are rejected. Shell hooks are never executed. `wireguard://` is an application share format rather than a universal WireGuard standard.

## Transport boundaries and remaining work

Raw TCP, TLS, WebSocket, HTTPUpgrade, gRPC and VLESS REALITY are implemented. Unsupported transports are rejected during conversion; they are never silently changed to raw TCP. VLESS `xtls-rprx-vision` is implemented natively over TLS 1.3 or REALITY/TCP: padding, fragmented frames, independent read/write raw-copy transitions, half-close and XUDP. The standard flow rejects UDP/443 to permit client TCP fallback. Other flows and Vision over WebSocket are rejected; the flow is never silently removed. Legacy VMess with nonzero `alterId` is also rejected.

Further protocol work must retain the native Rust core and pass independent interoperability tests before being listed as supported. Current remaining candidates include gRPC connection reuse and broader WireGuard profile compatibility.

## Validation

Synthetic regressions cover URI/YAML/JSON conversion, persistence, malformed options, TLS settings, stream framing, UDP boundaries, concurrent flows and resource release. Optional independent interoperability tests use a checksum-verified official sing-box executable only as a loopback test server; it is not shipped or invoked by the product:

```sh
SOCKROCKET_REFERENCE_SING_BOX=/path/to/verified/sing-box \
  cargo test -p sockrocket-core --lib reference_ -- --ignored
```

Tests do not require personal subscriptions, node credentials or changes to host routes. AnyTLS TCP/UDP and WireGuard concurrent TCP/UDP are verified against the independent server. Zero-length WireGuard UDP is tested between native endpoints separately because the reference server does not relay empty UDP payloads.

References: [Xray Vision implementation](https://github.com/XTLS/Xray-core/blob/main/proxy/proxy.go), [AnyTLS protocol](https://github.com/anytls/anytls-go/blob/main/docs/protocol.md), [UoT v2](https://sing-box.sagernet.org/configuration/shared/udp-over-tcp/), [WireGuard endpoint format](https://sing-box.sagernet.org/configuration/endpoint/wireguard/), [BoringTun](https://github.com/cloudflare/boringtun).

TUIC share links accept `insecure`, `allow_insecure` and `allowInsecure` aliases. A `pcs` pin is a SHA-256 hash of the leaf certificate in hexadecimal (optional colon/hyphen separators). When present, that exact leaf and its TLS handshake signature must verify, even if insecure mode is also selected. Pins survive local saves, share links and Clash exports. The pin is stored as `extra.tuic_certificate_sha256` in native node configurations.

## QUIC handshake profiles

Hysteria2 `fp=chrome` (alias `fp=chrome115`) enables a native Rust Chrome 115
**baseline** handshake profile with ALPN `h3`. It changes the actual TLS 1.3
cipher/extension order, X25519 key share, Brotli certificate compression,
source/destination connection ID lengths and shuffled QUIC transport parameters
(including GREASE). It preserves certificate verification and leaf pins. Existing
saved Chrome hints are recognized; Clash `client-fingerprint`, enabled sing-box
`tls.utls.fingerprint` and share-link
export/import preserve the choice. Other browser hints remain metadata and are
reported separately; absent/`none` fingerprints use the original QUIC stack.

This profile does **not** reproduce ALPS, Chrome's Initial packet number/framing,
HTTP/3 request behavior, or later Chrome releases. ALPS and 0-RTT are not
advertised: the HTTP/3 driver does not implement their authenticated settings
exchange. Quinn continues to handle loss recovery and key updates. Profile flow
control matches the limits actually enforced by the transport; receive burst
buffering is kept separate from individual DATAGRAM frame limits.

References: [uQUIC's versioned Chrome profiles](https://github.com/refraction-networking/uquic/blob/master/u_parrot.go)
and [RFC 9001](https://www.rfc-editor.org/rfc/rfc9001.html).

## VLESS over XHTTP

The native Rust client implements the XHTTP wire modes and options from [Xray v26.10.31](https://github.com/XTLS/Xray-core/tree/v26.10.31/transport/internet/splithttp). No external proxy core is launched by the application. The official Xray executable is used only by optional interoperability tests.

| Feature | Behavior |
|---|---|
| Upload modes | `packet-up`, `stream-up`, `stream-one`; `auto` selects packet-up normally, stream-one for REALITY, or stream-up for REALITY with a separate download leg |
| HTTP versions | HTTP/1.1, HTTP/2 and HTTP/3; stream-one requires HTTP/2 or HTTP/3 |
| Transport security | Native TLS and REALITY over HTTP/2; HTTP/3 uses QUIC/TLS and the existing QUIC certificate policy |
| Split download | Separate address, port, HTTP version, TLS/REALITY and XHTTP host/path/options in `downloadSettings` |
| Metadata | Session and sequence in path, query, header or Cookie; configurable keys, session alphabet and length |
| Packet upload | Body, header or Cookie payload; configurable chunk size, POST size, upload method and pacing |
| Padding | Legacy Referer padding or obfuscated header/query/Cookie/query-in-header; repeat-x and HPACK-length-aware tokenish padding |
| Multiplexing | Shared H2/H3 connections; configurable concurrency or connection target, reuse/request/age rotation and keepalive |
| Formats | Native configuration, Clash YAML, Xray JSON and VLESS share URIs; exports preserve XHTTP options |

Clash uses `network: xhttp` and `xhttp-opts`, while Xray JSON uses `xhttpSettings`. Advanced options may be direct fields or an `extra` object. As in Xray, an `extra` object replaces the direct advanced options, while the outer host/path/mode take precedence. Unknown options fail explicitly. `noSSEHeader`, `scMaxBufferedPosts`, `scStreamUpServerSecs`, `serverMaxHeaderBytes` and `h2Params` include server-side settings in Xray; accepting and preserving them does not apply them to an unrelated server. Their client-side counterparts remain locally bounded.

Router budgets are deliberate: at most 32 cached connections per factory, 256 sessions per connection, an 8 MiB shared packet-upload budget, at most 4 MiB per upload packet and 128 KiB of HTTP headers. Stream pipes are 16 KiB; HTTP/2 stream windows are 64 KiB and ordinary QUIC connections use a 256 KiB receive window. Chrome QUIC preserves its required 15 MiB window and 2 MiB datagram budget; all XHTTP QUIC connections share a 96 MiB reservation limit. Cached idle connections are reclaimed after about 90 seconds. Expired reuse slots are retired without aborting existing requests. Dropping a tunnel cancels its request tasks; dropping the last connection owner stops its driver. Header timeouts and non-200 responses return errors without exposing response bodies.

REALITY uses the existing native authentication and classic-key-share fallback. Vision remains restricted to native TLS/REALITY over TCP and cannot be layered over XHTTP. Download settings support the native XHTTP/TLS/REALITY fields, not arbitrary Xray socket options, dialer proxies or transport masks; those settings are rejected. HTTP/3 browser fingerprints use the existing supported QUIC profiles, rather than claiming every Xray/browser fingerprint is reproduced.

Interoperability tests cover the HTTP/mode matrix, REALITY modes, separate download security, custom upload methods, metadata locations, header/Cookie uploads, both padding algorithms and connection rotation, with TCP and UDP traffic through official Xray. Resource regressions cover rejection, cancellation and busy-slot retirement. Xray stream-one can terminate its session at upload EOF, so end-to-end TCP half-close semantics depend on the peer; applications should normally read their response before closing the tunnel.

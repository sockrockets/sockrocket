# Implementation overview (condensed)

For contributors: how crates fit together and where main logic lives.

## Workspace

| Crate | Role |
|-------|------|
| `sockrocket-core` | Protocols, config, DNS, router, local proxy, TUN |
| `sockrocket-gui` | GPUI desktop (`sockrocket` binary) |
| `sockrocket-cli` | Headless daemon + Merlin API/CGI |
| `craftls` | TLS ClientHello fingerprinting for TCP TLS |

Key deps: `tokio`, `quinn` (TUIC/Hy2), `shadowsocks`, `tun` + `ipstack`, `gpui`.

## Outbound trait

All protocols implement `Outbound::connect(host, port) -> BoxProxyStream`. Wrappers:

- `DirectOutbound` — TCP tuning, 10s timeout  
- `RetryOutbound` — 3 tries, exponential backoff (TCP outbounds only)  
- `RoutingOutbound` — `Router` chooses proxy vs direct  

Factory (`proxy/factory.rs`) maps `Node` → outbound; QUIC types manage their own reconnect.

## TCP vs QUIC lifecycle

**TCP (VMess, VLESS, Trojan, SS):** `ConnPool` (capacity 4, max age ~30s) reuses TLS+TCP where possible; each connect writes protocol header.

**QUIC (TUIC, Hy2):** One shared `Connection`; each request `open_bi()` (or H3 stream). Stale connection: detect via `close_reason`, reconnect under write lock.

## Transports

`transport.rs` + **craftls** for browser-like fingerprints on TCP TLS. QUIC uses standard rustls 0.23 roots (separate static cache).

## Local ingress

- **SOCKS5** — CONNECT only; UDP ASSOCIATE acknowledged but not fully relayed.  
- **HTTP** — CONNECT tunnel + plain GET forward; 8 KiB header limit.  

Both spawn `relay_bidirectional` (64 KiB buffers, custom Future polling both directions).

## Router

Rules evaluated in order; result cached (see [analysis](./analysis.md) for cache strategy). GeoIP for `geoip:CN` etc.

## DNS

Resolver with cache, inflight dedup, optional split DNS and fake-IP (`dns/fakeip.rs`) for TUN/Merlin.

## ProxyService

Starts SOCKS + HTTP listeners; `watch` channel for shutdown; atomic stats (with relay counting caveat).

## GUI (`app.rs`)

State: nodes, selection, proxy session id (stale async guard), latency tests (semaphore cap 5), system proxy integration, optional TUN. Flow: `create_outbound` → `ProxyService::start` → optional `verify_local_http_proxy`.

## CLI / Merlin

`main.rs` reads extended YAML keys, TUN setup, health loop, DNS listener. Merlin shell scripts manage iptables, dnsmasq drop-in, watchdog.

## TUN mode

Creates tun device, ipstack parses packets into streams, routes via same `Router` + outbound. Desktop: route hijack + emergency restore on exit/panic.

## Subscriptions

HTTP fetch → format detect (Clash / SingBox / V2Ray / base64 lines / URIs) → `Vec<Node>` merged in GUI or CLI startup.

---

Diagram (TUIC path):

```
App → SOCKS5/HTTP → RoutingOutbound → TuicOutbound → QUIC → server → target
```

See [Architecture](../architecture.md) and [Merlin reference](../merlin-reference.md) for router-specific paths.

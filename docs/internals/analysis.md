# Technical analysis (condensed)

Supplement to implementation docs: behavior verified in source, not only comments.

## Router cache

`router/engine.rs`: comment says LRU-like; at cap (**4096**) the map **`clear()`s entirely**, not single eviction. Effect: periodic miss storms and write-lock contention. Fix: `lru::LruCache` with bounded size.

## DNS cache

`dns/mod.rs`: true LRU via `last_used` and eviction at **1024** entries. Stale-while-revalidate may spawn a separate resolver for background refresh (duplicate inflight possible, low risk).

## Relay byte stats

`proxy/relay.rs`: `transfer_one` uses a local `transferred` counter reset each poll; partial writes that return `Pending` lose counts. `ProxyStats` in GUI can under-report badly. Forwarding itself is correct. Fix: accumulate via `&mut u64` on the `Relay` struct across polls.

## Speed test download

`speedtest.rs`: `GET /generate_204` has almost no body; `download_kbps` is meaningless until the endpoint uses a sized download (e.g. Cloudflare `__down?bytes=`).

## DNS inflight dedup

`OnceCell` per domain in inflight map; concurrent callers share one query. If all waiters cancel before completion, an entry may linger until next query (bounded by domain cardinality).

## ConnPool

`schedule_fill` atomic `filling` flag + double-check on insert — no over-fill race under review.

## QUIC keepalive

TUIC: 10s keepalive, 30s idle — safe. Hysteria2: 15s keepalive equals half idle — tighten to 10s for margin.

## Concurrency (GUI)

GPUI owns `AppState` on main thread; tokio **2** workers run I/O. Updates post back via `weak_handle.update`. Outbound state uses `RwLock`/`Mutex` as appropriate.

## Security highlights

- Local SOCKS5: no auth — warn if `listen_addr` is not loopback.
- `skip_cert_verify`: MITM risk; should be visible in UI.
- Credentials in plain YAML — same as other clients.
- Target hostnames for browsing go to remote proxy; local DNS mainly resolves proxy server names.

## RetryOutbound

Three attempts with 200ms then 400ms backoff between TCP failures; worst case adds protocol connect timeouts.

## Routing edge cases

Domain suffix rules normalize leading dots; CIDR prefix 0/32 handled via `checked_shl`.

## Subscriptions

Ensure HTTP client timeout (~30s) so startup is not blocked indefinitely.

## Tests

Good coverage on parsers and local proxy; gaps on full outbound connect paths, relay stats, router flush behavior, and QUIC stale reconnect races.

*Verify against current `main` when changing behavior.*

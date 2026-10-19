# Optimization and backlog (condensed)

Prioritized fixes and improvements from code review. See [analysis](./analysis.md) for rationale.

## Priority 1 — bugs (small diffs, high impact)

| ID | Area | Action |
|----|------|--------|
| BUG-FIX-1 | `relay.rs` | Cross-poll byte counting for stats |
| BUG-FIX-2 | `router/engine.rs` | Replace full `clear()` with LRU cap 4096 |
| BUG-FIX-3 | `hysteria2.rs` | `keep_alive_interval` 10s (not 15s) |
| BUG-FIX-5 | GUI latency | Store `None` on test failure, not 9999 |

## Priority 2 — performance

| ID | Area | Action |
|----|------|--------|
| PERF-1 | `pool.rs` | Lower max_age (~20s) or idle probe before reuse |
| PERF-2 | `speedtest.rs` | Keep-alive for latency test; sized download for throughput |
| PERF-3 | Router | Double-check cache before insert under write lock |

## Priority 3 — stability

| ID | Area | Action |
|----|------|--------|
| STAB-1 | SOCKS5 | Return REP 0x07 for unsupported UDP ASSOCIATE |
| STAB-2 | GUI | Periodic local port health check after connect |
| STAB-3 | DNS | Share inflight map for stale refresh tasks |

## Features (later)

- GUI throughput test after BUG-FIX-4 endpoint fix  
- Scheduled subscription refresh in GUI  
- Connection-level routing log lines  
- Optional local SOCKS auth when listening on LAN  

## Known issues (unchanged behavior)

- Router cache flush at 4096 entries  
- Relay stats underestimate traffic  
- `speed_test_node` download metric useless on 204 endpoint  
- HTTP proxy hop-by-hop headers not stripped on plain HTTP forward  
- TUN cleanup may run twice on panic + signal (guard with atomic)  
- Merlin: UDP through TUN incomplete; QUIC often bypasses at iptables  

## Suggested tests

- Router cache behavior at capacity  
- Relay totals on multi-poll transfer  
- RetryOutbound timing with mock failing outbound  
- ConnPool concurrent `get()` without exceeding capacity  

## Effort snapshot

Most P1 items are under ~30 lines each except relay refactor (~30 lines). FEAT items (GUI speed column, auto-refresh) are larger.

*Re-prioritize against product goals before large refactors.*

# Merlin transport repair, 2026-10-27

## Evidence and scope

The retained router log contained 944 payload retry-limit warnings between 01:53 and 04:42 PDT, a 512-session admission limit event at 03:35, and UDP byte-budget drops at 04:41. Earlier system logs recorded memory fail-open events at 94–97% usage. These observations establish resource pressure, but do not establish which process owned the memory during the earlier incidents. Raw router logs and configuration remain in ignored local storage.

A memory-only packet reproduction confirmed two TCP defects: retries depended on receiving another packet, and retry exhaustion discarded unacknowledged bytes while leaving the connection established and pending reads unresolved. The latter also bypassed the TCP task-error counter. This could retain failed relays until their ordinary idle timeout.

## Changes

- Schedule payload retries independently of inbound traffic and re-arm the worker after application writes. Preserve bytes until acknowledged or until the final retry's complete timeout expires.
- Fail exhausted flows, attempt a peer RST, clear buffered payload, wake pending reads/writes, count the failure, and allow the bidirectional relay to release its session. Local packet-queue backpressure does not consume a retry. Zero-window updates pause/resume data retrying; this change does not add TCP persist probes.
- Limit fast retransmission to one per duplicate-ACK run and cap exponential backoff. Process retries in bounded turns so other flows and cancellation remain schedulable.
- Bound stalled UDP upstream/client writes to 30 seconds, closing a path that could bypass the relay's existing idle timeout.
- Record anonymous process RSS/high-water usage before memory-pressure fail-open so a future incident can be attributed.

The 512-session admission limit, UDP queue budgets, and memory fail-open threshold remain unchanged. Network loss can still produce retry failures, handshake-reset warnings, or UDP drops. This repair addresses confirmed transport retention defects; it does not prove that all earlier memory pressure had the same cause.

## Validation

- Ten deterministic, memory-only TCP regressions: independent retry; retry exhaustion and reader wakeup; final-retry grace; ACK cancellation and re-arming; partial ACK; terminal RST; blocked writer wakeup; real relay/session reclamation; zero-window pause/resume; duplicate-ACK amplification.
- Two paused-clock UDP regressions covering stalled writes in each direction.
- Full core/CLI Rust suite: 632 passed, 15 ignored. Vendored ipstack private unit suite: 5 passed.
- Merlin fault-injection suite: 41 scenarios passed. Shell lifecycle suite: 63 checks passed.
- Product-library/binary clippy completed with existing warnings. ARM64 musl release build completed. Shell syntax and whitespace checks passed.

Deployment is validated separately against binary/script checksums, DNS on ports 53/5300, firewall consistency, API access, proxied HTTP/HTTPS, and LAN HTTPS through the router's Fake-IP route. Short post-deployment observation cannot prove long-term memory stability.

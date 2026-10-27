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

## Follow-up: silent half-closed relays

After 33 minutes on the first repair, the router had no admission rejects, UDP queue-drop warnings, or memory-pressure warnings. Ten retransmission failures were correctly reflected in the task-error counter. However, 56 TCP flows were in FIN_WAIT_2, with the oldest closing state about 25 minutes old. The relay used the same 1800-second inactivity allowance before and after EOF, permitting silent half-closed flows to retain their admission slots for 30 minutes.

The follow-up observes EOF in either direction and caps subsequent inactivity at 60 seconds, checked at intervals of at most 30 seconds. Payload progress in either direction renews the allowance, so a long response or upload can continue. An EOF after a long established idle period gets fresh grace. Fully established connections retain their configured idle allowance. A protocol that intentionally stays half-closed and silent for more than 60 seconds may now time out; this is the explicit resource policy for TUN relays.

Five added regressions cover both EOF directions, preservation of established idle time, fresh EOF grace, continued half-closed payload in either direction, and actual FIN_WAIT_2 relay/TCB reclamation. The two initial silent-flow regressions failed before the change. The complete Rust suite passed 637 tests with 15 ignored; the expanded symmetric payload test also passed independently. Product clippy, formatting/whitespace checks, and the ARM64 musl release build passed (existing warnings remain). The earlier shell/control-script checks remain applicable because this follow-up changes only the relay and its tests.

## Follow-up: abandoned handshakes

The next complete log capture revealed a later burst on the first repair: 243 SYN_RECEIVED flows at 05:51 PDT and 667 cumulative admission rejects by 05:52. Between 05:48 and 05:55, 817 handshake-reset warnings involved three LAN sources; 808 belonged to one source. This is evidence of a concentrated connection burst, not proof of its application or cause. The half-close repair's initial three-minute window had no rejects or warnings; it does not establish that the burst cannot recur.

An uncompleted handshake also inherited the 1800-second established idle budget and depended on application read polling. Add a separate `TcpConfig.handshake_timeout`, default 10 seconds, driven by the TCP worker. Duplicate SYNs still receive SYN-ACK replies but do not renew the deadline. Require the final ACK to acknowledge the SYN byte; a stale ACK must not bypass the deadline. A valid handshake disables this deadline and retains the established idle policy. Expiry counts as a task failure, attempts RST, and wakes I/O so the bounded upstream connect/relay task can release its slot.

Four regressions cover expiry without application polling, duplicate SYNs, stale final ACKs, and valid handshake cancellation. The initial silent-handshake regression failed before the change. This bounds abandoned handshakes; it does not guarantee that a sufficiently large simultaneous connection burst will never reach the intentional 512-session limit.

Final validation: 641 Rust tests passed, 15 ignored; product clippy, formatting/whitespace checks, and ARM64 musl release build passed with existing warnings. The earlier 41 fault scenarios and 63 shell lifecycle checks still apply to the unchanged control script.

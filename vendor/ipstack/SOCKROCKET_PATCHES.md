# Local ipstack patch

Based on the Apache-2.0 ipstack 1.0.0 release from https://github.com/narrowlink/ipstack.
Original copyright and license notices are retained. Library sources are vendored to make the session lifecycle correction reproducible on desktop and Merlin builds. Upstream benchmark/test harnesses and example-only build/DLL-copy tooling are omitted; Sockrocket manages its own WinTun runtime.

Sockrocket changes in src/lib.rs:

- Recover the packet from a closed session sender and recreate a valid session without discarding its first datagram.
- Associate delayed removal notifications with the original channel; an old notification cannot delete a replacement session sharing the same tuple.
- Ignore expected late removal notifications during stack shutdown rather than reporting them as independent errors.

TCP session creation continues through the original SYN validation. Unexpected accept/device failures retain their existing error handling.

TCP lifecycle changes in src/stream/tcp.rs:

- Complete write-half shutdown when drained data and FIN are queued, preserving peer-to-local reads instead of waiting for the peer FIN.
- Bound unacknowledged shutdown drain to five seconds and wake on payload ACKs; read idle timers are not required to drive cleanup.
- Abort the receive task without blocking the runtime during stream destruction, including cancellation and current-thread runtimes.
- Refresh the TCP idle deadline only when payload bytes are read or written; pending I/O polls and empty operations do not extend a dead connection's lifetime.
- Move ownership of the receive sender into the worker, publish terminal errors before channel closure, and wake pending readers and writers on worker completion or peer reset.
- Deliver peer FIN as ordered, persistent read EOF while preserving the response direction. Defer out-of-order FIN until preceding data arrives, and acknowledge duplicate FIN without duplicating EOF.
- Keep local FIN shutdown separate from peer FIN, validate final ACKs, support simultaneous close, and retry FIN with its original sequence number while a stream remains alive.
- Drive payload retransmission from an independent Tokio deadline, including after a local write when no inbound packets arrive. Retain unacknowledged bytes through the final retry's full timeout; exhaustion fails the flow, clears buffered payload, wakes pending I/O, and releases the relay/session instead of silently dropping bytes.
- Count a retransmission only after successful queueing, wait for device queue capacity without spending the retry budget, and bound each retry turn. Apply capped exponential backoff, pause data retries for a zero receive window, and send only one fast retransmission per duplicate-ACK run.
- Bound initial handshakes with an independent configurable deadline (10 seconds by default), even without application I/O polling. Duplicate SYNs do not renew it; only an ACK of the SYN byte can establish the connection. Expiry uses the same terminal error, wakeup and cleanup path as failed payload retries.

Resource and observability changes:

- Account queued packet payload plus metadata against per-flow and global byte budgets, with bulk headroom reserved for DNS/control traffic. Bound stream acceptance and reserve a separate DNS acceptance queue/session headroom.
- Include application-queued TCP bytes in the advertised receive window; bound queued chunks and packet-map entries to control tiny-segment metadata; reopen windows on consumption.
- Keep producer waiters weakly owned and scoped to the exhausted budget; wake in round-robin order rather than waking all flows on every packet.
- Publish anonymous TCP-state counts, receive-buffer occupancy and oldest closing age; terminate the stack on device EOF/error or a bounded write stall.
- Count peer TCP resets separately from task errors, scoped to each stack. Established/closing peer resets are DEBUG events; handshake resets and unexpected task errors retain WARN. Publish cumulative anonymous counts in TUN health samples and diagnostics.
- Reset UDP idle deadlines only on activity, reject undersized reads without panicking, and reject oversize writes rather than silently splitting or truncating datagrams.

TCP receive reassembly correction (2026-11-01):

- Preserve the new tail of retransmissions that overlap acknowledged bytes.
- Buffer out-of-order PSH payload just like other TCP payload and send a cumulative ACK for gaps or exhausted receive capacity.
- Trim duplicate ranges when extracting overlapping queued segments, retaining shorter-resend tails and releasing duplicate payload reservations. When partial extraction ends at another queued segment, preserve the longer tail and release duplicate reservations.
- Regressions cover partial retransmissions, reordered PSH, overlapping queue entries, sequence wrap, payload accounting, and a lossless 3 MiB upload with repeated reordering/overlap.

TCP retransmission timer correction (2026-11-02):

- Use one connection-level payload timer and retain its backed-off RTO when the oldest segment is acknowledged. Restart the timer when an ACK confirms new bytes or a zero receive window reopens; duplicate/stale ACKs and appending new data do not postpone it.
- Estimate SRTT/RTTVAR from fresh data, apply Karn's rule after retransmission, and clamp RTO to the configured initial floor and a cap of at least 60 seconds. Clean timing samples let healthy connections recover from backoff without forcing a reset when moving between segments.
- Fast retransmission restarts the timer without doubling RTO or consuming the timeout-retry budget. Window updates and ACKs carrying payload do not count as duplicate ACKs; see [RFC 5681 section 2](https://www.rfc-editor.org/rfc/rfc5681.html#section-2).
- Timeout recovery selects only the earliest unacknowledged segment; a later segment's older deadline cannot bypass the oldest segment's backed-off timer. See [RFC 6298 sections 5.3–5.4](https://datatracker.ietf.org/doc/html/rfc6298#section-5).
- Separate retransmission/probe packet sizing from new-write credit: a full send window must not truncate owned retry bytes into empty packets. Respect MTU and a shrunken peer window, and reset consecutive timeout counts when partial delivery advances.
- Add paced zero-window probing: retransmit one owned byte, or use an old-sequence ACK when a writer is blocked with no queued data. A replying peer may remain flow-controlled without exhausting data retries; unanswered probes are bounded. Queue backpressure does not spend probe attempts. Stop probes when all held data is acknowledged and no writer re-arms them. See [RFC 9293 section 3.8.6.1](https://www.rfc-editor.org/rfc/rfc9293.html#section-3.8.6.1).
- Process fresh downstream ACK/window information carried by duplicate upstream payload, and preserve a new FIN following already-delivered bytes.
- Raise the default retry budget from three to six. With an initially unmeasured one-second RTO, an open peer window and no ACK progress, capped exponential backoff now exhausts after approximately 123 seconds instead of 15 seconds. [RFC 9293 section 3.8.3](https://www.rfc-editor.org/rfc/rfc9293.html#section-3.8.3) recommends a failure threshold of at least 100 seconds. Handshake, idle and application deadlines still apply independently; retry exhaustion still fails pending I/O and returns payload reservations.
- Memory-packet regressions cover partial ACK progress near expiry, retained backoff across segments, fast retransmission versus timeout budgets, window updates versus duplicate ACKs, RTT smoothing and ambiguous samples, zero-window reopening and lost updates, full send-window retries, partial delivery after window shrinking, duplicate payload/FIN handling, queue backpressure, a 20-second blackout with recovery, oldest-segment selection and eventual cleanup of a permanently silent peer.

- Coalesce adjacent or overlapping upload segments into bounded reassembly chunks. At the metadata limit, advertise the leading gap and retain per-flow byte capacity for its repair. Regression coverage includes a 180-segment burst across sequence wrap, duplicate payload accounting, and 64 disjoint buffered ranges.

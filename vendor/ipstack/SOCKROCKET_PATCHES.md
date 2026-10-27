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

Resource and observability changes:

- Account queued packet payload plus metadata against per-flow and global byte budgets, with bulk headroom reserved for DNS/control traffic. Bound stream acceptance and reserve a separate DNS acceptance queue/session headroom.
- Include application-queued TCP bytes in the advertised receive window; bound queued chunks and packet-map entries to control tiny-segment metadata; reopen windows on consumption.
- Keep producer waiters weakly owned and scoped to the exhausted budget; wake in round-robin order rather than waking all flows on every packet.
- Publish anonymous TCP-state counts, receive-buffer occupancy and oldest closing age; terminate the stack on device EOF/error or a bounded write stall.
- Count peer TCP resets separately from task errors, scoped to each stack. Established/closing peer resets are DEBUG events; handshake resets and unexpected task errors retain WARN. Publish cumulative anonymous counts in TUN health samples and diagnostics.
- Reset UDP idle deadlines only on activity, reject undersized reads without panicking, and reject oversize writes rather than silently splitting or truncating datagrams.

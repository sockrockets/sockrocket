# Local integration patches

Based on tokio-wireguard 0.1.3 (MIT OR Apache-2.0). Upstream authorship and licenses are retained.

- Upgrade BoringTun to 0.7.1, including the 32-bit nonce-counter fix, and retain the existing patched TLS dependency graph.
- Accept a pre-bound UDP socket so parent TUN capture cannot route WireGuard transport into itself.
- Install IP-medium default routes inside the isolated userspace stack, not the host.
- Pass optional peer preshared keys to BoringTun.
- Own an abort handle; final public-interface drop or explicit abort closes the poll task instead of leaving graceful TCP teardown running.
- Bound internal packet queues to 256 packets and 512 KiB per direction.
- Bound UDP handshake/timer response writes to one second.

No OS TUN interface, system route, DNS preference, or proxy setting is created by this library path.

- Reuse only unowned socket registrations; never overwrite an active stream.
- Scan the complete ephemeral-port range on collisions and allow explicit port 65535.
- Schedule stack polling, UDP receive/send and timers fairly; wake the stack on socket activity and fix the write-waker registration.
- Treat SYN and listen states as pending, not closed readiness; connection failures terminate instead of spinning.

- Gracefully drain unowned TCP streams instead of aborting queued responses; bound retained socket registrations under churn.
- Share the ephemeral-port lease between split stream owners, avoiding premature release when one half drops.
- Correct UDP poll-send interest and handle empty TCP read/write buffers without parking forever.
- Redact peer keys and endpoint details from Debug output.

- Reset the shared BoringTun handshake rate limiter from the interface timer, including across long-running rekeys.

- Retain the ephemeral-port lease in the socket registration until TCP teardown finishes, preventing tuple reuse against a draining connection; leases do not retain the interface itself.

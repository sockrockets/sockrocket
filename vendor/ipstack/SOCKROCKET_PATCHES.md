# Local ipstack patch

Based on the Apache-2.0 ipstack 1.0.0 release from https://github.com/narrowlink/ipstack.
Original copyright and license notices are retained. Library sources are vendored to make the session lifecycle correction reproducible on desktop and Merlin builds. Upstream benchmark/test harnesses and example-only build/DLL-copy tooling are omitted; Sockrocket manages its own WinTun runtime.

Sockrocket changes in src/lib.rs:

- Recover the packet from a closed session sender and recreate a valid session without discarding its first datagram.
- Associate delayed removal notifications with the original channel; an old notification cannot delete a replacement session sharing the same tuple.
- Ignore expected late removal notifications during stack shutdown rather than reporting them as independent errors.

TCP session creation continues through the original SYN validation. Unexpected accept/device failures retain their existing error handling.

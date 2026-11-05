# Performance branch pre-push review — 2026-11-04

The performance branch retains opt-level=3, thin LTO and the original modern TLS dependency defaults. Runtime performance changes are shared with the separate size branch; size-specific Cargo changes remain separate. The common baseline also contains earlier protocol/configuration/UI work imported from the original workspace, so this review covers more than the latest DNS and packet-queue patch.

Review covered the connection pool's global budget, single-use preconnects and cancellation; Vision's concrete TLS stream and direct transition; bounded TUN batching, MSS/window limits, payload ownership, partial writes and ACK credit; traffic-accounting attribution and persistence; raw DNS query identity, exit/generation isolation, cancellation and cleanup; and the configuration/transport changes included in the shared baseline. No additional blocking finding remains after the fixes below. Testing does not prove the absence of every defect or establish a stable speedup over the size branch.

## Fixes made during review

- DNS lookup and raw-exchange cleanup previously inferred the number of callers from `Arc::strong_count` inside `Drop`. Two concurrent destructors can both observe the other's still-live field after its cleanup lock is released. The registry can then retain a completed or cancelled entry. Both paths now count participating callers explicitly while holding the registry mutex. Last-caller cleanup also checks entry identity so an old guard cannot remove a replacement entry.
- Added deterministic extra-owner cleanup coverage and 100 rounds of concurrent wire guard destruction, alongside the existing cancellation, transaction-ID, EDNS, query-type, direct/proxy and upstream-selection tests.
- Replaced a real public endpoint in the MTU route fixture with a documentation address. The replacement was applied to all newly published ancestors, not just the final tree.

## Validation

- Runtime regression command: `cargo test -p sockrocket-core -p sockrocket-cli -p ipstack --lib --bins --tests --offline`: 819 passed, 0 failed, 21 ignored. Ignored live-node/reference tests are not counted as successful.
- GUI library tests: 9 passed. `cargo check -p sockrocket-gui --offline` passed. Existing dependency warnings remain.
- Native TLS/REALITY policy, certificate validation/pins, DNS exit separation and protocol feature scope were retained. Vision interoperability passed in both warmed and unwarmed modes (2 tests). The sanitized MTU fixture passed all 10 related tests.
- Canonical router build must continue to use `tools/build-merlin-musl.sh`, Rust's final linker, `-C link-self-contained=yes`, and the bundled musl allocator. Zig is only the C dependency compiler. The reviewed ARM64 build passed the allocator check with zero Zig SMP symbols.

## Privacy and publication scope

The new commits and changed blobs in their unpublished ancestors were scanned for private-key blocks, common access-token formats, literal bearer credentials, credential-bearing URLs, local user paths, private IP literals and high-entropy strings. Flagged examples were inspected; remaining matches are synthetic fixtures, placeholders, protocol character tables or existing configuration behavior. No real credentials were identified. A pattern scan and manual inspection are evidence, not a guarantee that arbitrary secrets can always be recognized.

No `.local` files, private configuration, router environment/credentials, raw diagnostic archives, generated executables, symbols or Impeccable state are included. Public measurements contain no subscription URLs, node credentials or captured endpoint details. Only the performance branch is pushed; no default branch, release or router update is implied by this source push. The router deployment predates the cleanup fix in this review and remains unchanged by publishing it.

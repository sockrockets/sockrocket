# Router optimization branches — 2026-11-04

These branches share one source baseline. Existing protocol, configuration, UI and earlier performance work from the original workspace is retained in the common baseline; the import is a snapshot, not an assertion that all those changes belong to the latest optimization.

| Branch | Runtime performance changes | Modern TLS features | Release profile |
| --- | --- | --- | --- |
| `codex/router-performance` | Earlier deployed optimizations plus packet queue wakeup fast path and raw DNS concurrent-query deduplication | Original dependency defaults | opt-level=3, thin LTO |
| `codex/router-size` | Identical to performance branch | Explicit ring/std/TLS 1.2/logging; aws-lc only in test dependencies | opt-level=3, fat LTO |

The size branch is based on the performance branch. The two Cargo manifests are the only source differences, so comparing these branches keeps the runtime performance changes constant. This does not isolate TLS feature selection from LTO; compare those separately before attributing a difference to one setting.

The earlier Vision preconnect pool, bounded TUN batching/shared segment payload, MSS fixes, traffic-accounting aggregation and resource limits are present in both branches. The size branch uses fat LTO, not the smaller opt-level=s experiment.

Build either branch from its own worktree with:

```sh
bash tools/build-merlin-musl.sh
```

The canonical script must continue to use Rust's `rust-lld` for final linking with `-C link-self-contained=yes`, preserving the musl allocator. Zig is only the C dependency compiler. Every candidate must pass `verify-merlin-allocator.py` before stripping, with zero Zig SMP symbols. Do not switch final linking to Zig to reproduce the size result.

Use the same router configuration, node, URL set, request count and warmup for A/B/A testing. Record failures and tail latency, not only medians. The performance branch was subsequently built and deployed for comparison. Review fixes after that deployment are source changes until explicitly deployed. Creating or pushing branches does not deploy or roll back the router. Historical measurements belong to their recorded binaries; a fresh build may have a different hash.

Original branch and its index/working files are preserved. The new branches have their own worktrees under the original repository's ignored `.local/branches/` directory. No credentials, `.local` build artifacts or Impeccable state are included in the snapshot.

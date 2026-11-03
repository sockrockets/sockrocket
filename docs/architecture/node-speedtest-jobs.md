# Node latency tests

The firmware UI submits a batch once (`speedtest_start`) and reads its state
(`speedtest_job`). A detached worker loads a private configuration snapshot and
owns one Tokio runtime for all nodes. Up to eight tasks run concurrently; real
proxy probes share two protocol slots on the router. Fast TCP remains the default
as requested; TUIC, Hysteria2 and WireGuard always use proxy HTTP probes.

Interactive HTTP probes use HEAD to http://www.gstatic.com/generate_204, require
204, and share a five-second deadline across proxy connection, writes and response
headers. They have no warmup, fallback target race or extra TCP diagnostic delay.
This follows URL-test timing semantics; it does not claim identical defaults or
protocol behavior to another client. Existing periodic warm health checks remain.

The worker stores progress and final results privately in the temporary filesystem,
with atomic snapshots at most four times per second. Refreshing a page reattaches
without launching new work. Results survive page closure but not router reboot.
Only the latest batch is retained. A launch lock and request id prevent concurrent
or replayed launches. A dead worker is shown as interrupted, retaining partial
results. Node identities hash canonical config and include the snapshot index, so
duplicates remain distinct and changed/reordered nodes cannot receive stale scores.

The UI polls once a second, maps results by identity and renders once per poll.
Normal control requests no longer wait for individual probe requests to finish.

References inspected (2026-11-02):
- Karing ServerManager.getDelay delegates to ClashApi.getDelay and records latency:
  https://github.com/KaringX/karing/blob/main/lib/app/modules/server_manager.dart
- Mihomo URLTest measures dial plus HTTP HEAD, records history, with optional
  unified-delay second request:
  https://github.com/MetaCubeX/mihomo/blob/Meta/adapter/adapter.go
- Mihomo HealthCheck uses single-flight and bounded concurrency (10 there):
  https://github.com/MetaCubeX/mihomo/blob/Meta/adapter/provider/healthcheck.go

Validation: merlin_speedtest_jobs_test.py exercises detached CGI workers, 128
identical nodes, result identity matching, failed-node isolation, retained results
and idempotent starts. webui_speedtest_batch_test.cjs checks polling, duplicate
identities, failures, refresh reattachment and bounded rendering.

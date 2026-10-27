# Merlin first-subscription latency, 2026-10-27

## Scope and evidence

The reported symptom is the router Web UI taking a long time to show nodes after adding a subscription URL. This investigation does not establish a measured comparison with another client or a specific provider.

The existing download path waited for direct HTTP to fail before trying the running daemon's SOCKS listener: up to 12 seconds direct, then up to 15 seconds proxy, plus a short listener check. Domestic DNS used `join!`, so a successful answer still waited for the second resolver's two-second timeout. After download and persistence, the Web UI waited for the service restart to finish before refreshing its node lists, polling every two seconds.

Read-only inspection of recent router job records found successful overall durations between 1 and 19 seconds. Those older records do not separate download, persistence and restart durations, so they cannot attribute an individual job's delay.

## Change

- Give direct downloads a 500 ms head start, then try the local SOCKS route concurrently if available. A direct transport failure starts fallback immediately. The first successful bounded HTTP body wins and the losing request is cancelled. A failure on one route does not abort a direct download that can still succeed. Fast direct HTTP errors retain their existing behavior and do not start fallback.
- Return the first usable real IPv4 DNS answer; an empty, Fake-IP or IPv6-only answer still waits for the other resolver. Preserve the existing address policy and each lookup's timeout.
- When an explicit local SOCKS fallback is supplied, bypass environment proxies for the direct request so both candidates cannot accidentally use the same proxy. Callers without explicit fallback retain environment-proxy behavior.
- Publish `saving` before the configuration transaction and `applying` with `nodes_saved: true` only after fresh nodes are committed. The page refreshes nodes/subscriptions once when it observes confirmed saved nodes, displays that settings are still being applied, and continues waiting for the actual final result. A failed save or an older backend's ambiguous `applying` stage cannot be displayed as ready.
- Poll running jobs every 500 ms during the first five seconds, then every two seconds. Launch recovery and transport failures keep their existing two-second retry cadence. Preserve submitted input and manual-stop behavior.
- Emit anonymous core download/parse timing events when tracing is enabled, without subscription URLs, credentials or node contents. The detached CGI worker currently has no tracing subscriber, so its existing job timestamps still measure overall completion rather than these individual phases.

Slow direct downloads can now issue a second GET through the proxy. Fast direct downloads still issue one GET. This is a bounded two-route hedge, not a retry loop. The 8 MiB response limit, TLS verification, redirect limit, per-request timeouts and outer 30-second CGI budget remain in place. A provider/network outage or a cold proxy connection can still be slow.

## Validation

- A loopback HTTP/SOCKS regression failed on the previous implementation: a healthy proxy could not finish within two seconds while direct stalled. The repaired path completed in 503 ms in the local fixture; this is a synthetic result, not a claim about real-provider latency. The same test confirms the losing direct connection closes.
- Additional HTTP/SOCKS cases cover fast direct success without duplicate GETs, direct HTTP 403 with URL-safe errors and no proxy retry, and proxy failure while direct still succeeds.
- Paused-clock regressions cover first-success DNS in either order, unusable DNS answers, immediate transport fallback, an unavailable proxy, and direct HTTP errors after proxy failure.
- A real CGI/worker test holds the configuration lock to verify `saving` precedes commit, then holds service application open to verify committed nodes are readable while the job remains running.
- Full core/CLI Rust suite: 651 passed, 15 ignored. All seven Web UI test scripts passed, including first-poll latency, slower error retries, no repeated list reloads and no premature success. Product clippy, formatting/whitespace checks and ARM64 musl release build passed with existing warnings.

The candidate is on a separate subscription-latency branch. The deployed router version is unchanged during this investigation.

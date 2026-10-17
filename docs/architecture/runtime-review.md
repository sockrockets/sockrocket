# Runtime stability and privacy review

## Changes

- Merlin Fake-IP replies have a main-table route through TUN. Verification checks that route; activation fails before LAN interception if installation fails. Shutdown removes owned routes and restores direct DNS.
- Firmware DNS inputs remain readable after dnsmasq drops privileges. Legacy permissions are repaired, the upstream list reloads once, and failed backup/restoration retains recovery evidence.
- Manual Stop survives background subscription updates and boot hooks. Explicit Start remains available through the WebUI.
- Subscription URLs, fetched nodes and active selection persist together without discarding router-specific settings. Elevated writes retain the interactive user's file ownership.
- Desktop proxy takeovers retain prior settings in a private ownership journal; automatic cleanup preserves external proxy edits. TUN recovery records survive unexpected exits, and macOS scoped routes are removed before restoring the physical default. Resolver cache commands have bounded execution time.
- RPC bodies are isolated per request and promptly removed. Responses expire, concurrency is bounded, and Stop has a separate work slot. Logs and configuration artifacts default to private permissions.
- WebUI binary updates stage and sync on the destination filesystem before stopping service. Atomic rename replaces the executable; failed staging preserves the installed binary.

## Review evidence and limits

Local validation covers core/CLI regressions, shell fault injection, RPC isolation and WebUI transport. A separate production-only source tree is compiled and tested to verify the committed files independently of new local regression material. Private subscription/node values and personal home paths are checked against the candidate files. New test fixtures, diagnostic scripts, captured logs, generated bundles and local credentials are excluded from this commit.

The deployed Fake-IP fix passed router-side DNS, route, HTTP proxy and management checks; the user subsequently reported other-device access working. These observations are not a long-term stability guarantee. Router reboot and additional live desktop exit/restart tests were intentionally not performed during the commit review, preserving the user's working connections. The latest manual-stop/update safety changes require a future deployment to reach the running router. IPv6 remains dependent on system policy and does not imply complete LAN dual-stack support. Closed-flow packet regressions did not reproduce global TUN acceptance failure; the historical channel-closed warnings remain an observation item.

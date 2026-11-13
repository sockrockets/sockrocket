# Subscription proxy server DNS

Clash subscriptions can specify `dns.proxy-server-nameserver` as a list of resolvers used to look up proxy endpoint hostnames. SockRocket retains that list on each imported node and uses it for both normal connections and node probes. It does not replace destination DNS or another subscription's policy.

Supported endpoints are plain IP addresses (optionally with a port), `udp://`, `tcp://`, `tls://` and `https://`. HTTPS paths, queries and non-default ports are retained. `dns.default-nameserver` supplies IP-based UDP bootstrap resolvers for named DNS endpoints; when absent, the existing public bootstrap defaults are used. `dns.ipv6` controls endpoint AAAA resolution. TLS certificates remain verified.

When `proxy-server-nameserver` is absent, SockRocket uses the subscription’s `dns.nameserver` list to resolve node hostnames. Refresh saved subscriptions after upgrading to apply the new policy. The import covers the primary resolver list. Clash’s `fallback` and `fallback-filter` rules are outside its scope.

SockRocket keeps its existing node DNS settings when general DNS is disabled, the primary list is empty, or the inherited settings use unsupported syntax. An explicitly empty `proxy-server-nameserver` also keeps the existing settings. A nonempty explicit policy takes precedence and must pass validation. For example, Mihomo transport fragments and QUIC DNS are unsupported, and named resolver endpoints require IP-based UDP bootstrap. Validation errors omit private resolver URLs. Once a policy is imported, a failed lookup returns an error.

Policies survive native configuration saves and reloads. Nodes with different endpoint DNS policies have different deduplication identities. DNS answers follow the resolver cache TTL; a changed endpoint creates a new transport while retaining the original SNI, certificate name, and WebSocket host. No resolved IP is written back to the subscription or saved node.

Private resolver paths and query strings can contain credentials. They belong only in private user configuration, never in diagnostics or committed fixtures. Tests use synthetic domains and credentials.

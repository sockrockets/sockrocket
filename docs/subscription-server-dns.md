# Subscription proxy server DNS

Clash subscriptions can specify `dns.proxy-server-nameserver` as a list of resolvers used to look up proxy endpoint hostnames. SockRocket retains that list on each imported node and uses it for both normal connections and node probes. It does not replace destination DNS or another subscription's policy.

Supported endpoints are plain IP addresses (optionally with a port), `udp://`, `tcp://`, `tls://` and `https://`. HTTPS paths, queries and non-default ports are retained. `dns.default-nameserver` supplies IP-based UDP bootstrap resolvers for named DNS endpoints; when absent, the existing public bootstrap defaults are used. `dns.ipv6` controls endpoint AAAA resolution. TLS certificates remain verified.

Missing or empty `proxy-server-nameserver` retains the existing resolution path. An explicit policy that fails does not fall back to system DNS. Unsupported endpoint syntax (including Mihomo transport fragments and QUIC DNS) is rejected with an error that does not include the resolver URL. Encrypted-only default-nameserver bootstrap is currently unsupported for named resolver endpoints and is rejected rather than downgraded to plaintext.

Policies survive native configuration saves and reloads. Nodes with different endpoint DNS policies have different deduplication identities. DNS answers follow the resolver cache TTL; a changed endpoint creates a new transport while retaining the original SNI, certificate name, and WebSocket host. No resolved IP is written back to the subscription or saved node.

Private resolver paths and query strings can contain credentials. They belong only in private user configuration, never in diagnostics or committed fixtures. Tests use synthetic domains and credentials.

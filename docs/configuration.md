# Configuration reference

> New users should start with [Getting started](./getting-started.md).

## Internal YAML format

All imported configs normalize to Sockrocket’s internal YAML schema.

```yaml
listen_addr: "127.0.0.1"
socks_port: 1080
http_port: 1087
active_node: 0

subscriptions:
  - name: "My Subscription"
    url: "https://example.com/subscribe"
    format: auto

nodes:
  - name: "HK-SS"
    server: "proxy.example.com"
    port: 8388
    protocol:
      type: shadowsocks
      cipher: aes-256-gcm
      password: "your-password"
      udp: true

  - name: "JP-VMess"
    server: "proxy.example.com"
    port: 443
    protocol:
      type: vmess
      uuid: "b0e80a62-8a51-47f0-91f1-f0f7faf8d9d4"
      alter_id: 0
      cipher: auto
      udp: true
    transport:
      type: tls
      tls:
        sni: "example.com"
        alpn: ["h2", "http/1.1"]

rules:
  - rule_type: domain-suffix
    pattern: "google.com"
    target: proxy
    priority: 10          # higher runs first (default 0)
  - rule_type: dst-port
    pattern: "443"        # or "1000-2000"
    target: direct
  - rule_type: ip-cidr
    pattern: "192.168.0.0/16"
    target: direct
  - rule_type: geoip
    pattern: CN           # built-in ~7456 China IPv4 CIDRs (+ private)
    target: direct
  - rule_type: final      # alias: match
    pattern: "*"
    target: proxy
```

Rule types: `domain`, `domain-suffix`, `domain-keyword`, `ip-cidr`, `geoip`, `dst-port` (alias `port`), `final`/`match`.

Evaluation: enabled rules sorted by **priority descending**, then list order. First match wins.

Refresh the CN CIDR database with `scripts/update-china-cidrs.sh` (merged from chnroutes2, Loyalsoldier, and china-operator-ip).

## DNS and IPv6

Merlin's **Network & DNS** page configures upstreams, cache limits and IPv6 forwarding. Desktop Settings exposes direct/proxy/bootstrap upstreams, address strategy and cache enablement; the advanced DNS fields below are also preserved when edited in YAML.

```yaml
# Merlin only. Existing installations retain IPv4 compatibility protection.
ipv6_mode: compatibility  # compatibility | dual_stack
dns:
  direct_servers:
    - "udp://223.5.5.5:53"
    - "udp://119.29.29.29:53"
  proxy_servers:
    - "tls://8.8.8.8:853?server_name=dns.google"
    - "tls://1.1.1.1:853?server_name=cloudflare-dns.com"
  bootstrap_servers:
    - "223.5.5.5:53"
    - "119.29.29.29:53"
  strategy: prefer_ipv4  # prefer_ipv4 | prefer_ipv6 | ipv4_only | ipv6_only
  cache_enabled: true
  cache_capacity: 1024
  cache_ttl: 0           # 0 uses upstream TTL; otherwise caps cache retention
  timeout_secs: 5
  direct_domains: []    # additional suffixes; built-in China domains remain
```

- Missing DNS fields use defaults. Direct and proxy groups each require 1–8 endpoints; bootstrap requires 1–4 IP:port entries. IPv6 literals use brackets, for example `[2001:db8::53]:53` (documentation address; replace before use).
- Direct upstreams accept `udp://`, `tcp://`, `tls://` and `https://`; proxy upstreams accept TCP, TLS and HTTPS. UDP requires a literal IP. Example HTTPS endpoint: `https://dns.google/dns-query`. TLS accepts an optional `server_name` query parameter when connecting to an IP. Certificates are always verified; URL credentials and fragments are rejected.
- DNS endpoint hostnames and proxy node hostnames use independent bootstrap DNS. Node bootstrap can query both address families even when client answers are restricted to IPv4 or IPv6. A proxy-group failure is not retried against the direct group.
- Cache capacity is 16–16384 entries, retention cap 0–86400 seconds, and query timeout 1–12 seconds. Wire-answer caching additionally has a 1 MiB byte limit, and internal address results are bounded. Changing the published DNS policy invalidates old cache generations. HTTPS currently uses bounded HTTP/1 exchanges; DoQ and HTTP/3 DNS are not implemented.
- Omit `dns` to continue reading Merlin's legacy `dns_direct_server` and `dns_direct_domains`. An explicit `dns` object takes precedence. Configuration writes preserve unrelated subscription, node and router fields. Local/loopback resolver endpoints are intentionally rejected to prevent recursion through the application's own listener or dnsmasq.
- Saving in Merlin reapplies the firewall policy in place without restarting the proxy, preserving the existing DNS/TUN toggles; a stopped service remains stopped. If the host cannot complete dual-stack, the IPv4 proxy and DNS hijack stay in place, external LAN IPv6 is rejected, and the watchdog retries every minute. Save/apply failures identify whether settings were persisted. The DNS test checks the current form without saving it and reports safe error codes rather than private URLs or credentials.
- Default and recommended when the ISP IPv6 status is unknown: `compatibility`. It keeps the IPv4 proxy, answers LAN DNS with IPv4 only, and rejects forwarded external IPv6 only when the firmware already has IPv6 enabled. No ISP or Merlin IPv6 setup is required.
- `dual_stack` is optional after the firmware already has IPv6 (typically Merlin “Native” with a LAN prefix). It installs owned IPv6 TCP/UDP policy routes and DNS interception without changing firmware IPv6 enablement, RA/DHCPv6 or WAN default routes. If dual-stack cannot finish, IPv4 proxy and DNS hijack stay in place, external LAN IPv6 is rejected, and the watchdog retries every minute. IPv6-only DNS is rejected with compatibility-mode transparent routing.
- Dual-stack uses an IPv6 Fake-IP prefix from benchmark space and the same persisted domain allocation identity as IPv4. Stop and recovery remove application-owned IPv6 routes and firewall chains. Full LAN forwarding, WAN prefix changes and prolonged workloads still require router acceptance before enabling this candidate in production.

`geoip:CN` now also uses IPv6 CN allocation prefixes generated by [scripts/update-china-ipv6.py](../scripts/update-china-ipv6.py). These [APNIC statistics](https://ftp.apnic.net/stats/apnic/delegated-apnic-latest) describe original allocations, not guaranteed current physical location. Specific domain/IP rules can override that approximation.

---

## Import formats

### Clash YAML

Sockrocket reads the `proxies` list. Clash `type` maps to `protocol.type`:

| Clash | Sockrocket |
|-------|------------|
| `ss` | `shadowsocks` |
| `vmess` | `vmess` |
| `vless` | `vless` |
| `tuic` | `tuic` |
| `trojan` | `trojan` |
| `hysteria2` | `hysteria2` |

### V2Ray JSON

Standard `outbounds` JSON is parsed into nodes.

### Base64 subscription (V2Ray / generic)

1. HTTP GET subscription URL  
2. Base64-decode body  
3. Split lines; each line is a URI  

Supported prefixes: `ss://`, `vmess://`, `vless://`, `tuic://`, `trojan://`, `hysteria2://` / `hy2://`

The format hint `v2ray` (Merlin / config) means this URI/base64 list. Use `v2ray-json` for a full V2Ray `outbounds` JSON document.

### Auto detection (`format: auto`)

1. YAML with `proxies` → Clash  
2. JSON with `outbounds` → V2Ray  
3. Base64 with `://` → line URIs  
4. Otherwise try plain URI lines  

---

## Default config paths

| Platform | Path |
|----------|------|
| Windows | `%APPDATA%\sockrocket\config.yaml` |
| macOS | `~/Library/Application Support/sockrocket/config.yaml` |
| Linux | `~/.config/sockrocket/config.yaml` |

## Proxy groups and device defaults

In Merlin, open **Nodes → Node groups** to create a named set of nodes and choose its member. Names can contain letters (including Chinese), numbers, hyphens and underscores. The rule target selector, preset import selector and device policy selector all include these groups. A group selection does not change the global active node. Rules still need rule mode to take effect.

- `select`: new TCP and UDP use one member. Merlin saves only that node. The desktop app can keep several members so you can switch the current one; the others are not tried in order. A missing selection fails closed and does not use the global node.
- `fallback`: for five minutes, new TCP starts at the last member that connected. After that it starts at the first member again. If a member fails to establish, the rest are tried in list order. An established stream is not replayed. UDP stays on the first member.
- `url-test`: for five minutes, new TCP reuses the fastest recent handshake. After that the members race, and list order does not choose the winner. UDP uses that same pick and does not start a new race.

A group does nothing until a rule's exit is that group and the router is in rule mode. On Merlin: Nodes → create the group, then Routing rules → set the rule group's exit, or a single rule's action, to `分组 · name`. Direct and reject rules in that rule group stay as they are. See [Routing](./routing.md).

Membership survives display-name changes. Subscription changes that replace server credentials may invalidate a member; the page shows missing members and permits replacing them. The private configuration retains existing membership identities; API responses expose only SHA-256 opaque member IDs, not credentials or endpoints. Group renames rewrite all rule references atomically, including disabled rules. Deletion is refused while any rule references a group. Limits: 64 groups and 256 members per group. Groups cannot contain other groups.

Rules target groups with the case-sensitive syntax `group:Video` (where `Video` already exists). Native batch import/export preserves this target, for example:

```text
DOMAIN-SUFFIX,example.com,group:Video
SOURCE,192.0.2.10/32,MATCH,group:Work
```

The addresses and domains above are documentation examples only.

**Rules → Device default policy** manages a final rule for an IPv4/IPv6 address or CIDR. An empty source sets the global default. Selecting inherit removes final rules for that exact source, leaving destination-specific exceptions intact. Saving replaces final rules for that source; the UI asks for confirmation. Use DHCP reservations for stable device addresses, or a stable IPv6 prefix. MAC-address identities and DHCP discovery are not implemented.

Evaluation order is: device destination rules → global destination rules → built-in domestic/private direct → device final → global final (proxy when absent). A device final replaces only the global fallback for that source. Domestic sites and shared destination rules still apply. Priorities within each phase still apply; overlapping device subnets use configured priority, not longest-prefix selection. Inherit means deleting that source's final rule. The flow diagram and routing query use these same saved rules and group names. A fallback group's displayed first member is a candidate, not proof of the member used by a live connection. Existing streams keep their admitted policy snapshot. DNS upstream transport remains the separately configured DNS policy using the global proxy node.

## Traffic

The overview counts bytes each node forwarded since this process started. A restart starts those counters again.

The subscriptions page shows the last three calendar months. Every node has its own row, including manual nodes and Direct. A subscription total is the sum of the nodes that currently belong to it. The tables show subscription names, not subscription URLs.

Changing a group or device policy applies through the existing restart path only if the router service is running. A manually stopped service stays stopped. This feature has local regression coverage; router deployment and live device acceptance are separate steps using a complete Software Center package.

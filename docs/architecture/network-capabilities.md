# Network capabilities

This matrix describes source-level capabilities and limits. It does not replace platform acceptance or provider interoperability testing.

| Path | Capability | Boundary |
| --- | --- | --- |
| HTTP proxy | HTTPS CONNECT; HTTP/1 absolute-form forwarding | Each plain request is parsed and routed separately. Streaming request/response bodies and requested upgrades are supported. A CONNECT tunnel starts on a dedicated connection. HTTP/2 and HTTP/3 proxy requests are not implemented. |
| SOCKS5 | TCP CONNECT | No-auth negotiation only; restrict listener exposure to trusted clients. |
| SOCKS5 | UDP ASSOCIATE | TCP-owned, same-peer IP and pinned UDP port, bounded destination flows and timeouts. Fragmented SOCKS UDP requests are rejected. The selected outbound must support datagrams. |
| SOCKS5 | BIND | Unsupported; returns command-not-supported. |
| TUN | IPv4 TCP/UDP | Bounded queues and flow admission. UDP message boundaries are preserved; replies exceeding the interface MTU fail explicitly. IP fragmentation/reassembly is not a complete supported path. |
| TUN | IPv6 | The stack parses IPv6 and platform setup can assign an IPv6 address. Full transparent routing, DNS and exit/restart recovery are not accepted as complete across platforms. Merlin currently uses its explicit IPv4 fallback policy. |
| Outbound | UDP | An explicit datagram implementation is required; TCP byte-stream connection success does not establish UDP support. Provider settings and transport combinations can disable a capability. |
| macOS | System-proxy ownership | Journaled physical network-service IDs and per-protocol fields. Restore only settings still owned by the application. External changes remain external. |
| Recovery | Abrupt process exit | The independent guardian can restore recorded networking while its process and authorization remain available. Machine power loss and guardian termination require startup recovery. Retain journals on failure. |

Health is layered: a listening process, a DNS response or a router-origin outbound probe does not prove successful forwarding from a LAN device. Runtime diagnostics therefore leave LAN reachability unverified unless a separate LAN acceptance probe supplies evidence.

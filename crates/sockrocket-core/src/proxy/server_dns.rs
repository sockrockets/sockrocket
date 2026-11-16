//! Resolve subscription endpoints before constructing transports, retaining TLS identity.
use super::{
    connector::{BoxProxyStream, Outbound, SharedOutbound},
    datagram::{BoxDatagram, DatagramFuture},
};
use crate::{
    config::{
        model::*,
        server_dns::{KEY, ServerDns},
    },
    dns::DnsResolver,
};
use anyhow::Result;
use std::{future::Future, net::IpAddr, pin::Pin, sync::Mutex};

async fn endpoint_addresses(resolver: &DnsResolver, host: &str) -> Result<Vec<IpAddr>> {
    let ips = resolver
        .resolve_addresses(host)
        .await
        .map_err(|_| anyhow::anyhow!("Subscription proxy server DNS lookup failed"))?;
    anyhow::ensure!(
        !ips.is_empty(),
        "Subscription proxy server DNS returned no address"
    );
    Ok(ips)
}

pub(super) async fn connect_endpoint_tcp(
    resolver: &DnsResolver,
    host: &str,
    port: u16,
) -> Result<tokio::net::TcpStream> {
    let ips = endpoint_addresses(resolver, host).await?;
    let addresses: Vec<_> = ips.into_iter().map(|ip| (ip, port).into()).collect();
    Ok(super::transport::connect_staggered(&addresses).await?)
}

pub struct ServerDnsOutbound {
    node: Node,
    resolver: DnsResolver,
    warm: bool,
    cached: Mutex<Option<(IpAddr, SharedOutbound)>>,
}
impl ServerDnsOutbound {
    pub fn new(node: &Node, policy: ServerDns, warm: bool) -> Result<Self> {
        let resolver = DnsResolver::with_config(policy.config()?);
        let mut node = node.clone();
        node.extra.remove(KEY);
        // Validate protocol/security options now, before the first request.
        super::factory::create_outbound_unwarmed(&node)?;
        Ok(Self {
            node,
            resolver,
            warm,
            cached: Mutex::new(None),
        })
    }
    async fn addresses(&self) -> Result<Vec<IpAddr>> {
        let mut ips = endpoint_addresses(&self.resolver, &self.node.server).await?;
        if let Some((cached_ip, _)) = self.cached.lock().unwrap().as_ref()
            && let Some(index) = ips.iter().position(|ip| ip == cached_ip)
        {
            ips.swap(0, index);
        }
        Ok(ips)
    }
    fn outbound(&self, ip: IpAddr) -> Result<SharedOutbound> {
        let mut cached = self.cached.lock().unwrap();
        if let Some((cached_ip, outbound)) = cached.as_ref()
            && *cached_ip == ip
        {
            return Ok(outbound.clone());
        }
        let mut node = self.node.clone();
        node.server = ip.to_string();
        preserve_identity(&mut node, &self.node);
        let outbound = if self.warm {
            super::factory::create_outbound(&node)?
        } else {
            super::factory::create_outbound_unwarmed(&node)?
        };
        *cached = Some((ip, outbound.clone()));
        Ok(outbound)
    }
}
fn preserve_identity(node: &mut Node, original: &Node) {
    let implicit_tls = matches!(
        node.protocol,
        ProxyProtocol::AnyTls { .. }
            | ProxyProtocol::Tuic { .. }
            | ProxyProtocol::Hysteria2 { .. }
            | ProxyProtocol::Trojan { .. }
    );
    if node.transport.is_none() && implicit_tls {
        node.transport = Some(
            serde_json::from_value(serde_json::json!({"type":"tls"})).expect("static transport"),
        );
    }
    if let Some(t) = node.transport.as_mut() {
        if t.tls.is_none()
            && (implicit_tls || t.transport_type == TransportType::Tls)
            && t.reality.is_none()
        {
            t.tls = Some(TlsConfig {
                sni: None,
                skip_cert_verify: false,
                alpn: None,
                fingerprint: None,
            });
        }
        if let Some(tls) = t.tls.as_mut() {
            tls.sni.get_or_insert_with(|| original.server.clone());
        }
        if let Some(r) = t.reality.as_mut() {
            r.sni.get_or_insert_with(|| original.server.clone());
        }
        if matches!(
            t.transport_type,
            TransportType::WebSocket | TransportType::HttpUpgrade
        ) {
            // Both upgrade transports default Host to the configured server name.
            let ws = t.ws.get_or_insert(WsConfig {
                path: None,
                host: None,
                headers: None,
            });
            ws.host.get_or_insert_with(|| original.server.clone());
        }
    }
}
impl Outbound for ServerDnsOutbound {
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
        let host = host.to_owned();
        Box::pin(async move {
            let mut error = None;
            for ip in self.addresses().await? {
                match self.outbound(ip)?.connect(&host, port).await {
                    Ok(stream) => return Ok(stream),
                    Err(e) => error = Some(e),
                }
            }
            Err(error.unwrap_or_else(|| anyhow::anyhow!("Proxy server has no usable address")))
        })
    }
    fn connect_datagram(&self, host: &str, port: u16) -> DatagramFuture<'_, BoxDatagram> {
        let host = host.to_owned();
        Box::pin(async move {
            let mut error = None;
            for ip in self.addresses().await? {
                match self.outbound(ip)?.0.connect_datagram(&host, port).await {
                    Ok(stream) => return Ok(stream),
                    Err(e) => error = Some(e),
                }
            }
            Err(error.unwrap_or_else(|| anyhow::anyhow!("Proxy server has no usable address")))
        })
    }
    fn name(&self) -> &str {
        self.node.protocol.name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dns::{DnsConfig, DnsServer};
    use tokio::net::{TcpListener, UdpSocket};
    fn node() -> Node {
        crate::config::clash::parse_clash_config("proxies:\n- {name: synthetic, type: ss, server: endpoint.example.test, port: 443, cipher: aes-128-gcm, password: synthetic}\n").unwrap().remove(0)
    }
    fn wrapper() -> ServerDnsOutbound {
        ServerDnsOutbound::new(
            &node(),
            ServerDns {
                servers: vec!["1.1.1.1".into()],
                bootstrap: vec![],
                ipv6: false,
            },
            false,
        )
        .unwrap()
    }
    #[test]
    fn preserves_implicit_and_explicit_tls_identity_and_reuses_transport() {
        let mut n = node();
        n.protocol = ProxyProtocol::AnyTls {
            pool: Default::default(),
            password: "synthetic".into(),
            udp: true,
        };
        let mut resolved = n.clone();
        resolved.server = "192.0.2.1".into();
        preserve_identity(&mut resolved, &n);
        assert_eq!(
            resolved
                .transport
                .as_ref()
                .unwrap()
                .tls
                .as_ref()
                .unwrap()
                .sni
                .as_deref(),
            Some("endpoint.example.test")
        );
        resolved
            .transport
            .as_mut()
            .unwrap()
            .tls
            .as_mut()
            .unwrap()
            .sni = Some("explicit.example.test".into());
        preserve_identity(&mut resolved, &n);
        assert_eq!(
            resolved.transport.unwrap().tls.unwrap().sni.as_deref(),
            Some("explicit.example.test")
        );
        let w = wrapper();
        let a = w.outbound("192.0.2.1".parse().unwrap()).unwrap();
        let b = w.outbound("192.0.2.1".parse().unwrap()).unwrap();
        assert!(std::sync::Arc::ptr_eq(&a.0, &b.0));
    }
    #[tokio::test]
    async fn resolved_upgrade_transports_preserve_the_wire_host_and_path() {
        use tokio::io::{AsyncBufReadExt, BufReader};

        for network in ["ws", "httpupgrade"] {
            for (options, expected_host, expected_path) in [
                ("", "endpoint.example.test", "/"),
                (", ws-opts: {path: /path}", "endpoint.example.test", "/path"),
                (
                    ", ws-opts: {path: /path, headers: {Host: cdn.example.test:8443}}",
                    "cdn.example.test:8443",
                    "/path",
                ),
            ] {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let addr = listener.local_addr().unwrap();
                let yaml = format!(
                    "dns: {{enable: true, nameserver: [192.0.2.53]}}\nproxies:\n- {{name: ws-test, type: vmess, server: endpoint.example.test, port: {}, uuid: 00000000-0000-0000-0000-000000000001, network: {network}{options}}}",
                    addr.port()
                );
                let node = crate::config::clash::parse_clash_config(&yaml)
                    .unwrap()
                    .remove(0);
                let policy = ServerDns::from_node(&node).unwrap().unwrap();
                let wrapper = ServerDnsOutbound::new(&node, policy, false).unwrap();
                let resolved = wrapper.outbound(addr.ip()).unwrap();
                let capture = async {
                    let (stream, _) = listener.accept().await.unwrap();
                    let mut lines = BufReader::new(stream).lines();
                    assert_eq!(
                        lines.next_line().await.unwrap().unwrap(),
                        format!("GET {expected_path} HTTP/1.1")
                    );
                    let mut host = None;
                    while let Some(line) = lines.next_line().await.unwrap() {
                        if line.is_empty() {
                            break;
                        }
                        if let Some((name, value)) = line.split_once(':')
                            && name.eq_ignore_ascii_case("host")
                        {
                            host = Some(value.trim().to_owned());
                        }
                    }
                    assert_eq!(host.as_deref(), Some(expected_host), "{network}{options}");
                };
                tokio::time::timeout(std::time::Duration::from_secs(3), async {
                    tokio::select! {
                        () = capture => {}
                        result = resolved.connect("destination.example.test", 80) =>
                            panic!("outbound finished before {network} handshake: {:?}", result.err()),
                    }
                })
                .await
                .unwrap();
            }
        }
    }

    #[tokio::test]
    async fn subscription_dns_controls_actual_socket_and_never_falls_back() {
        let dns = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let server = dns.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let mut buf = [0; 4096];
            loop {
                let (len, peer) = dns.recv_from(&mut buf).await.unwrap();
                let reply =
                    crate::dns::build_dns_response(&buf[..len], &["127.0.0.1".parse().unwrap()])
                        .unwrap();
                dns.send_to(&reply, peer).await.unwrap();
            }
        });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut w = wrapper();
        w.node.port = listener.local_addr().unwrap().port();
        w.resolver = DnsResolver::with_config(DnsConfig {
            servers: vec![DnsServer::Udp(server)],
            fallback: vec![],
            ..Default::default()
        });
        let _stream = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            w.connect("target.example.test", 80),
        )
        .await
        .unwrap()
        .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let probe = connect_endpoint_tcp(&w.resolver, &w.node.server, w.node.port)
            .await
            .unwrap();
        assert_eq!(probe.peer_addr().unwrap(), listener.local_addr().unwrap());
        task.abort();
        w.resolver = DnsResolver::with_config(DnsConfig {
            servers: vec![],
            fallback: vec![],
            ..Default::default()
        });
        assert!(
            w.connect("target.example.test", 80)
                .await
                .err()
                .unwrap()
                .to_string()
                .contains("DNS")
        );
        assert!(
            connect_endpoint_tcp(&w.resolver, &w.node.server, w.node.port)
                .await
                .unwrap_err()
                .to_string()
                .contains("DNS")
        );
    }
}

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
        let mut ips = self
            .resolver
            .resolve_addresses(&self.node.server)
            .await
            .map_err(|_| anyhow::anyhow!("Subscription proxy server DNS lookup failed"))?;
        anyhow::ensure!(
            !ips.is_empty(),
            "Subscription proxy server DNS returned no address"
        );
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
        if let Some(ws) = t.ws.as_mut() {
            ws.host.get_or_insert_with(|| {
                super::transport::format_host_port(&original.server, original.port)
            });
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
    }
}

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;

use super::connector::{BoxProxyStream, Outbound, SharedOutbound};
use crate::router::{RouteAction, Router};

// Context is scoped to one accepted connection, survives executor migration,
// and is never inherited by detached DNS/health tasks or read from HTTP headers.
tokio::task_local! { static CLIENT_SOURCE: Option<std::net::IpAddr>; }
tokio::task_local! { static TUN_DESTINATION: (Option<std::net::IpAddr>, bool); }
pub(crate) fn tun_destination() -> (Option<std::net::IpAddr>, bool) {
    TUN_DESTINATION
        .try_with(|context| *context)
        .unwrap_or((None, false))
}
pub(crate) async fn with_tun_destination<F: Future>(
    ip: Option<std::net::IpAddr>,
    domestic: bool,
    future: F,
) -> F::Output {
    TUN_DESTINATION.scope((ip, domestic), future).await
}
pub(crate) fn client_source() -> Option<std::net::IpAddr> {
    CLIENT_SOURCE.try_with(|ip| *ip).ok().flatten()
}
pub(crate) async fn with_client_source<F: Future>(
    ip: Option<std::net::IpAddr>,
    future: F,
) -> F::Output {
    let ip = ip.map(|ip| match ip {
        std::net::IpAddr::V6(v6) => v6.to_ipv4_mapped().map(std::net::IpAddr::V4).unwrap_or(ip),
        _ => ip,
    });
    CLIENT_SOURCE.scope(ip, future).await
}

/// Shared immutable rule snapshots for long-lived listeners. Never hold the
/// lock across an await; accepted streams retain their own snapshot.
#[derive(Clone)]
pub struct RoutingState(Arc<std::sync::RwLock<Option<Arc<Router>>>>);

impl RoutingState {
    pub fn new(router: Option<Arc<Router>>) -> Self {
        Self(Arc::new(std::sync::RwLock::new(router)))
    }

    pub fn snapshot(&self) -> Option<Arc<Router>> {
        self.0.read().unwrap().clone()
    }

    pub fn replace(&self, router: Option<Arc<Router>>) {
        *self.0.write().unwrap() = router;
    }
}

/// An outbound that routes connections through a Router, dispatching to
/// either a proxy outbound, direct outbound, or rejecting the connection.
pub struct RoutingOutbound {
    router: Arc<Router>,
    proxy_outbound: SharedOutbound,
    direct_outbound: SharedOutbound,
}

impl RoutingOutbound {
    pub fn new(router: Arc<Router>, proxy_outbound: SharedOutbound) -> Self {
        Self {
            router,
            proxy_outbound,
            direct_outbound: SharedOutbound::direct(),
        }
    }
}

impl Outbound for RoutingOutbound {
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
        // Own the host: the returned future's lifetime is tied to `&self`,
        // so it cannot borrow the caller's `host` string.
        let host = host.to_string();
        Box::pin(async move {
            let action = self.router.route_resolving(&host, port).await;
            tracing::debug!("Route: {}:{} -> {:?}", host, port, action);

            match action {
                RouteAction::Proxy => self.proxy_outbound.0.connect(&host, port).await,
                RouteAction::Direct => self.direct_outbound.0.connect(&host, port).await,
                RouteAction::Reject => {
                    anyhow::bail!("Connection rejected by routing rule: {}:{}", host, port)
                }
            }
        })
    }

    fn connect_datagram(
        &self,
        host: &str,
        port: u16,
    ) -> super::datagram::DatagramFuture<'_, super::datagram::BoxDatagram> {
        let host = host.to_owned();
        Box::pin(async move {
            match self.router.route_resolving(&host, port).await {
                RouteAction::Direct => self.direct_outbound.0.connect_datagram(&host, port).await,
                RouteAction::Proxy => self.proxy_outbound.0.connect_datagram(&host, port).await,
                RouteAction::Reject => anyhow::bail!("UDP rejected by routing rule"),
            }
        })
    }

    fn routing_policy(&self) -> Option<Arc<Router>> {
        Some(self.router.clone())
    }
    fn is_direct(&self) -> bool {
        self.proxy_outbound.0.is_direct()
    }

    fn name(&self) -> &str {
        "routing"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::{MatchRule, RuleSet};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn routing_updates_are_shared_but_old_snapshots_remain_valid() {
        let first = Arc::new(Router::new(RuleSet::new()));
        let state = RoutingState::new(Some(first.clone()));
        let reader = state.clone();
        let snapshot = reader.snapshot().unwrap();
        state.replace(None);
        assert!(reader.snapshot().is_none());
        assert!(Arc::ptr_eq(&snapshot, &first));
        let second = Arc::new(Router::new(RuleSet::new()));
        state.replace(Some(second.clone()));
        assert!(Arc::ptr_eq(&reader.snapshot().unwrap(), &second));
    }

    #[tokio::test]
    async fn test_routing_outbound_direct() {
        // Echo server
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _echo = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 5];
            s.read_exact(&mut buf).await.unwrap();
            s.write_all(&buf).await.unwrap();
        });

        // Route everything direct
        let mut rules = RuleSet::new();
        rules.set_default(RouteAction::Direct);
        let router = Arc::new(Router::new(rules));

        let outbound = RoutingOutbound::new(router, SharedOutbound::direct());
        let mut stream = outbound
            .connect(&addr.ip().to_string(), addr.port())
            .await
            .unwrap();

        stream.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 5];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");
    }

    #[tokio::test]
    async fn test_routing_outbound_reject() {
        let mut rules = RuleSet::new();
        rules.add_rule(MatchRule::DomainKeyword("ads".into()), RouteAction::Reject);
        let router = Arc::new(Router::new(rules));

        let outbound = RoutingOutbound::new(router, SharedOutbound::direct());
        let result = outbound.connect("ads.example.com", 80).await;
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.to_string().contains("rejected"));
    }
}

#[cfg(test)]
mod device_ingress_tests {
    use super::*;
    use crate::proxy::{ProxyStats, http::HttpProxyServer, socks5::Socks5Server};
    use crate::{RoutingRule, rule_mode_ruleset};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[tokio::test]
    async fn actual_http_socks_and_udp_use_socket_source_instead_of_destination_only() {
        let target = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_addr = target.local_addr().unwrap();
        let scoped = RoutingRule {
            source: "127.0.0.1/32".into(),
            name: String::new(),
            group: String::new(),
            rule_type: "final".into(),
            pattern: "*".into(),
            target: "reject".into(),
            enabled: true,
            priority: 0,
        };
        let policy = Arc::new(Router::new(rule_mode_ruleset(&[scoped])));
        let outbound = SharedOutbound(Arc::new(RoutingOutbound::new(
            policy,
            SharedOutbound::direct(),
        )));
        let (stop, rx) = tokio::sync::watch::channel(false);
        let http = HttpProxyServer::bind("127.0.0.1", 0, outbound.clone(), ProxyStats::default())
            .await
            .unwrap();
        let port = http.local_port();
        let http_task = tokio::spawn(http.run(rx.clone()));
        let mut client = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        client
            .write_all(
                format!("CONNECT {target_addr} HTTP/1.1\r\nHost: {target_addr}\r\n\r\n").as_bytes(),
            )
            .await
            .unwrap();
        let mut response = [0u8; 128];
        let n = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            client.read(&mut response),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(String::from_utf8_lossy(&response[..n]).starts_with("HTTP/1.1 502"));
        let socks = Socks5Server::bind("127.0.0.1", 0, outbound.clone(), ProxyStats::default())
            .await
            .unwrap();
        let port = socks.local_port();
        let socks_task = tokio::spawn(socks.run(rx));
        let mut client = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        client.write_all(&[5, 1, 0]).await.unwrap();
        let mut hello = [0; 2];
        client.read_exact(&mut hello).await.unwrap();
        assert_eq!(hello, [5, 0]);
        let mut request = vec![5, 1, 0, 1, 127, 0, 0, 1];
        request.extend(target_addr.port().to_be_bytes());
        client.write_all(&request).await.unwrap();
        let mut response = [0; 10];
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            client.read_exact(&mut response),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response[1], 4);
        with_client_source(Some("127.0.0.1".parse().unwrap()), async {
            assert!(outbound.0.connect_datagram("127.0.0.1", 53).await.is_err());
        })
        .await;
        assert!(outbound.0.connect_datagram("127.0.0.1", 53).await.is_ok());
        stop.send(true).unwrap();
        http_task.await.unwrap().unwrap();
        socks_task.await.unwrap().unwrap();
    }
}

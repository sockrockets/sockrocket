use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;

use super::connector::{BoxProxyStream, Outbound, SharedOutbound};
use crate::router::{RouteAction, Router};

// Context is scoped to one accepted connection, survives executor migration,
// and is never inherited by detached DNS/health tasks or read from HTTP headers.
tokio::task_local! { static CLIENT_SOURCE: Option<std::net::IpAddr>; }
tokio::task_local! { static TUN_DESTINATION: (Option<std::net::IpAddr>, bool); }
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TunFamily {
    V4,
    V6,
}

impl TunFamily {
    pub(crate) fn matches(self, ip: IpAddr) -> bool {
        matches!(
            (self, ip),
            (Self::V4, IpAddr::V4(_)) | (Self::V6, IpAddr::V6(_))
        )
    }
}

tokio::task_local! { static TUN_FAMILY: Option<TunFamily>; }
pub(crate) fn tun_family() -> Option<TunFamily> {
    TUN_FAMILY.try_with(|family| *family).ok().flatten()
}
// Return the Tokio scope directly: an async wrapper retains both its input
// future and scoped await state, multiplying large TUN frames at each layer.
pub(crate) fn with_tun_family<F: Future>(
    destination: Option<IpAddr>,
    future: F,
) -> impl Future<Output = F::Output> {
    let family = match destination {
        Some(IpAddr::V6(_)) => Some(TunFamily::V6),
        Some(IpAddr::V4(ip)) if crate::dns::fakeip::is_fake_ip(ip) => Some(TunFamily::V4),
        _ => None,
    };
    TUN_FAMILY.scope(family, future)
}
pub(crate) fn tun_destination() -> (Option<std::net::IpAddr>, bool) {
    TUN_DESTINATION
        .try_with(|context| *context)
        .unwrap_or((None, false))
}
pub(crate) fn with_tun_destination<F: Future>(
    ip: Option<std::net::IpAddr>,
    domestic: bool,
    future: F,
) -> impl Future<Output = F::Output> {
    TUN_DESTINATION.scope((ip, domestic), future)
}
pub(crate) fn client_source() -> Option<std::net::IpAddr> {
    CLIENT_SOURCE.try_with(|ip| *ip).ok().flatten()
}
pub(crate) fn with_client_source<F: Future>(
    ip: Option<std::net::IpAddr>,
    future: F,
) -> impl Future<Output = F::Output> {
    let ip = ip.map(|ip| match ip {
        std::net::IpAddr::V6(v6) => v6.to_ipv4_mapped().map(std::net::IpAddr::V4).unwrap_or(ip),
        _ => ip,
    });
    CLIENT_SOURCE.scope(ip, future)
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
    groups: std::collections::HashMap<String, SharedOutbound>,
    nodes: std::collections::HashMap<String, SharedOutbound>,
}

impl RoutingOutbound {
    pub fn new(router: Arc<Router>, proxy_outbound: SharedOutbound) -> Self {
        Self {
            router,
            groups: Default::default(),
            nodes: Default::default(),
            proxy_outbound,
            direct_outbound: SharedOutbound::direct(),
        }
    }
}

impl RoutingOutbound {
    #[cfg(test)]
    pub(super) fn with_test_direct(mut self, direct: SharedOutbound) -> Self {
        self.direct_outbound = direct;
        self
    }

    pub fn with_groups(
        mut self,
        groups: &[crate::config::model::ProxyGroupConfig],
        nodes: &[crate::Node],
    ) -> Self {
        self.groups = super::groups::build_groups(groups, nodes);
        self.nodes = super::groups::pinned_nodes(nodes);
        self
    }
    fn group(&self, name: &str) -> Result<&SharedOutbound> {
        self.groups
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Routing proxy group does not exist"))
    }
    fn node(&self, id: &str) -> Result<&SharedOutbound> {
        self.nodes
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("Routing node does not exist"))
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
            let decision = self.router.decision_resolving(&host, port).await;
            tracing::debug!("Route: {}:{} -> {:?}", host, port, decision.action);

            match decision.action {
                RouteAction::Proxy => {
                    if let Some(target) = crate::traffic::active_traffic_target() {
                        crate::traffic::note_traffic(target);
                    }
                    self.proxy_outbound.0.connect(&host, port).await
                }
                RouteAction::Group(name) => self.group(&name)?.0.connect(&host, port).await,
                RouteAction::Node(id) => self.node(&id)?.0.connect(&host, port).await,
                RouteAction::Direct => {
                    crate::traffic::note_traffic(crate::traffic::direct_traffic_target());
                    if decision.direct_ips.is_empty() {
                        self.direct_outbound.0.connect(&host, port).await
                    } else {
                        self.direct_outbound
                            .0
                            .connect_ips(&decision.direct_ips, port)
                            .await
                    }
                }
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
            let decision = self.router.decision_resolving(&host, port).await;
            match decision.action {
                RouteAction::Direct => {
                    crate::traffic::note_traffic(crate::traffic::direct_traffic_target());
                    let Some((last, earlier)) = decision.direct_ips.split_last() else {
                        return self.direct_outbound.0.connect_datagram(&host, port).await;
                    };
                    // Only association setup may advance to the next address.
                    // A successful socket retains one peer for its entire lifetime.
                    tokio::time::timeout(super::connector::UPSTREAM_CONNECT_TIMEOUT, async {
                        for ip in earlier {
                            if let Ok(socket) = self
                                .direct_outbound
                                .0
                                .connect_datagram(&ip.to_string(), port)
                                .await
                            {
                                return Ok(socket);
                            }
                        }
                        self.direct_outbound
                            .0
                            .connect_datagram(&last.to_string(), port)
                            .await
                    })
                    .await?
                }
                RouteAction::Proxy => {
                    if let Some(target) = crate::traffic::active_traffic_target() {
                        crate::traffic::note_traffic(target);
                    }
                    self.proxy_outbound.0.connect_datagram(&host, port).await
                }
                RouteAction::Group(name) => {
                    self.group(&name)?.0.connect_datagram(&host, port).await
                }
                RouteAction::Node(id) => self.node(&id)?.0.connect_datagram(&host, port).await,
                RouteAction::Reject => anyhow::bail!("UDP rejected by routing rule"),
            }
        })
    }

    fn default_proxy(&self) -> Option<SharedOutbound> {
        Some(self.proxy_outbound.clone())
    }

    fn with_default_proxy(&self, proxy: SharedOutbound) -> Option<SharedOutbound> {
        Some(SharedOutbound(Arc::new(Self {
            router: self.router.clone(),
            proxy_outbound: proxy,
            direct_outbound: self.direct_outbound.clone(),
            groups: self.groups.clone(),
            nodes: self.nodes.clone(),
        })))
    }
    fn routing_policy(&self) -> Option<Arc<Router>> {
        Some(self.router.clone())
    }
    fn proxy_transport(&self) -> Option<SharedOutbound> {
        Some(self.proxy_outbound.clone())
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
    async fn failed_direct_candidates_never_retry_other_answers_or_the_hostname() {
        use super::super::datagram::{BoxDatagram, DatagramFuture};
        use crate::dns::{DnsConfig, DnsResolver};

        #[derive(Default)]
        struct Refusals(std::sync::Mutex<Vec<String>>);
        impl Refusals {
            fn refuse<T: 'static>(&self, host: &str) -> DatagramFuture<'_, T> {
                self.0.lock().unwrap().push(host.to_owned());
                let message = format!("unreachable {host}");
                Box::pin(async move { anyhow::bail!(message) })
            }
        }
        impl Outbound for Refusals {
            fn connect(&self, host: &str, _: u16) -> DatagramFuture<'_, BoxProxyStream> {
                self.refuse(host)
            }
            fn connect_datagram(&self, host: &str, _: u16) -> DatagramFuture<'_, BoxDatagram> {
                self.refuse(host)
            }
            fn name(&self) -> &str {
                "refusals"
            }
        }

        let direct = Arc::new(Refusals::default());
        let proxy = Arc::new(Refusals::default());
        let mut rules = RuleSet::new();
        rules.add_rule(
            MatchRule::IpCidr {
                addr: "192.0.2.0".parse().unwrap(),
                prefix_len: 24,
            },
            RouteAction::Direct,
        );
        let outbound = RoutingOutbound::new(
            Arc::new(Router::new(rules)),
            SharedOutbound(proxy.clone()),
        )
        .with_test_direct(SharedOutbound(direct.clone()));
        let host = "mixed.example";
        let mut config = DnsConfig::default();
        config.hosts.insert(
            host.into(),
            ["198.51.100.1", "192.0.2.2", "203.0.113.3", "192.0.2.4"]
                .map(|ip| ip.parse().unwrap())
                .to_vec(),
        );
        DnsResolver::with_config(config)
            .scope(async {
                let tcp = outbound.connect(host, 443).await.err().unwrap();
                let udp = outbound.connect_datagram(host, 443).await.err().unwrap();
                assert_eq!(tcp.to_string(), "unreachable 192.0.2.4");
                assert_eq!(udp.to_string(), tcp.to_string());
            })
            .await;
        assert_eq!(
            *direct.0.lock().unwrap(),
            ["192.0.2.2", "192.0.2.4", "192.0.2.2", "192.0.2.4"]
        );
        assert!(proxy.0.lock().unwrap().is_empty());
    }

    struct Tagged(&'static str);
    impl Outbound for Tagged {
        fn connect(
            &self,
            _: &str,
            _: u16,
        ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
            Box::pin(async move { anyhow::bail!(self.0) })
        }
        fn connect_datagram(
            &self,
            _: &str,
            _: u16,
        ) -> super::super::datagram::DatagramFuture<'_, super::super::datagram::BoxDatagram>
        {
            Box::pin(async move { anyhow::bail!(self.0) })
        }
        fn name(&self) -> &str {
            "tagged"
        }
    }
    #[tokio::test]
    async fn group_tcp_udp_and_device_policy_use_the_same_exit_and_fail_closed() {
        let mut rules = crate::parse_batch_rules(
            "DOMAIN,example.invalid,group:Video\nSOURCE,192.0.2.9/32,MATCH,group:Device",
            "proxy",
        )
        .rules;
        assert_eq!(rules.len(), 2);
        let router = Arc::new(Router::new(crate::rule_mode_ruleset(&rules)));
        let mut outbound = RoutingOutbound::new(router, SharedOutbound(Arc::new(Tagged("global"))));
        outbound
            .groups
            .insert("Video".into(), SharedOutbound(Arc::new(Tagged("video"))));
        outbound
            .groups
            .insert("Device".into(), SharedOutbound(Arc::new(Tagged("device"))));
        assert_eq!(
            outbound
                .connect("example.invalid", 443)
                .await
                .err()
                .unwrap()
                .to_string(),
            "video"
        );
        assert_eq!(
            outbound
                .connect_datagram("example.invalid", 443)
                .await
                .err()
                .unwrap()
                .to_string(),
            "video"
        );
        with_client_source(Some("192.0.2.9".parse().unwrap()), async {
            assert_eq!(
                outbound
                    .connect("example.invalid", 443)
                    .await
                    .err()
                    .unwrap()
                    .to_string(),
                "device"
            );
            assert_eq!(
                outbound
                    .connect("unlisted.invalid", 443)
                    .await
                    .err()
                    .unwrap()
                    .to_string(),
                "device"
            );
            assert_eq!(
                outbound
                    .connect_datagram("example.invalid", 443)
                    .await
                    .err()
                    .unwrap()
                    .to_string(),
                "device"
            );
        })
        .await;
        outbound.groups.clear();
        assert!(
            outbound
                .connect("example.invalid", 443)
                .await
                .err()
                .unwrap()
                .to_string()
                .contains("does not exist")
        );
        assert!(
            outbound
                .connect_datagram("example.invalid", 443)
                .await
                .err()
                .unwrap()
                .to_string()
                .contains("does not exist")
        );
        let text = crate::format_rules_export(&rules);
        let roundtrip = crate::parse_batch_rules(&text, "proxy");
        assert!(roundtrip.errors.is_empty());
        rules.sort_by(|a, b| a.source.cmp(&b.source));
        assert!(roundtrip.rules.iter().any(|r| r.target == "group:Video"));
        assert!(
            roundtrip
                .rules
                .iter()
                .any(|r| r.target == "group:Device" && !r.source.is_empty())
        );
    }

    #[tokio::test]
    async fn a_missing_pinned_node_does_not_use_the_global_node() {
        let id = "ab".repeat(32);
        let rules =
            crate::parse_batch_rules(&format!("DOMAIN,example.invalid,node:{id}"), "proxy").rules;
        assert_eq!(rules[0].target, format!("node:{id}"));
        let router = Arc::new(Router::new(crate::rule_mode_ruleset(&rules)));
        let outbound = RoutingOutbound::new(router, SharedOutbound(Arc::new(Tagged("global"))));
        let err = outbound
            .connect("example.invalid", 443)
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("does not exist"), "{err}");
        assert!(!err.contains("global"), "{err}");
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
            .write_all(b"CONNECT 203.0.113.10:443 HTTP/1.1\r\nHost: 203.0.113.10:443\r\n\r\n")
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
        let mut request = vec![5, 1, 0, 1, 203, 0, 113, 10];
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
            assert!(
                outbound
                    .0
                    .connect_datagram("203.0.113.10", 53)
                    .await
                    .is_err()
            );
        })
        .await;
        assert!(
            outbound
                .0
                .connect_datagram("203.0.113.10", 53)
                .await
                .is_ok()
        );
        stop.send(true).unwrap();
        http_task.await.unwrap().unwrap();
        socks_task.await.unwrap().unwrap();
    }
}

#[cfg(test)]
mod async_context_memory_review {
    use super::*;
    #[tokio::test]
    async fn nested_context_scopes_do_not_multiply_the_inner_future_frame() {
        let future = async move {
            let payload = [0x5au8; 8192];
            tokio::task::yield_now().await;
            std::hint::black_box(payload);
            assert_eq!(client_source(), Some("192.0.2.4".parse().unwrap()));
            assert_eq!(
                tun_destination(),
                (Some("198.51.100.7".parse().unwrap()), true)
            );
            assert_eq!(tun_family(), Some(TunFamily::V6));
        };
        let inner = std::mem::size_of_val(&future);
        let scoped = crate::traffic::with_traffic_scope(with_client_source(
            Some("::ffff:192.0.2.4".parse().unwrap()),
            with_tun_destination(
                Some("198.51.100.7".parse().unwrap()),
                true,
                with_tun_family(Some("2001:db8::1".parse().unwrap()), future),
            ),
        ));
        let actual = std::mem::size_of_val(&scoped);
        println!("context_future_bytes inner={inner} wrapped={actual}");
        assert!(
            actual <= inner + 512,
            "nested context frame grew from {inner} to {actual} bytes"
        );
        scoped.await;
        assert_eq!(client_source(), None);
        assert_eq!(tun_destination(), (None, false));
        assert_eq!(tun_family(), None);
    }
}

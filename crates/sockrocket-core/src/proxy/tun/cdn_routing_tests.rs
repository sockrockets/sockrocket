use super::*;
use crate::config::dns::AddressStrategy;
use crate::dns::wire::{build_dns_query, parse_dns_response};
use crate::dns::{
    DnsConfig, DnsServer, QTYPE_A, answer_raw_dns_query, build_dns_response_with_ttl,
};
use crate::proxy::connector::{BoxProxyStream, Outbound};
use crate::proxy::datagram::{BoxDatagram, DatagramFuture};
use crate::proxy::routing::RoutingOutbound;
use std::collections::HashMap;
use tokio::sync::mpsc;

type Answers = HashMap<&'static str, Vec<IpAddr>>;

fn answers(records: impl Fn(Ipv4Addr) -> Vec<IpAddr>) -> Answers {
    DEEPSEEK_HOSTS
        .into_iter()
        .map(|(host, cn)| (host, records(cn)))
        .collect()
}

/// Replace only the transport to the DNS upstream; queries and replies retain
/// the normal DNS-over-TCP framing, parser, cache and client answer policy.
struct Upstream {
    primary: Answers,
    domestic: Answers,
}

impl Outbound for Upstream {
    fn name(&self) -> &str {
        "fixture DNS"
    }

    fn connect(&self, host: &str, port: u16) -> DatagramFuture<'_, BoxProxyStream> {
        let answers = match (host, port) {
            ("192.0.2.53", 53) => self.primary.clone(),
            ("192.0.2.54", 53) => self.domestic.clone(),
            other => panic!("unexpected DNS endpoint {other:?}"),
        };
        Box::pin(async move {
            let (client, mut server) = tokio::io::duplex(4096);
            tokio::spawn(async move {
                while let Ok(len) = server.read_u16().await {
                    let mut query = vec![0; usize::from(len)];
                    server.read_exact(&mut query).await.unwrap();
                    let (domain, qtype) = crate::dns::parse_dns_query(&query).unwrap();
                    assert_eq!(qtype, QTYPE_A);
                    let records = answers
                        .get(domain.as_str())
                        .unwrap_or_else(|| panic!("unexpected DNS question {domain}"));
                    let reply = build_dns_response_with_ttl(&query, records, 60).unwrap();
                    server.write_u16(reply.len() as u16).await.unwrap();
                    server.write_all(&reply).await.unwrap();
                }
            });
            Ok(Box::new(client) as BoxProxyStream)
        })
    }
}

fn resolver(
    policy: DnsConfig,
    primary: Answers,
    domestic: Option<Answers>,
    pool: &Arc<FakeIpPool>,
    geoip: &Arc<crate::router::GeoIpDb>,
) -> Arc<DnsResolver> {
    let mut resolver = DnsResolver::with_config(DnsConfig {
        servers: vec![DnsServer::ProxyTcp(([192, 0, 2, 53], 53).into())],
        fallback: domestic
            .as_ref()
            .map(|_| DnsServer::ProxyTcp(([192, 0, 2, 54], 53).into()))
            .into_iter()
            .collect(),
        strategy: AddressStrategy::Ipv4Only,
        ..policy
    });
    resolver.set_fakeip(pool.clone());
    resolver.set_geoip(geoip.clone());
    resolver.set_outbound(SharedOutbound(Arc::new(Upstream {
        primary,
        domestic: domestic.unwrap_or_default(),
    })));
    Arc::new(resolver)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Transport {
    Tcp,
    Udp,
}

type Dial = (Transport, &'static str, String, u16);
struct Endpoint(&'static str, mpsc::UnboundedSender<Dial>);

impl Endpoint {
    fn record<T: 'static>(
        &self,
        transport: Transport,
        host: &str,
        port: u16,
    ) -> DatagramFuture<'_, T> {
        self.1
            .send((transport, self.0, host.to_owned(), port))
            .unwrap();
        Box::pin(std::future::pending())
    }
}

impl Outbound for Endpoint {
    fn name(&self) -> &str {
        self.0
    }
    fn connect(&self, host: &str, port: u16) -> DatagramFuture<'_, BoxProxyStream> {
        self.record(Transport::Tcp, host, port)
    }
    fn connect_datagram(&self, host: &str, port: u16) -> DatagramFuture<'_, BoxDatagram> {
        self.record(Transport::Udp, host, port)
    }
}

async fn assert_tun_dial(
    resolver: &Arc<DnsResolver>,
    pool: &Arc<FakeIpPool>,
    router: &Arc<Router>,
    target: Ipv4Addr,
    expected_exit: &'static str,
    expected_host: &str,
) {
    for transport in [Transport::Tcp, Transport::Udp] {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let endpoint = |name| SharedOutbound(Arc::new(Endpoint(name, tx.clone())));
        let outbound = RoutingOutbound::new(router.clone(), endpoint("proxy"))
            .with_test_direct(endpoint("direct"));
        let (mut device, tun) = tokio::io::duplex(4096);
        let mut stack = ipstack::IpStack::new(Default::default(), tun);
        let ip = etherparse::PacketBuilder::ipv4([192, 168, 50, 2], target.octets(), 64);
        let mut packet = Vec::new();
        match transport {
            Transport::Tcp => ip
                .tcp(53000, 443, 1000, 32768)
                .syn()
                .write(&mut packet, &[]),
            Transport::Udp => ip.udp(53000, 443).write(&mut packet, b"probe"),
        }
        .unwrap();
        device.write_all(&packet).await.unwrap();
        let stream = stack.accept().await.unwrap();
        let handle = handle_tun_stream(
            stream,
            SharedOutbound(Arc::new(outbound)),
            resolver.clone(),
            Some(pool.clone()),
            Some(router.clone()),
            Default::default(),
            (Arc::new(AtomicUsize::new(0)), 8),
        );
        tokio::select! {
            result = resolver.scope(handle) => panic!("TUN stream ended before dialing: {result:?}"),
            dial = rx.recv() => {
                assert_eq!(dial.unwrap(), (transport, expected_exit, expected_host.to_owned(), 443));
            }
        }
    }
}

const DEEPSEEK_HOSTS: [(&str, Ipv4Addr); 2] = [
    ("chat.deepseek.com", Ipv4Addr::new(116, 205, 40, 113)),
    ("api.deepseek.com", Ipv4Addr::new(112, 84, 131, 227)),
];
const OVERSEAS_CDN: Ipv4Addr = Ipv4Addr::new(3, 173, 21, 63);

fn token_from_reply(reply: &[u8], pool: &FakeIpPool, host: &str) -> Ipv4Addr {
    let addresses = parse_dns_response(reply).unwrap();
    let [token] = addresses.as_slice() else {
        panic!("expected one fake address for {host}, got {addresses:?}")
    };
    let token = crate::dns::fakeip::fake_ipv4(*token).expect("expected a TUN token");
    assert_eq!(pool.lookup(token).as_deref(), Some(host));
    token
}

#[tokio::test]
async fn deepseek_dns_answers_and_tun_connections_follow_the_cdn_address() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let geoip = Arc::new(crate::china_geoip_db());
        let router = Arc::new(Router::new(crate::rule_mode_ruleset(&[])).with_geoip(geoip.clone()));
        let foreign = OVERSEAS_CDN;
        let pool = FakeIpPool::new();
        let overseas = resolver(
            Default::default(),
            answers(|_| vec![foreign.into()]),
            None,
            &pool,
            &geoip,
        );
        let local = resolver(
            Default::default(),
            answers(|cn| vec![foreign.into(), cn.into()]),
            None,
            &pool,
            &geoip,
        );
        for (host, cn) in DEEPSEEK_HOSTS {
            let query = build_dns_query(host, QTYPE_A);
            let reply = answer_raw_dns_query(&overseas, &query).await;
            let token = token_from_reply(&reply, &pool, host);
            assert_tun_dial(&overseas, &pool, &router, token, "proxy", host).await;

            // An application can retain its original token while the CDN
            // answer changes. The first answer is deliberately outside CN.
            let mixed = [IpAddr::V4(foreign), IpAddr::V4(cn)];
            let reply = answer_raw_dns_query(&local, &query).await;
            assert_eq!(parse_dns_response(&reply).unwrap(), mixed);
            assert_eq!(pool.lookup_domestic(cn.into()).as_deref(), Some(host));
            assert_eq!(pool.lookup_domestic(foreign.into()), None);
            assert_tun_dial(&local, &pool, &router, token, "direct", &cn.to_string()).await;
            assert_tun_dial(&local, &pool, &router, cn, "direct", &cn.to_string()).await;
            assert_tun_dial(
                &local,
                &pool,
                &router,
                foreign,
                "proxy",
                &foreign.to_string(),
            )
            .await;
        }
    })
    .await
    .expect("in-memory DNS and TUN fixtures should finish");
}

#[tokio::test]
async fn automatic_cn_preference_routes_deepseek_without_domain_rules() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let geoip = Arc::new(crate::china_geoip_db());
        let router = Arc::new(Router::new(crate::rule_mode_ruleset(&[])).with_geoip(geoip.clone()));
        let pool = FakeIpPool::new();
        let overseas = answers(|_| vec![OVERSEAS_CDN.into()]);
        let domestic = answers(|cn| vec![OVERSEAS_CDN.into(), cn.into()]);
        let policy = DnsConfig {
            prefer_cn_answers: true,
            ..Default::default()
        };
        let no_cn = resolver(
            policy.clone(),
            overseas.clone(),
            Some(overseas.clone()),
            &pool,
            &geoip,
        );
        let local = resolver(policy, overseas, Some(domestic), &pool, &geoip);
        for (host, cn) in DEEPSEEK_HOSTS {
            let query = build_dns_query(host, QTYPE_A);
            let reply = answer_raw_dns_query(&no_cn, &query).await;
            let token = token_from_reply(&reply, &pool, host);
            assert_tun_dial(&no_cn, &pool, &router, token, "proxy", host).await;

            // Both servers get the same hostname. Only the configured
            // domestic upstream provides an address that matches GeoIP CN.
            let reply = answer_raw_dns_query(&local, &query).await;
            assert_eq!(parse_dns_response(&reply).unwrap(), [IpAddr::V4(cn)]);
            assert_eq!(pool.lookup_domestic(cn.into()).as_deref(), Some(host));
            assert_tun_dial(&local, &pool, &router, cn, "direct", &cn.to_string()).await;
            assert_tun_dial(&local, &pool, &router, token, "direct", &cn.to_string()).await;
        }
        // The resolver and token pool retain both hostnames. Revisit each
        // after both entries exist to catch cross-host cache reuse.
        for (host, cn) in DEEPSEEK_HOSTS.into_iter().rev() {
            let reply = answer_raw_dns_query(&local, &build_dns_query(host, QTYPE_A)).await;
            assert_eq!(parse_dns_response(&reply).unwrap(), [IpAddr::V4(cn)]);
        }
    })
    .await
    .expect("in-memory DNS preference and TUN fixtures should finish");
}

#[tokio::test]
async fn explicit_proxy_and_automatic_direct_share_a_cdn_without_sharing_identity() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let (proxy, direct) = ("api.deepseek.com", "chat.deepseek.com");
        let shared = Ipv4Addr::new(116, 205, 40, 113);
        let app = crate::AppConfig {
            rules: serde_json::from_value(serde_json::json!([{
                "rule_type": "domain", "pattern": proxy, "target": "proxy"
            }]))
            .unwrap(),
            ..Default::default()
        };
        let policy = crate::config::dns::DnsSettings::default()
            .resolver_for_app(&app)
            .unwrap();
        assert!(policy.prefer_cn_answers);
        let geoip = Arc::new(crate::china_geoip_db());
        let router =
            Arc::new(Router::new(crate::rule_mode_ruleset(&app.rules)).with_geoip(geoip.clone()));
        for order in [[proxy, direct], [direct, proxy]] {
            let pool = FakeIpPool::new();
            let primary = [
                (proxy, vec![shared.into()]),
                (direct, vec![OVERSEAS_CDN.into()]),
            ]
            .into();
            let domestic = [(direct, vec![shared.into()])].into();
            let resolver = resolver(policy.clone(), primary, Some(domestic), &pool, &geoip);
            let mut replies = HashMap::new();
            for host in order {
                let query = build_dns_query(host, QTYPE_A);
                replies.insert(host, answer_raw_dns_query(&resolver, &query).await);
            }

            let token = token_from_reply(&replies[proxy], &pool, proxy);
            assert_eq!(
                parse_dns_response(&replies[direct]).unwrap(),
                [IpAddr::V4(shared)]
            );
            assert_eq!(pool.lookup_domestic(shared.into()).as_deref(), Some(direct));
            assert_tun_dial(&resolver, &pool, &router, token, "proxy", proxy).await;
            assert_tun_dial(
                &resolver,
                &pool,
                &router,
                shared,
                "direct",
                &shared.to_string(),
            )
            .await;
        }
    })
    .await
    .expect("both query orders should preserve the per-domain exits");
}

use super::*;
use crate::config::dns::AddressStrategy;
use crate::dns::wire::{build_dns_query, parse_dns_response};
use crate::dns::{
    DnsConfig, DnsServer, QTYPE_A, answer_raw_dns_query, build_dns_response_with_ttl,
};
use crate::proxy::connector::{BoxProxyStream, Outbound};
use crate::proxy::datagram::{BoxDatagram, DatagramFuture};
use crate::proxy::routing::RoutingOutbound;
use tokio::sync::mpsc;

/// Replace only the transport to the DNS upstream; queries and replies retain
/// the normal DNS-over-TCP framing, parser, cache and client answer policy.
struct Upstream {
    primary: Vec<IpAddr>,
    domestic: Vec<IpAddr>,
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
                    let (_, qtype) = crate::dns::parse_dns_query(&query).unwrap();
                    assert_eq!(qtype, QTYPE_A);
                    let reply = build_dns_response_with_ttl(&query, &answers, 60).unwrap();
                    server.write_u16(reply.len() as u16).await.unwrap();
                    server.write_all(&reply).await.unwrap();
                }
            });
            Ok(Box::new(client) as BoxProxyStream)
        })
    }
}

fn resolver(
    primary: &[IpAddr],
    domestic: Option<&[IpAddr]>,
    pool: &Arc<FakeIpPool>,
    geoip: &Arc<crate::router::GeoIpDb>,
) -> Arc<DnsResolver> {
    let mut resolver = DnsResolver::with_config(DnsConfig {
        servers: vec![DnsServer::ProxyTcp(([192, 0, 2, 53], 53).into())],
        fallback: domestic
            .map(|_| DnsServer::ProxyTcp(([192, 0, 2, 54], 53).into()))
            .into_iter()
            .collect(),
        prefer_cn_answers: domestic.is_some(),
        strategy: AddressStrategy::Ipv4Only,
        ..Default::default()
    });
    resolver.set_fakeip(pool.clone());
    resolver.set_geoip(geoip.clone());
    resolver.set_outbound(SharedOutbound(Arc::new(Upstream {
        primary: primary.to_vec(),
        domestic: domestic.unwrap_or_default().to_vec(),
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
        for (host, cn) in DEEPSEEK_HOSTS {
            let pool = FakeIpPool::new();
            let query = build_dns_query(host, QTYPE_A);
            let overseas = resolver(&[foreign.into()], None, &pool, &geoip);
            let reply = answer_raw_dns_query(&overseas, &query).await;
            let token = token_from_reply(&reply, &pool, host);
            assert_tun_dial(&overseas, &pool, &router, token, "proxy", host).await;

            // An application can retain its original token while the CDN
            // answer changes. The first answer is deliberately outside CN.
            let mixed = [foreign.into(), cn.into()];
            let local = resolver(&mixed, None, &pool, &geoip);
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
        let overseas = [OVERSEAS_CDN.into()];
        for (host, cn) in DEEPSEEK_HOSTS {
            let pool = FakeIpPool::new();
            let query = build_dns_query(host, QTYPE_A);
            let no_cn = resolver(&overseas, Some(&overseas), &pool, &geoip);
            let reply = answer_raw_dns_query(&no_cn, &query).await;
            let token = token_from_reply(&reply, &pool, host);
            assert_tun_dial(&no_cn, &pool, &router, token, "proxy", host).await;

            // Both servers get the same hostname. Only the configured
            // domestic upstream provides an address that matches GeoIP CN.
            let domestic = [OVERSEAS_CDN.into(), cn.into()];
            let local = resolver(&overseas, Some(&domestic), &pool, &geoip);
            let reply = answer_raw_dns_query(&local, &query).await;
            assert_eq!(parse_dns_response(&reply).unwrap(), [IpAddr::V4(cn)]);
            assert_eq!(pool.lookup_domestic(cn.into()).as_deref(), Some(host));
            assert_tun_dial(&local, &pool, &router, cn, "direct", &cn.to_string()).await;
            assert_tun_dial(&local, &pool, &router, token, "direct", &cn.to_string()).await;
        }
    })
    .await
    .expect("in-memory DNS preference and TUN fixtures should finish");
}

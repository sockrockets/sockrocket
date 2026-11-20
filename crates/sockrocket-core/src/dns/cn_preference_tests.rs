use super::*;
use crate::proxy::connector::Outbound;
use crate::proxy::datagram::DatagramFuture;
use std::sync::atomic::{AtomicUsize, Ordering};

const HOST: &str = "dual.cdn-check.com";
const OVERSEAS: &str = "2606:4700:4700::1111";
const CN: &str = "2400:3200::1";

#[derive(Clone, Copy)]
enum Answer {
    Addresses(&'static [&'static str], u32),
    Error(u8),
    Malformed,
    Silent,
    Recovers,
    RegionalPrimary,
}

struct Upstreams {
    primary: Answer,
    direct: Answer,
    primary_queries: Arc<AtomicUsize>,
    probes: Arc<AtomicUsize>,
}

impl Outbound for Upstreams {
    fn name(&self) -> &str {
        "DNS test upstream"
    }

    fn connect(&self, host: &str, _: u16) -> DatagramFuture<'_, BoxProxyStream> {
        let (answer, count) = match host {
            "192.0.2.1" => (self.primary, self.primary_queries.clone()),
            "192.0.2.2" => (self.direct, self.probes.clone()),
            _ => panic!("unexpected DNS server {host}"),
        };
        Box::pin(async move {
            let (client, mut server) = tokio::io::duplex(4096);
            tokio::spawn(async move {
                while let Ok(size) = server.read_u16().await {
                    let mut query = vec![0; usize::from(size)];
                    server.read_exact(&mut query).await.unwrap();
                    let request_index = count.fetch_add(1, Ordering::Relaxed);
                    let reply = match answer {
                        Answer::Addresses(addresses, ttl) => {
                            let ips: Vec<_> =
                                addresses.iter().map(|ip| ip.parse().unwrap()).collect();
                            build_dns_response_with_ttl(&query, &ips, ttl).unwrap()
                        }
                        Answer::Error(rcode) => build_dns_error_response(&query, rcode),
                        Answer::Malformed => {
                            // Valid CN address followed by a truncated authority TTL.
                            let mut reply =
                                build_dns_response(&query, &[CN.parse().unwrap()]).unwrap();
                            reply[9] = 1;
                            reply.extend_from_slice(&[0xc0, 12, 0, 2, 0, 1, 0]);
                            reply
                        }
                        Answer::Silent => {
                            // The caller closes the connection when the probe budget expires.
                            let _ = server.read_u8().await;
                            return;
                        }
                        Answer::RegionalPrimary => {
                            let (name, qtype) = parse_dns_query(&query).unwrap();
                            let ip = match (name.as_str() == HOST, qtype) {
                                (true, QTYPE_A) => "116.205.40.114",
                                (true, QTYPE_AAAA) => CN,
                                (false, QTYPE_A) => "3.173.21.63",
                                (false, QTYPE_AAAA) => OVERSEAS,
                                _ => panic!("unexpected query type {qtype}"),
                            };
                            build_dns_response(&query, &[ip.parse().unwrap()]).unwrap()
                        }
                        Answer::Recovers if request_index == 0 => {
                            build_dns_error_response(&query, 3)
                        }
                        Answer::Recovers => {
                            build_dns_response_with_ttl(&query, &[CN.parse().unwrap()], 20).unwrap()
                        }
                    };
                    server.write_u16(reply.len() as u16).await.unwrap();
                    server.write_all(&reply).await.unwrap();
                }
            });
            Ok(Box::new(client) as BoxProxyStream)
        })
    }
}

fn fixture(primary: Answer, direct: Answer, cache_enabled: bool) -> (DnsResolver, Arc<Upstreams>) {
    let upstreams = Arc::new(Upstreams {
        primary,
        direct,
        primary_queries: Arc::default(),
        probes: Arc::default(),
    });
    let mut resolver = DnsResolver::with_config(DnsConfig {
        servers: vec![DnsServer::ProxyTcp(([192, 0, 2, 1], 53).into())],
        fallback: vec![DnsServer::ProxyTcp(([192, 0, 2, 2], 53).into())],
        prefer_cn_answers: true,
        dual_stack_fakeip: true,
        cache_enabled,
        timeout_secs: 1,
        ..Default::default()
    });
    resolver.set_outbound(SharedOutbound(upstreams.clone()));
    (resolver, upstreams)
}

#[tokio::test(start_paused = true)]
async fn optional_timeout_returns_primary_or_fake_ipv6_and_respects_cooldown() {
    for cache in [true, false] {
        let (resolver, servers) =
            fixture(Answer::Addresses(&[OVERSEAS], 55), Answer::Silent, cache);
        let mut lan = resolver.clone();
        let pool = FakeIpPool::new();
        lan.set_fakeip(pool.clone());
        let query = build_dns_query(HOST, QTYPE_AAAA);
        for (index, policy) in [&resolver, &lan].into_iter().enumerate() {
            let start = tokio::time::Instant::now();
            let reply = answer_raw_dns_query(policy, &query).await;
            let ips = parse_dns_response(&reply).unwrap();
            assert_eq!(ips.len(), 1);
            let elapsed = start.elapsed();
            if index == 0 || !cache {
                assert!(
                    elapsed >= PROBE_BUDGET && elapsed <= PROBE_BUDGET + Duration::from_millis(1),
                    "{elapsed:?}"
                );
            } else {
                assert_eq!(elapsed, Duration::ZERO);
            }
            if index == 0 {
                assert_eq!(ips[0], OVERSEAS.parse::<IpAddr>().unwrap());
            } else {
                let token = fakeip::fake_ipv4(ips[0]).unwrap();
                assert_eq!(pool.lookup(token).as_deref(), Some(HOST));
            }
        }
        assert_eq!(
            servers.probes.load(Ordering::Relaxed),
            if cache { 1 } else { 2 }
        );
        if cache {
            // The failure marker follows the same generation as real answers.
            resolver.clear_cache().await;
            answer_raw_dns_query(&resolver, &query).await;
            assert_eq!(servers.probes.load(Ordering::Relaxed), 2);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn failed_or_empty_primary_never_queries_the_direct_resolver() {
    let query = build_dns_query(HOST, QTYPE_AAAA);
    for (answer, rcode) in [
        (Answer::Error(3), 3),
        (Answer::Error(2), 2),
        (Answer::Addresses(&[], 0), 0),
        (Answer::Silent, 2),
    ] {
        let (resolver, servers) = fixture(answer, Answer::Addresses(&[CN], 20), true);
        let reply = answer_raw_dns_query(&resolver, &query).await;
        assert_eq!(reply[3] & 15, rcode);
        assert_eq!(reply[6..8], [0, 0]);
        assert_eq!(servers.probes.load(Ordering::Relaxed), 0);
    }
}

#[tokio::test(start_paused = true)]
async fn optional_dns_failures_preserve_the_real_answer_and_ttl() {
    let query = build_dns_query(HOST, QTYPE_AAAA);
    for bad_reply in [Answer::Error(3), Answer::Malformed] {
        let (resolver, servers) = fixture(Answer::Addresses(&[OVERSEAS], 51), bad_reply, true);
        for _ in 0..2 {
            let reply = answer_raw_dns_query(&resolver, &query).await;
            assert_eq!(
                parse_dns_response(&reply).unwrap(),
                [OVERSEAS.parse::<IpAddr>().unwrap()]
            );
            // Remaining cached TTL can lose a second to rounding.
            assert!((50..=51).contains(&wire::answer_ttl(&reply).unwrap()));
        }
        assert_eq!(servers.probes.load(Ordering::Relaxed), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn prefers_only_same_family_cn_and_the_shorter_ttl() {
    let query = build_dns_query(HOST, QTYPE_AAAA);
    for (primary, direct, expected, ttl, probes) in [
        (
            Answer::Addresses(&[OVERSEAS], 41),
            Answer::Addresses(&[OVERSEAS, CN], 7),
            CN,
            7,
            1,
        ),
        (
            Answer::Addresses(&[OVERSEAS, CN], 6),
            Answer::Silent,
            CN,
            6,
            0,
        ),
        (
            Answer::Addresses(&[OVERSEAS], 17),
            Answer::Addresses(&["116.205.40.114", OVERSEAS], 8),
            OVERSEAS,
            17,
            1,
        ),
    ] {
        let (resolver, servers) = fixture(primary, direct, false);
        let reply = answer_raw_dns_query(&resolver, &query).await;
        let expected = [expected.parse::<IpAddr>().unwrap()];
        assert_eq!(parse_dns_response(&reply).unwrap(), expected);
        assert_eq!(wire::answer_ttl(&reply).unwrap(), ttl);
        assert_eq!(servers.probes.load(Ordering::Relaxed), probes);
        assert_eq!(
            resolver.resolve_with_qtype(HOST, QTYPE_AAAA).await.unwrap(),
            expected
        );
    }
}

#[tokio::test(start_paused = true)]
async fn repeated_real_aaaa_nodata_uses_the_parsed_negative_cache() {
    let (resolver, servers) = fixture(Answer::Addresses(&[], 0), Answer::Silent, true);
    for _ in 0..3 {
        assert!(
            resolver
                .resolve_with_qtype(HOST, QTYPE_AAAA)
                .await
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(servers.primary_queries.load(Ordering::Relaxed), 1);
    assert_eq!(servers.probes.load(Ordering::Relaxed), 0);
}

#[tokio::test(start_paused = true)]
async fn temporary_direct_failure_does_not_pin_real_routes_to_an_overseas_ip() {
    let (resolver, servers) = fixture(Answer::Addresses(&[OVERSEAS], 600), Answer::Recovers, true);
    assert_eq!(
        resolver.resolve_with_qtype(HOST, QTYPE_AAAA).await.unwrap(),
        [OVERSEAS.parse::<IpAddr>().unwrap()]
    );
    let key: CacheKey = (HOST.to_owned(), QTYPE_AAAA, AddressPolicy::PreferCn);
    let expired = Instant::now() - Duration::from_secs(1);
    {
        let mut cache = resolver.cache.write().await;
        let entry = cache.get_mut(&key).unwrap();
        assert!(entry.expires_at.saturating_duration_since(Instant::now()) <= PROBE_FAILURE_TTL);
        entry.expires_at = expired;
    }
    let mut marker = build_dns_query(HOST, QTYPE_AAAA);
    marker[..2].fill(0);
    marker.push(2);
    resolver
        .wire_cache
        .lock()
        .await
        .entries
        .get_mut(&marker)
        .unwrap()
        .1 = expired;

    // Stale-while-revalidate returns the last real answer while a fresh
    // direct probe runs, then subsequent requests get the recovered CN edge.
    assert_eq!(
        resolver.resolve_with_qtype(HOST, QTYPE_AAAA).await.unwrap(),
        [OVERSEAS.parse::<IpAddr>().unwrap()]
    );
    let cn = CN.parse::<IpAddr>().unwrap();
    for _ in 0..100 {
        tokio::task::yield_now().await;
        if resolver.cache.read().await.peek(&key).unwrap().addresses == [cn] {
            assert_eq!(
                resolver.resolve_with_qtype(HOST, QTYPE_AAAA).await.unwrap(),
                [cn]
            );
            assert_eq!(servers.probes.load(Ordering::Relaxed), 2);
            return;
        }
    }
    panic!("background refresh did not adopt the recovered direct answer");
}

#[tokio::test(start_paused = true)]
async fn explicit_domain_policy_avoids_only_its_own_direct_probe() {
    let (mut resolver, servers) = fixture(
        Answer::Addresses(&[OVERSEAS], 600),
        Answer::Addresses(&[CN], 20),
        true,
    );
    set_domain_policy(&mut resolver, HOST, true);
    let pinned = answer_raw_dns_query(&resolver, &build_dns_query(HOST, QTYPE_AAAA)).await;
    assert_eq!(
        parse_dns_response(&pinned).unwrap(),
        [OVERSEAS.parse::<IpAddr>().unwrap()]
    );
    assert_eq!(servers.probes.load(Ordering::Relaxed), 0);
    let other = answer_raw_dns_query(
        &resolver,
        &build_dns_query("other.cdn-check.com", QTYPE_AAAA),
    )
    .await;
    assert_eq!(
        parse_dns_response(&other).unwrap(),
        [CN.parse::<IpAddr>().unwrap()]
    );
    assert_eq!(servers.probes.load(Ordering::Relaxed), 1);
}

fn set_domain_policy(resolver: &mut DnsResolver, domain: &str, preserve_identity: bool) {
    Arc::make_mut(&mut resolver.config).domain_routes = [DnsDomainRoute {
        selector: crate::router::MatchRule::Domain(domain.into()),
        preserve_identity,
    }]
    .into();
}

#[tokio::test(start_paused = true)]
async fn explicit_and_automatic_routes_keep_distinct_identities_on_a_shared_cdn() {
    let (mut resolver, servers) = fixture(
        Answer::RegionalPrimary,
        Answer::Addresses(&["116.205.40.114", CN], 20),
        false,
    );
    let pool = FakeIpPool::new();
    resolver.set_fakeip(pool.clone());
    set_domain_policy(&mut resolver, HOST, true);
    let neighbor = "other.cdn-check.com";
    for (qtype, edge) in [(QTYPE_A, "116.205.40.114"), (QTYPE_AAAA, CN)] {
        let edge = edge.parse::<IpAddr>().unwrap();
        let before = servers.probes.load(Ordering::Relaxed);
        let query = build_dns_query(HOST, qtype);
        let reply = answer_raw_dns_query(&resolver, &query).await;
        let token = parse_dns_response(&reply).unwrap()[0];
        assert_eq!(
            pool.lookup(fakeip::fake_ipv4(token).unwrap()).as_deref(),
            Some(HOST)
        );
        assert_eq!(
            resolver.resolve_with_qtype(HOST, qtype).await.unwrap(),
            [token]
        );
        assert_eq!(servers.probes.load(Ordering::Relaxed), before);
        let auto = answer_raw_dns_query(&resolver, &build_dns_query(neighbor, qtype)).await;
        assert_eq!(parse_dns_response(&auto).unwrap(), [edge]);
        assert_eq!(pool.lookup_domestic(edge).as_deref(), Some(neighbor));
        assert_eq!(servers.probes.load(Ordering::Relaxed), before + 1);
    }
    // Internal real-address lookups and explicit proxy DNS diagnostics use
    // the configured upstream, independently of the client-visible token.
    let real = resolver.resolve_addresses(HOST).await.unwrap();
    assert_eq!(
        real,
        [
            "116.205.40.114".parse::<IpAddr>().unwrap(),
            CN.parse().unwrap()
        ]
    );
    assert_eq!(servers.probes.load(Ordering::Relaxed), 2);
}

#[tokio::test(start_paused = true)]
async fn identity_policy_keeps_negative_answers_and_uses_the_configured_group() {
    for (direct, rcode) in [
        (Answer::Addresses(&[], 0), 0),
        (Answer::Error(3), 3),
        (Answer::Error(2), 2),
    ] {
        let (mut resolver, servers) = fixture(Answer::Silent, direct, false);
        let pool = FakeIpPool::new();
        resolver.set_fakeip(pool.clone());
        set_domain_policy(&mut resolver, HOST, true);
        Arc::make_mut(&mut resolver.config)
            .domain_rules
            .insert(HOST.into(), DnsGroup::Fallback);
        let reply = answer_raw_dns_query(&resolver, &build_dns_query(HOST, QTYPE_AAAA)).await;
        assert_eq!(reply[3] & 15, rcode);
        assert_eq!(reply[6..8], [0, 0]);
        assert_eq!(servers.primary_queries.load(Ordering::Relaxed), 0);
        assert_eq!(servers.probes.load(Ordering::Relaxed), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn identity_tokens_respect_hosts_node_bootstrap_and_client_ipv6_capability() {
    let (mut resolver, servers) = fixture(Answer::Silent, Answer::Silent, false);
    let pool = FakeIpPool::new();
    resolver.set_fakeip(pool.clone());
    set_domain_policy(&mut resolver, HOST, true);
    let ip = CN.parse::<IpAddr>().unwrap();
    Arc::make_mut(&mut resolver.config)
        .hosts
        .insert(HOST.into(), vec![ip]);
    let v6 = build_dns_query(HOST, QTYPE_AAAA);
    let token = parse_dns_response(&answer_raw_dns_query(&resolver, &v6).await).unwrap()[0];
    assert_eq!(
        pool.lookup(fakeip::fake_ipv4(token).unwrap()).as_deref(),
        Some(HOST)
    );
    assert_eq!(resolver.resolve_addresses(HOST).await.unwrap(), [ip]);
    for qtype in [QTYPE_A, 64, 65] {
        let reply = answer_raw_dns_query(&resolver, &build_dns_query(HOST, qtype)).await;
        assert!(parse_dns_response(&reply).unwrap().is_empty());
    }
    Arc::make_mut(&mut resolver.config).dual_stack_fakeip = false;
    assert!(
        parse_dns_response(&answer_raw_dns_query(&resolver, &v6).await)
            .unwrap()
            .is_empty()
    );
    assert!(
        resolver
            .resolve_with_qtype(HOST, QTYPE_AAAA)
            .await
            .unwrap()
            .is_empty()
    );
    Arc::make_mut(&mut resolver.config)
        .node_hosts
        .insert(HOST.into());
    assert_eq!(
        parse_dns_response(&answer_raw_dns_query(&resolver, &v6).await).unwrap(),
        [ip]
    );

    let local = "device.home.arpa";
    set_domain_policy(&mut resolver, local, true);
    let local_ip = "192.168.1.10".parse::<IpAddr>().unwrap();
    Arc::make_mut(&mut resolver.config)
        .hosts
        .insert(local.into(), vec![local_ip]);
    let reply = answer_raw_dns_query(&resolver, &build_dns_query(local, QTYPE_A)).await;
    assert_eq!(parse_dns_response(&reply).unwrap(), [local_ip]);
    assert_eq!(servers.primary_queries.load(Ordering::Relaxed), 0);
    assert_eq!(servers.probes.load(Ordering::Relaxed), 0);
}

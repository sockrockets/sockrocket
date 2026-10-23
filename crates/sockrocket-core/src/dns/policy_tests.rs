use super::*;
use crate::config::dns::AddressStrategy;
use std::sync::atomic::{AtomicUsize, Ordering};

#[tokio::test]
async fn node_bootstrap_is_independent_of_client_family_and_proxy_dns() {
    let (bootstrap, queries, first) = upstream("192.0.2.77".parse().unwrap()).await;
    let (primary, leaks, second) = upstream("192.0.2.88".parse().unwrap()).await;
    let resolver = DnsResolver::with_config(DnsConfig {
        servers: vec![DnsServer::Udp(primary)],
        bootstrap_servers: Some(vec![bootstrap]),
        node_hosts: ["node.example.test".into()].into_iter().collect(),
        strategy: AddressStrategy::Ipv6Only,
        ..Default::default()
    });
    let answer = resolver
        .scope(scoped_resolve("NoDe.Example.Test."))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(answer, ["192.0.2.77".parse::<IpAddr>().unwrap()]);
    assert!(queries.load(Ordering::SeqCst) > 0);
    assert_eq!(leaks.load(Ordering::SeqCst), 0);
    first.abort();
    second.abort();
}

#[tokio::test]
async fn configured_self_resolver_fails_before_sending_a_query() {
    let (server, calls, task) = upstream("192.0.2.1".parse().unwrap()).await;
    let resolver = DnsResolver::with_config(DnsConfig {
        servers: vec![DnsServer::Udp(server)],
        reject_local_addresses: true,
        ..Default::default()
    });
    assert!(
        resolver
            .test_upstream("loop.example.test", 1)
            .await
            .unwrap_err()
            .to_string()
            .contains("resolver loop")
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    task.abort();
}

async fn upstream(ip: IpAddr) -> (SocketAddr, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let task = tokio::spawn(async move {
        let mut buf = [0; 4096];
        loop {
            let (size, peer) = socket.recv_from(&mut buf).await.unwrap();
            counter.fetch_add(1, Ordering::SeqCst);
            let reply = build_dns_response_with_ttl(&buf[..size], &[ip], 60).unwrap();
            socket.send_to(&reply, peer).await.unwrap();
        }
    });
    (address, calls, task)
}
#[tokio::test]
async fn published_dns_changes_invalidate_wire_cache_without_node_change() {
    let (one, calls1, t1) = upstream("192.0.2.1".parse().unwrap()).await;
    let (two, calls2, t2) = upstream("192.0.2.2".parse().unwrap()).await;
    let outbound = SharedOutbound::direct();
    let swappable = Arc::new(crate::SwappableOutbound::new(outbound.clone()));
    let policy = |server| {
        Arc::new(DnsConfig {
            servers: vec![DnsServer::Udp(server)],
            proxy_servers: Some(vec![]),
            ..Default::default()
        })
    };
    swappable.publish(outbound.clone(), policy(one));
    let mut resolver = DnsResolver::new();
    resolver.follow_runtime(SharedOutbound(swappable.clone()));
    let query = build_dns_query("cache.example.test", 1);
    let first = answer_raw_dns_query(&resolver, &query).await;
    let mut next = query.clone();
    next[0] ^= 1;
    let second = answer_raw_dns_query(&resolver, &next).await;
    assert_eq!(&second[..2], &next[..2]);
    assert_eq!(
        parse_dns_response(&first).unwrap(),
        parse_dns_response(&second).unwrap()
    );
    assert_eq!(calls1.load(Ordering::SeqCst), 1);
    swappable.publish(outbound, policy(two));
    let reply = answer_raw_dns_query(&resolver, &query).await;
    assert_eq!(
        parse_dns_response(&reply).unwrap(),
        ["192.0.2.2".parse::<IpAddr>().unwrap()]
    );
    assert_eq!(calls2.load(Ordering::SeqCst), 1);
    t1.abort();
    t2.abort();
}
#[tokio::test]
async fn scope_resolves_actual_tcp_destinations_without_system_dns() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let target = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = target.local_addr().unwrap().port();
    let echo = tokio::spawn(async move {
        let (mut s, _) = target.accept().await.unwrap();
        let mut byte = [0];
        s.read_exact(&mut byte).await.unwrap();
        s.write_all(&byte).await.unwrap();
    });
    let (address, _, task) = upstream("127.0.0.1".parse().unwrap()).await;
    let config = Arc::new(DnsConfig {
        servers: vec![DnsServer::Udp(address)],
        proxy_servers: Some(vec![]),
        ..Default::default()
    });
    let swappable = crate::SwappableOutbound::new(SharedOutbound::direct());
    swappable.publish(SharedOutbound::direct(), config);
    let mut stream = crate::Outbound::connect(&swappable, "no-system-dns.example.test", port)
        .await
        .unwrap();
    stream.write_all(b"x").await.unwrap();
    let mut byte = [0];
    stream.read_exact(&mut byte).await.unwrap();
    assert_eq!(byte, b"x"[..]);
    echo.await.unwrap();
    task.abort();
}
#[tokio::test]
async fn dual_stack_fakeip_maps_ipv6_and_preserves_domestic_ipv6() {
    let pool = FakeIpPool::new();
    let (server, _, task) = upstream("2001:4860:4860::8888".parse().unwrap()).await;
    let mut resolver = DnsResolver::with_config(DnsConfig {
        servers: vec![DnsServer::Udp(server)],
        dual_stack_fakeip: true,
        ..Default::default()
    });
    resolver.set_fakeip(pool.clone());
    resolver.set_geoip(Arc::new(crate::china_geoip_db()));
    let query = build_dns_query("ipv6.example.test", 28);
    let reply = answer_raw_dns_query(&resolver, &query).await;
    let address = parse_dns_response(&reply).unwrap()[0];
    assert!(address.is_ipv6());
    let v4 = fakeip::fake_ipv4(address).unwrap();
    assert_eq!(pool.lookup(v4).as_deref(), Some("ipv6.example.test"));
    assert_eq!(v4, pool.allocate("ipv6.example.test"));
    resolver.set_client_ipv4_only(true);
    assert!(
        parse_dns_response(&answer_raw_dns_query(&resolver, &query).await)
            .unwrap()
            .is_empty()
    );
    task.abort();
}
#[tokio::test]
async fn ipv4_only_clients_keep_a_answers_under_ipv6_only_strategy() {
    let (server, _, task) = upstream("192.0.2.7".parse().unwrap()).await;
    let mut resolver = DnsResolver::with_config(DnsConfig {
        servers: vec![DnsServer::Udp(server)],
        strategy: AddressStrategy::Ipv6Only,
        ..Default::default()
    });
    let a = build_dns_query("v4.example.test", 1);
    assert!(
        parse_dns_response(&answer_raw_dns_query(&resolver, &a).await)
            .unwrap()
            .is_empty()
    );
    resolver.set_client_ipv4_only(true);
    assert_eq!(
        parse_dns_response(&answer_raw_dns_query(&resolver, &a).await).unwrap(),
        ["192.0.2.7".parse::<IpAddr>().unwrap()]
    );
    let aaaa = build_dns_query("v4.example.test", 28);
    assert!(
        parse_dns_response(&answer_raw_dns_query(&resolver, &aaaa).await)
            .unwrap()
            .is_empty()
    );
    task.abort();
}
#[tokio::test]
async fn fakeip_withholds_https_records_for_proxied_names() {
    let (server, calls, task) = upstream("192.0.2.8".parse().unwrap()).await;
    let mut resolver = DnsResolver::with_config(DnsConfig {
        servers: vec![DnsServer::Udp(server)],
        dual_stack_fakeip: true,
        ..Default::default()
    });
    resolver.set_fakeip(FakeIpPool::new());
    let reply = answer_raw_dns_query(&resolver, &build_dns_query("svcb.example.test", 65)).await;
    assert_eq!(reply[3] & 15, 0);
    assert_eq!(u16::from_be_bytes([reply[6], reply[7]]), 0);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    task.abort();
}
#[test]
fn dns_flag_update_keeps_the_current_outbound() {
    let first = SharedOutbound::direct();
    let swappable = crate::SwappableOutbound::new(first.clone());
    assert!(!swappable.update_dns(|dns| dns.dual_stack_fakeip = true));
    swappable.publish(first, Arc::new(DnsConfig::default()));
    let second = SharedOutbound::direct();
    swappable.set(second.clone());
    assert!(swappable.update_dns(|dns| dns.dual_stack_fakeip = true));
    let (outbound, dns) = crate::Outbound::dns_snapshot(&swappable).unwrap();
    assert!(Arc::ptr_eq(&outbound.0, &second.0));
    assert!(dns.unwrap().dual_stack_fakeip);
}
#[tokio::test]
async fn address_strategy_and_failed_proxy_never_leak_to_direct_group() {
    let (direct, calls, task) = upstream("192.0.2.1".parse().unwrap()).await;
    let resolver = DnsResolver::with_config(DnsConfig {
        servers: vec![DnsServer::ProxyTcp("127.0.0.1:9".parse().unwrap())],
        fallback: vec![DnsServer::Udp(direct)],
        strategy: AddressStrategy::Ipv4Only,
        ..Default::default()
    });
    assert!(
        resolver
            .resolve_with_qtype("ipv6.example.test", 28)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        answer_raw_dns_query(&resolver, &build_dns_query("proxy.example.test", 1)).await[3] & 15,
        2
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    task.abort();
}
#[test]
fn wire_cache_has_a_byte_limit_and_ipv6_cn_classification_is_available() {
    let mut cache = WireCache::new(16384);
    for n in 0..2000u32 {
        cache.put(
            n.to_be_bytes().to_vec(),
            vec![0; 4096],
            Instant::now() + Duration::from_secs(1),
        );
    }
    assert!(cache.bytes <= 1024 * 1024);
    assert!(cache.entries.len() < 300);
    let geo = crate::china_geoip_db();
    assert!(geo.is_cn("2400:3200::1".parse().unwrap()));
    assert!(geo.is_cn("240e::1".parse().unwrap()));
    assert!(!geo.is_cn("2001:4860:4860::8888".parse().unwrap()));
    assert!(!geo.is_cn("2001:2:0:4765::c612:2".parse().unwrap()));
}

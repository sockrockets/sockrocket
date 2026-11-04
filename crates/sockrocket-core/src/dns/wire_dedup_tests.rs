//! Wire-level dedup must preserve request identity and cancellation ownership.
use super::*;
use tokio::time::timeout;

fn resolver(address: SocketAddr) -> DnsResolver {
    DnsResolver::with_config(DnsConfig {
        servers: vec![DnsServer::Udp(address)],
        fallback: vec![],
        cache_enabled: false,
        ..Default::default()
    })
}

async fn participants(resolver: &DnsResolver, expected: usize) {
    timeout(Duration::from_secs(2), async {
        loop {
            let joined = resolver
                .wire_inflight
                .lock()
                .unwrap()
                .values()
                .map(|cell| Arc::strong_count(cell) - 1)
                .sum::<usize>();
            if joined == expected {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn twenty_wire_queries_share_one_exchange_and_keep_their_own_ids() {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let resolver = resolver(socket.local_addr().unwrap());
    let mut clients = tokio::task::JoinSet::new();
    for id in 0..20u16 {
        let resolver = resolver.clone();
        clients.spawn(async move {
            let mut query = build_dns_query("burst.example.test", 1);
            query[..2].copy_from_slice(&id.to_be_bytes());
            let reply = resolver.exchange_raw(&query).await.unwrap();
            assert_eq!(&reply[..2], &id.to_be_bytes());
            assert_eq!(
                parse_dns_response(&reply).unwrap(),
                ["192.0.2.42".parse::<IpAddr>().unwrap()]
            );
        });
    }
    let mut buffer = [0; 4096];
    let (n, peer) = timeout(Duration::from_secs(2), socket.recv_from(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    participants(&resolver, 20).await;
    let reply = build_dns_response(&buffer[..n], &["192.0.2.42".parse().unwrap()]).unwrap();
    socket.send_to(&reply, peer).await.unwrap();
    while let Some(result) = clients.join_next().await {
        result.unwrap();
    }
    assert!(resolver.wire_inflight.lock().unwrap().is_empty());
    assert!(
        timeout(Duration::from_millis(30), socket.recv_from(&mut buffer))
            .await
            .is_err()
    );
    // TTL/cache disabled: a later, non-concurrent request must hit upstream.
    let next = tokio::spawn({
        let resolver = resolver.clone();
        async move {
            resolver
                .exchange_raw(&build_dns_query("burst.example.test", 1))
                .await
                .unwrap()
        }
    });
    let (n, peer) = timeout(Duration::from_secs(2), socket.recv_from(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    socket
        .send_to(&build_dns_response(&buffer[..n], &[]).unwrap(), peer)
        .await
        .unwrap();
    next.await.unwrap();
}

#[tokio::test]
async fn cancelled_leader_transfers_initialization_and_last_cancel_cleans_up() {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let resolver = resolver(socket.local_addr().unwrap());
    let query = build_dns_query("cancel-wire.example.test", 1);
    let leader = tokio::spawn({
        let resolver = resolver.clone();
        let query = query.clone();
        async move { resolver.exchange_raw(&query).await }
    });
    let mut buffer = [0; 4096];
    timeout(Duration::from_secs(2), socket.recv_from(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    let follower = tokio::spawn({
        let resolver = resolver.clone();
        let mut query = query.clone();
        query[..2].copy_from_slice(&123u16.to_be_bytes());
        async move { resolver.exchange_raw(&query).await.unwrap() }
    });
    participants(&resolver, 2).await;
    leader.abort();
    assert!(leader.await.unwrap_err().is_cancelled());
    let (n, peer) = timeout(Duration::from_secs(2), socket.recv_from(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buffer[..2], &123u16.to_be_bytes());
    socket
        .send_to(&build_dns_response(&buffer[..n], &[]).unwrap(), peer)
        .await
        .unwrap();
    assert_eq!(
        &timeout(Duration::from_secs(2), follower)
            .await
            .unwrap()
            .unwrap()[..2],
        &123u16.to_be_bytes()
    );
    assert!(resolver.wire_inflight.lock().unwrap().is_empty());
    let last = tokio::spawn({
        let resolver = resolver.clone();
        async move { resolver.exchange_raw(&query).await }
    });
    timeout(Duration::from_secs(2), socket.recv_from(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    last.abort();
    let _ = last.await;
    assert!(resolver.wire_inflight.lock().unwrap().is_empty());
}

#[tokio::test]
async fn wire_flags_edns_qtype_exit_and_upstream_set_stay_isolated() {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let resolver = resolver(socket.local_addr().unwrap());
    let ordinary = build_dns_query("identity.example.test", 1);
    let mut cd = ordinary.clone();
    cd[3] |= 0x10;
    let mut edns = ordinary.clone();
    edns[11] = 1;
    edns.extend_from_slice(&[0, 0, 41, 4, 208, 0, 0, 0, 0, 0, 0]);
    let aaaa = build_dns_query("identity.example.test", 28);
    let mut direct = resolver.clone();
    direct.direct_resolution = true;
    let mut override_resolver = resolver.clone();
    override_resolver.primary_override = Some(Arc::new(vec![
        DnsServer::Udp(socket.local_addr().unwrap()),
        DnsServer::Udp(socket.local_addr().unwrap()),
    ]));
    let mut clients = tokio::task::JoinSet::new();
    for (resolver, query) in [
        (resolver.clone(), ordinary.clone()),
        (resolver.clone(), cd),
        (resolver.clone(), edns),
        (resolver.clone(), aaaa),
        (direct, ordinary.clone()),
        (override_resolver, ordinary),
    ] {
        clients.spawn(async move { resolver.exchange_raw(&query).await.unwrap() });
    }
    participants(&resolver, 6).await;
    assert_eq!(resolver.wire_inflight.lock().unwrap().len(), 6);
    let mut buffer = [0; 4096];
    // The overridden set intentionally races two copies of its upstream.
    for _ in 0..7 {
        let (n, peer) = timeout(Duration::from_secs(2), socket.recv_from(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        socket
            .send_to(&build_dns_response(&buffer[..n], &[]).unwrap(), peer)
            .await
            .unwrap();
    }
    while let Some(result) = clients.join_next().await {
        result.unwrap();
    }
    assert!(resolver.wire_inflight.lock().unwrap().is_empty());
}

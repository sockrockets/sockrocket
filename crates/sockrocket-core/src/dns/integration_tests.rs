use super::*;
use tokio::net::{TcpListener, TcpStream};

fn txt_reply(query: &[u8]) -> Vec<u8> {
    let mut reply = build_dns_response(query, &[]).unwrap();
    reply[7] = 1;
    reply.extend([0xc0, 0x0c, 0, 16, 0, 1]);
    reply.extend(123u32.to_be_bytes());
    reply.extend([0, 6, 5]);
    reply.extend(b"hello");
    reply
}
fn config(server: DnsServer) -> DnsResolver {
    DnsResolver::with_config(DnsConfig {
        servers: vec![server],
        fallback: vec![],
        domain_rules: HashMap::new(),
        cache_ttl: 0,
        cache_enabled: false,
    })
}
#[tokio::test]
async fn raw_txt_and_nxdomain_are_preserved() {
    let upstream = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let resolver = config(DnsServer::Udp(upstream.local_addr().unwrap()));
    let server = tokio::spawn(async move {
        let mut buf = [0; 4096];
        for negative in [false, true] {
            let (n, peer) = upstream.recv_from(&mut buf).await.unwrap();
            let reply = if negative {
                build_dns_error_response(&buf[..n], 3)
            } else {
                txt_reply(&buf[..n])
            };
            upstream.send_to(&reply, peer).await.unwrap();
        }
    });
    let query = build_dns_query("example.com", 16);
    assert_eq!(
        answer_raw_dns_query(&resolver, &query).await,
        txt_reply(&query)
    );
    let query = build_dns_query("missing.example", 1);
    let reply = answer_raw_dns_query(&resolver, &query).await;
    assert_eq!(reply, build_dns_error_response(&query, 3));
    server.await.unwrap();
}
#[tokio::test]
async fn truncated_udp_retries_same_upstream_over_tcp() {
    let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = udp.local_addr().unwrap();
    let tcp = TcpListener::bind(addr).await.unwrap();
    let server = tokio::spawn(async move {
        let mut buf = [0; 4096];
        let (n, peer) = udp.recv_from(&mut buf).await.unwrap();
        let mut truncated = build_dns_response(&buf[..n], &[]).unwrap();
        truncated[2] |= 2;
        udp.send_to(&truncated, peer).await.unwrap();
        let (mut socket, _) = tcp.accept().await.unwrap();
        let len = socket.read_u16().await.unwrap() as usize;
        let mut query = vec![0; len];
        socket.read_exact(&mut query).await.unwrap();
        let reply = txt_reply(&query);
        socket.write_u16(reply.len() as u16).await.unwrap();
        socket.write_all(&reply).await.unwrap();
    });
    let resolver = config(DnsServer::Udp(addr));
    let query = build_dns_query("example.com", 16);
    assert_eq!(
        answer_raw_dns_query(&resolver, &query).await,
        txt_reply(&query)
    );
    server.await.unwrap();
}
#[tokio::test]
async fn tcp_listener_answers_multiple_frames_and_stops_idle_clients() {
    let upstream = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let resolver = config(DnsServer::Udp(upstream.local_addr().unwrap()));
    let responder = tokio::spawn(async move {
        let mut buf = [0; 4096];
        for _ in 0..2 {
            let (n, peer) = upstream.recv_from(&mut buf).await.unwrap();
            upstream.send_to(&txt_reply(&buf[..n]), peer).await.unwrap();
        }
    });
    let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = probe.local_addr().unwrap();
    drop(probe);
    let shutdown = Arc::new(tokio::sync::Notify::new());
    let task = tokio::spawn(serve_dns_udp(
        "127.0.0.1",
        address.port(),
        resolver,
        shutdown.clone(),
    ));
    let mut client = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(client) = TcpStream::connect(address).await {
                break client;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    for _ in 0..2 {
        let query = build_dns_query("example.com", 16);
        client.write_u16(query.len() as u16).await.unwrap();
        client.write_all(&query).await.unwrap();
        let len = client.read_u16().await.unwrap() as usize;
        let mut reply = vec![0; len];
        client.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply, txt_reply(&query));
    }
    shutdown.notify_one();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(TcpListener::bind(address).await.is_ok());
    assert!(UdpSocket::bind(address).await.is_ok());
    responder.await.unwrap();
}
#[tokio::test]
async fn cancellation_drops_checked_out_dns_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut resolver = config(DnsServer::ProxyTcp(address));
    resolver.set_outbound(SharedOutbound::direct());
    resolver.proxy_tcp_conn = Arc::new(ProxyConnPool::new(1));
    let resolver = Arc::new(resolver);
    let received = Arc::new(tokio::sync::Notify::new());
    let received2 = received.clone();
    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let len = first.read_u16().await.unwrap() as usize;
        first.read_exact(&mut vec![0; len]).await.unwrap();
        received2.notify_one();
        let mut byte = [0];
        assert_eq!(first.read(&mut byte).await.unwrap(), 0);
        let (mut second, _) = listener.accept().await.unwrap();
        let len = second.read_u16().await.unwrap() as usize;
        let mut query = vec![0; len];
        second.read_exact(&mut query).await.unwrap();
        let reply = txt_reply(&query);
        second.write_u16(reply.len() as u16).await.unwrap();
        second.write_all(&reply).await.unwrap();
    });
    let first = {
        let resolver = resolver.clone();
        tokio::spawn(async move {
            resolver
                .exchange_raw(&build_dns_query("first.example", 16))
                .await
        })
    };
    received.notified().await;
    first.abort();
    let _ = first.await;
    let query = build_dns_query("second.example", 16);
    let reply = tokio::time::timeout(Duration::from_secs(2), resolver.exchange_raw(&query))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply, txt_reply(&query));
    server.await.unwrap();
}
#[test]
fn edns_payload_and_classic_udp_truncation() {
    let query = build_dns_query("example.com", 16);
    let mut reply = build_dns_response(&query, &[]).unwrap();
    reply[7] = 1;
    reply.extend([0xc0, 12, 0, 16, 0, 1, 0, 0, 0, 60]);
    reply.extend(700u16.to_be_bytes());
    reply.extend(vec![0; 700]);
    let truncated = wire::udp_response(&query, reply.clone());
    assert!(truncated[2] & 2 != 0);
    assert!(truncated.len() <= 512);
    let mut edns = query;
    edns[11] = 1;
    edns.extend([0, 0, 41, 4, 208, 0, 0, 0, 0, 0, 0]);
    assert_eq!(wire::udp_response(&edns, reply.clone()), reply);
}

#[tokio::test]
async fn published_outbound_changes_isolate_dns_cache_generations() {
    use crate::proxy::connector::SwappableOutbound;
    let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = server.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut query = [0; 1024];
        for ip in ["192.0.2.1", "192.0.2.2"] {
            let (size, peer) = server.recv_from(&mut query).await.unwrap();
            let reply = build_dns_response(&query[..size], &[ip.parse().unwrap()]).unwrap();
            server.send_to(&reply, peer).await.unwrap();
        }
    });
    let swappable = Arc::new(SwappableOutbound::new(SharedOutbound::direct()));
    let mut resolver = DnsResolver::with_config(DnsConfig {
        servers: vec![DnsServer::Udp(addr)],
        fallback: vec![],
        domain_rules: HashMap::new(),
        cache_ttl: 0,
        cache_enabled: true,
    });
    resolver.set_outbound(SharedOutbound(swappable.clone()));
    assert_eq!(
        resolver.resolve("example.com").await.unwrap(),
        vec!["192.0.2.1".parse::<IpAddr>().unwrap()]
    );
    let first = resolver.query_snapshot();
    swappable.set(SharedOutbound::direct());
    assert_eq!(
        resolver.resolve("example.com").await.unwrap(),
        vec!["192.0.2.2".parse::<IpAddr>().unwrap()]
    );
    let second = resolver.query_snapshot();
    assert!(!Arc::ptr_eq(&first.cache, &second.cache));
    assert!(!Arc::ptr_eq(&first.inflight, &second.inflight));
    task.await.unwrap();
}
#[tokio::test]
async fn zero_ttl_answers_are_not_served_from_stale_cache() {
    let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = server.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut query = [0; 1024];
        for ip in ["192.0.2.1", "192.0.2.2"] {
            let (size, peer) = server.recv_from(&mut query).await.unwrap();
            let reply =
                build_dns_response_with_ttl(&query[..size], &[ip.parse().unwrap()], 0).unwrap();
            server.send_to(&reply, peer).await.unwrap();
        }
    });
    let resolver = DnsResolver::with_config(DnsConfig {
        servers: vec![DnsServer::Udp(addr)],
        fallback: vec![],
        domain_rules: HashMap::new(),
        cache_ttl: 0,
        cache_enabled: true,
    });
    assert_eq!(
        resolver.resolve("example.com").await.unwrap(),
        vec!["192.0.2.1".parse::<IpAddr>().unwrap()]
    );
    assert_eq!(
        resolver.resolve("example.com").await.unwrap(),
        vec!["192.0.2.2".parse::<IpAddr>().unwrap()]
    );
    assert_eq!(resolver.cache_stats().await.0, 0);
    task.await.unwrap();
}

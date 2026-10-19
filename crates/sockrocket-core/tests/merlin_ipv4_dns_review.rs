use sockrocket_core::dns::{
    DnsConfig, DnsResolver, DnsServer, answer_raw_dns_query, build_dns_response,
};
use std::net::IpAddr;
fn build_dns_query(name: &str, kind: u16) -> Vec<u8> {
    let mut query = vec![0x12, 0x34, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    for label in name.split('.') {
        query.push(label.len() as u8);
        query.extend_from_slice(label.as_bytes());
    }
    query.push(0);
    query.extend_from_slice(&kind.to_be_bytes());
    query.extend_from_slice(&[0, 1]);
    query
}
use tokio::net::UdpSocket;

#[tokio::test]
async fn ipv4_gateway_returns_nodata_without_upstream_for_aaaa_and_https() {
    let mut resolver = DnsResolver::with_config(DnsConfig {
        servers: vec![],
        fallback: vec![],
        ..Default::default()
    });
    resolver.set_client_ipv4_only(true);
    for kind in [28, 65] {
        let query = build_dns_query("example.test", kind);
        let reply = answer_raw_dns_query(&resolver, &query).await;
        assert_eq!(&reply[..2], &query[..2]);
        assert_eq!(reply[3] & 15, 0);
        assert_eq!(&reply[6..12], &[0; 6]);
        assert_eq!(&reply[12..], &query[12..]);
    }
}

#[tokio::test]
async fn ipv4_client_policy_preserves_a_and_internal_node_ipv6_resolution() {
    let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = server.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut buf = [0; 1024];
        loop {
            let (n, peer) = server.recv_from(&mut buf).await.unwrap();
            let (_, kind) = sockrocket_core::parse_dns_query(&buf[..n]).unwrap();
            let ip: IpAddr = if kind == 28 {
                "2001:db8::7".parse().unwrap()
            } else {
                "192.0.2.7".parse().unwrap()
            };
            let reply = build_dns_response(&buf[..n], &[ip]).unwrap();
            server.send_to(&reply, peer).await.unwrap();
        }
    });
    let config = DnsConfig {
        servers: vec![DnsServer::Udp(address)],
        fallback: vec![DnsServer::Udp(address)],
        ..Default::default()
    };
    let mut gateway = DnsResolver::with_config(config.clone());
    gateway.set_client_ipv4_only(true);
    let reply = answer_raw_dns_query(&gateway, &build_dns_query("example.test", 1)).await;
    assert_eq!(&reply[6..8], &[0, 1]);
    assert_eq!(&reply[reply.len() - 4..], &[192, 0, 2, 7]);
    assert_eq!(
        gateway
            .resolve_ipv6("node.example.test")
            .await
            .unwrap()
            .to_string(),
        "2001:db8::7"
    );
    let desktop = DnsResolver::with_config(config);
    let reply = answer_raw_dns_query(&desktop, &build_dns_query("example.test", 28)).await;
    assert_eq!(&reply[6..8], &[0, 1]);
    assert_eq!(
        &reply[reply.len() - 16..],
        &"2001:db8::7"
            .parse::<std::net::Ipv6Addr>()
            .unwrap()
            .octets()
    );
    task.abort();
}

use sockrocket_core::proxy::{
    ProxyStats, connector::SharedOutbound, http::HttpProxyServer, socks5::Socks5Server,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::watch,
    time::{Duration, timeout},
};

#[tokio::test]
async fn stopping_http_listener_closes_existing_tunnels_and_records() {
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = upstream.local_addr().unwrap();
    let stats = ProxyStats::default();
    let server = HttpProxyServer::bind("127.0.0.1", 0, SharedOutbound::direct(), stats.clone())
        .await
        .unwrap();
    let port = server.local_port();
    let (stop, rx) = watch::channel(false);
    let task = tokio::spawn(server.run(rx));
    let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    client
        .write_all(format!("CONNECT {target} HTTP/1.1\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let (_remote, _) = upstream.accept().await.unwrap();
    let mut response = [0u8; 39];
    client.read_exact(&mut response).await.unwrap();
    assert_eq!(stats.active_connections(), 1);
    stop.send(true).unwrap();
    timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let mut b = [0u8; 1];
    assert_eq!(
        timeout(Duration::from_secs(1), client.read(&mut b))
            .await
            .unwrap()
            .unwrap(),
        0
    );
    assert_eq!(stats.active_connections(), 0);
    assert!(
        stats
            .recent_connections(10, |_| true)
            .iter()
            .all(|r| !r.active)
    );
}
#[tokio::test]
async fn failed_connect_does_not_leave_an_active_record() {
    use sockrocket_core::proxy::connector::{BoxProxyStream, Outbound};
    use std::{future::Future, pin::Pin, sync::Arc};
    struct RefusedOutbound;
    impl Outbound for RefusedOutbound {
        fn connect(
            &self,
            _: &str,
            _: u16,
        ) -> Pin<Box<dyn Future<Output = anyhow::Result<BoxProxyStream>> + Send + '_>> {
            Box::pin(async {
                Err(std::io::Error::from(std::io::ErrorKind::ConnectionRefused).into())
            })
        }
        fn name(&self) -> &str {
            "synthetic-refusal"
        }
    }
    // Fail deterministically: releasing an ephemeral port races its reuse and
    // closed-port refusal latency varies by OS. This test exercises cleanup.
    let target = "example.invalid:443";
    let stats = ProxyStats::default();
    let server = HttpProxyServer::bind(
        "127.0.0.1",
        0,
        SharedOutbound(Arc::new(RefusedOutbound)),
        stats.clone(),
    )
    .await
    .unwrap();
    let port = server.local_port();
    let (stop, rx) = watch::channel(false);
    let task = tokio::spawn(server.run(rx));
    let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    client
        .write_all(format!("CONNECT {target} HTTP/1.1\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut response = Vec::new();
    timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(response.starts_with(b"HTTP/1.1 502 Bad Gateway"));
    stop.send(true).unwrap();
    task.await.unwrap().unwrap();
    assert_eq!(stats.active_connections(), 0);
    let records = stats.recent_connections(10, |_| true);
    assert_eq!(records.len(), 1);
    assert!(!records[0].active);
}
#[tokio::test]
async fn stalled_socks_handshake_has_a_deadline() {
    let stats = ProxyStats::default();
    let server = Socks5Server::bind("127.0.0.1", 0, SharedOutbound::direct(), stats.clone())
        .await
        .unwrap();
    let port = server.local_port();
    let (stop, rx) = watch::channel(false);
    let task = tokio::spawn(server.run(rx));
    let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    client.write_all(&[5]).await.unwrap();
    let mut b = [0u8; 1];
    assert_eq!(
        timeout(Duration::from_secs(12), client.read(&mut b))
            .await
            .unwrap()
            .unwrap(),
        0
    );
    stop.send(true).unwrap();
    task.await.unwrap().unwrap();
    assert_eq!(stats.active_connections(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_stalled_pool_factory_cannot_hold_an_on_demand_connect_forever() {
    use sockrocket_core::proxy::{
        connector::BoxProxyStream,
        pool::{ConnFactory, ConnPool},
    };
    struct Stalled;
    impl ConnFactory for Stalled {
        fn create(
            &self,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = anyhow::Result<BoxProxyStream>> + Send + '_>,
        > {
            Box::pin(std::future::pending())
        }
    }
    let pool = ConnPool::lazy(std::sync::Arc::new(Stalled));
    assert!(
        pool.get()
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("timed out")
    );
}

#[tokio::test]
async fn plain_http_does_not_forward_proxy_credentials_to_the_origin() {
    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = origin.local_addr().unwrap();
    let server = HttpProxyServer::bind(
        "127.0.0.1",
        0,
        SharedOutbound::direct(),
        ProxyStats::default(),
    )
    .await
    .unwrap();
    let port = server.local_port();
    let (stop, rx) = watch::channel(false);
    let task = tokio::spawn(server.run(rx));
    let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    client.write_all(format!("GET http://{addr}/synthetic HTTP/1.1\r\nHost: {addr}\r\nProxy-Authorization: Basic synthetic\r\n\r\n").as_bytes()).await.unwrap();
    client.shutdown().await.unwrap();
    let (mut remote, _) = origin.accept().await.unwrap();
    let mut request = Vec::new();
    while !request.ends_with(b"\r\n\r\n") {
        request.push(remote.read_u8().await.unwrap());
    }
    assert!(
        !String::from_utf8_lossy(&request)
            .to_ascii_lowercase()
            .contains("proxy-authorization")
    );
    remote
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    remote.shutdown().await.unwrap();
    let mut b = Vec::new();
    client.read_to_end(&mut b).await.unwrap();
    stop.send(true).unwrap();
    task.await.unwrap().unwrap();
}

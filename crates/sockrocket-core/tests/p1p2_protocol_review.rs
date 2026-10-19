use sockrocket_core::proxy::{
    ProxyStats, connector::SharedOutbound, http::HttpProxyServer, socks5::Socks5Server,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::watch,
    time::{Duration, timeout},
};
async fn headers(s: &mut TcpStream) -> Vec<u8> {
    let mut b = Vec::new();
    while !b.ends_with(b"\r\n\r\n") {
        b.push(s.read_u8().await.unwrap());
        assert!(b.len() < 65536);
    }
    b
}
#[tokio::test]
async fn keepalive_rechecks_authority_and_strips_nominated_secrets() {
    let a = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let b = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addresses = [a.local_addr().unwrap(), b.local_addr().unwrap()];
    let origins = tokio::spawn(async move {
        for listener in [a, b] {
            let (mut s, _) = listener.accept().await.unwrap();
            let h = String::from_utf8(headers(&mut s).await)
                .unwrap()
                .to_lowercase();
            assert!(!h.contains("synthetic-secret"));
            assert!(h.starts_with("get /item http/1.1"));
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await
                .unwrap();
        }
    });
    let stats = ProxyStats::default();
    let server = HttpProxyServer::bind("127.0.0.1", 0, SharedOutbound::direct(), stats.clone())
        .await
        .unwrap();
    let port = server.local_port();
    let (stop, rx) = watch::channel(false);
    let task = tokio::spawn(server.run(rx));
    let mut c = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    for addr in addresses {
        c.write_all(format!("GET http://{addr}/item HTTP/1.1\r\nHost: ignored.invalid\r\nConnection: keep-alive, X-Private\r\nX-Private: synthetic-secret\r\nProxy-Authorization: synthetic-secret\r\n\r\n").as_bytes()).await.unwrap();
        let h = timeout(Duration::from_secs(2), headers(&mut c))
            .await
            .unwrap();
        assert!(h.starts_with(b"HTTP/1.1 200"));
        let mut body = [0; 2];
        c.read_exact(&mut body).await.unwrap();
        assert_eq!(&body, b"ok");
    }
    assert!(
        stats.bytes_received() > 0,
        "long-lived proxy connection must report live traffic"
    );
    drop(c);
    origins.await.unwrap();
    stop.send(true).unwrap();
    task.await.unwrap().unwrap();
    assert_eq!(stats.active_connections(), 0);
}
#[tokio::test]
async fn udp_association_pins_client_and_closes_with_control() {
    let echo = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let target = echo.local_addr().unwrap();
    let echoes = tokio::spawn(async move {
        let mut b = [0; 100];
        let (n, p) = echo.recv_from(&mut b).await.unwrap();
        echo.send_to(&b[..n], p).await.unwrap();
    });
    let stats = ProxyStats::default();
    let server = Socks5Server::bind("127.0.0.1", 0, SharedOutbound::direct(), stats.clone())
        .await
        .unwrap();
    let port = server.local_port();
    let (stop, rx) = watch::channel(false);
    let task = tokio::spawn(server.run(rx));
    let mut c = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    c.write_all(&[5, 1, 0]).await.unwrap();
    let mut auth = [0; 2];
    c.read_exact(&mut auth).await.unwrap();
    assert_eq!(auth, [5, 0]);
    c.write_all(&[5, 3, 0, 1, 0, 0, 0, 0, 0, 0]).await.unwrap();
    let mut reply = [0; 10];
    c.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply[1], 0);
    let relay =
        std::net::SocketAddr::from(([127, 0, 0, 1], u16::from_be_bytes([reply[8], reply[9]])));
    let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let mut packet = vec![0, 0, 0, 1, 127, 0, 0, 1];
    packet.extend(target.port().to_be_bytes());
    packet.extend(b"hello");
    udp.send_to(&packet, relay).await.unwrap();
    let mut response = [0; 100];
    let (n, _) = timeout(Duration::from_secs(2), udp.recv_from(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&response[..n], &packet);
    echoes.await.unwrap();
    drop(c);
    timeout(Duration::from_secs(2), async {
        while stats.active_connections() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("control close must finish association teardown");
    udp.send_to(&packet, relay).await.unwrap();
    match timeout(Duration::from_millis(100), udp.recv_from(&mut response)).await {
        Err(_) => {} // Systems that silently drop traffic to the closed relay.
        Ok(Err(error))
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionRefused
            ) => {} // Windows reports the ICMP rejection.
        result => panic!("closed UDP association must not relay data: {result:?}"),
    }
    stop.send(true).unwrap();
    task.await.unwrap().unwrap();
}
#[tokio::test]
async fn chunked_request_body_streams_without_retaining_whole_body() {
    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = origin.local_addr().unwrap();
    let remote = tokio::spawn(async move {
        let (mut s, _) = origin.accept().await.unwrap();
        let h = headers(&mut s).await;
        assert!(
            String::from_utf8_lossy(&h)
                .to_lowercase()
                .contains("transfer-encoding: chunked")
        );
        let mut body = Vec::new();
        while !body.ends_with(b"0\r\n\r\n") {
            body.push(s.read_u8().await.unwrap());
        }
        assert!(body.windows(5).any(|b| b == b"hello"));
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
    });
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
    let mut c = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    c.write_all(format!("POST http://{addr}/ HTTP/1.1\r\nHost: {addr}\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n").as_bytes()).await.unwrap();
    assert!(
        timeout(Duration::from_secs(2), headers(&mut c))
            .await
            .unwrap()
            .starts_with(b"HTTP/1.1 200")
    );
    remote.await.unwrap();
    stop.send(true).unwrap();
    task.await.unwrap().unwrap();
}
#[tokio::test]
async fn upgrade_preserves_buffered_bytes_and_stopping_closes_the_tunnel() {
    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = origin.local_addr().unwrap();
    let remote = tokio::spawn(async move {
        let (mut s, _) = origin.accept().await.unwrap();
        let h = headers(&mut s).await;
        assert!(
            String::from_utf8_lossy(&h)
                .to_lowercase()
                .contains("upgrade: synthetic")
        );
        s.write_all(b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: synthetic\r\n\r\nready").await.unwrap();
        let mut body = [0; 4];
        s.read_exact(&mut body).await.unwrap();
        assert_eq!(&body, b"ping");
        s.write_all(b"pong").await.unwrap();
        let mut b = [0];
        timeout(Duration::from_secs(2), s.read(&mut b))
            .await
            .unwrap()
            .unwrap()
    });
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
    let mut c = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    c.write_all(format!("GET http://{addr}/ HTTP/1.1\r\nHost: {addr}\r\nConnection: Upgrade\r\nUpgrade: synthetic\r\n\r\nping").as_bytes()).await.unwrap();
    assert!(
        timeout(Duration::from_secs(2), headers(&mut c))
            .await
            .unwrap()
            .starts_with(b"HTTP/1.1 101")
    );
    let mut response = [0; 9];
    timeout(Duration::from_secs(2), c.read_exact(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&response, b"readypong");
    stop.send(true).unwrap();
    task.await.unwrap().unwrap();
    let mut b = [0];
    assert_eq!(c.read(&mut b).await.unwrap(), 0);
    assert_eq!(remote.await.unwrap(), 0);
}
#[tokio::test]
async fn cancelled_tunnel_keeps_live_bytes_without_counting_them_twice() {
    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = origin.local_addr().unwrap();
    let remote = tokio::spawn(async move {
        let (mut s, _) = origin.accept().await.unwrap();
        let mut body = vec![0; 65536];
        s.read_exact(&mut body).await.unwrap();
        s.write_all(&body).await.unwrap();
        let mut b = [0];
        timeout(Duration::from_secs(2), s.read(&mut b))
            .await
            .unwrap()
            .unwrap()
    });
    let stats = ProxyStats::default();
    let server = HttpProxyServer::bind("127.0.0.1", 0, SharedOutbound::direct(), stats.clone())
        .await
        .unwrap();
    let port = server.local_port();
    let (stop, rx) = watch::channel(false);
    let task = tokio::spawn(server.run(rx));
    let mut c = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    c.write_all(format!("CONNECT {addr} HTTP/1.1\r\n\r\n").as_bytes())
        .await
        .unwrap();
    headers(&mut c).await;
    c.write_all(&vec![42; 65536]).await.unwrap();
    let mut echo = vec![0; 65536];
    c.read_exact(&mut echo).await.unwrap();
    assert_eq!(stats.bytes_sent(), 65536);
    assert_eq!(stats.bytes_received(), 65536);
    let rows = stats.recent_connections(1, |_| true);
    assert!(rows[0].active);
    assert_eq!(rows[0].bytes_up, 65536);
    stop.send(true).unwrap();
    task.await.unwrap().unwrap();
    assert_eq!(stats.bytes_sent(), 65536);
    assert_eq!(stats.bytes_received(), 65536);
    let rows = stats.recent_connections(1, |_| true);
    assert!(!rows[0].active);
    assert_eq!(rows[0].bytes_up, 65536);
    assert_eq!(rows[0].completion, Some("interrupted"));
    assert_eq!(remote.await.unwrap(), 0);
}

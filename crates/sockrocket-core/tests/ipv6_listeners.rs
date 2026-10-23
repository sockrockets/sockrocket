//! Real loopback IPv6 proxy listeners; no host routes or preferences change.
use sockrocket_core::{HttpProxyServer, ProxyStats, SharedOutbound, Socks5Server};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::watch,
};

#[tokio::test]
async fn http_and_socks_listeners_accept_ipv6_clients_and_destinations() {
    // IPv6-disabled CI hosts cannot exercise this platform capability.
    let Ok(echo) = TcpListener::bind("[::1]:0").await else {
        return;
    };
    let destination = echo.local_addr().unwrap();
    let peer = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut stream, _) = echo.accept().await.unwrap();
            tokio::spawn(async move {
                let (mut r, mut w) = stream.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    let http = HttpProxyServer::bind("::1", 0, SharedOutbound::direct(), ProxyStats::default())
        .await
        .unwrap();
    let socks = Socks5Server::bind("[::1]", 0, SharedOutbound::direct(), ProxyStats::default())
        .await
        .unwrap();
    let hp = http.local_port();
    let sp = socks.local_port();
    let (stop, rx) = watch::channel(false);
    let ht = tokio::spawn(http.run(rx.clone()));
    let st = tokio::spawn(socks.run(rx));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let mut client = TcpStream::connect(("::1", hp)).await.unwrap();
        client
            .write_all(
                format!("CONNECT {destination} HTTP/1.1\r\nHost: {destination}\r\n\r\n").as_bytes(),
            )
            .await
            .unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            header.push(client.read_u8().await.unwrap());
            assert!(header.len() < 4096);
        }
        assert!(header.starts_with(b"HTTP/1.1 200"));
        client.write_all(b"http-v6").await.unwrap();
        let mut reply = [0; 7];
        client.read_exact(&mut reply).await.unwrap();
        assert_eq!(&reply, b"http-v6");
        let mut client = TcpStream::connect(("::1", sp)).await.unwrap();
        client.write_all(&[5, 1, 0]).await.unwrap();
        let mut auth = [0; 2];
        client.read_exact(&mut auth).await.unwrap();
        assert_eq!(auth, [5, 0]);
        let mut connect = vec![5, 1, 0, 4];
        connect.extend(std::net::Ipv6Addr::LOCALHOST.octets());
        connect.extend(destination.port().to_be_bytes());
        client.write_all(&connect).await.unwrap();
        let mut result = [0; 4];
        client.read_exact(&mut result).await.unwrap();
        assert_eq!(result[1], 0);
        let bytes = if result[3] == 4 { 18 } else { 6 };
        let mut bound = vec![0; bytes];
        client.read_exact(&mut bound).await.unwrap();
        client.write_all(b"socks-v6").await.unwrap();
        let mut reply = [0; 8];
        client.read_exact(&mut reply).await.unwrap();
        assert_eq!(&reply, b"socks-v6");
    })
    .await
    .unwrap();
    stop.send(true).unwrap();
    ht.await.unwrap().unwrap();
    st.await.unwrap().unwrap();
    peer.await.unwrap();
}

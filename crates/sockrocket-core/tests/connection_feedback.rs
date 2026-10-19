use sockrocket_core::ProxyService;
use tokio::net::TcpListener;

#[tokio::test]
async fn busy_socks_port_has_actionable_error() {
    let occupied = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = occupied.local_addr().unwrap().port();
    let mut service = ProxyService::new();
    let error = service.start("127.0.0.1", port, 0).await.unwrap_err();
    let text = format!("{error:#}");
    assert!(text.contains("SOCKS5") && text.contains(&port.to_string()));
    assert!(text.contains("Close the other proxy instance"));
    assert!(!service.is_running());
}

#[tokio::test]
async fn busy_http_port_releases_partial_socks_listener() {
    let occupied = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http_port = occupied.local_addr().unwrap().port();
    let reserved = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let socks_port = reserved.local_addr().unwrap().port();
    drop(reserved);
    let mut service = ProxyService::new();
    let error = service
        .start("127.0.0.1", socks_port, http_port)
        .await
        .unwrap_err();
    let text = format!("{error:#}");
    assert!(text.contains("HTTP") && text.contains(&http_port.to_string()));
    assert!(!service.is_running());
    let _released = TcpListener::bind(("127.0.0.1", socks_port)).await.unwrap();
}

use sockrocket_core::{AppConfig, Subscription, apply_subscription_refresh, fetch_subscription};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
fn sub(url: String) -> Subscription {
    Subscription {
        name: "Synthetic".into(),
        url,
        format: "clash".into(),
        last_updated: None,
        refresh_interval_hours: 24,
    }
}
async fn serve(headers: String, body: Vec<u8>) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut c, _) = listener.accept().await.unwrap();
        let mut b = [0u8; 2048];
        let _ = c.read(&mut b).await;
        let _ = c.write_all(headers.as_bytes()).await;
        let _ = c.write_all(&body).await;
    });
    (format!("http://{addr}/synthetic"), task)
}
#[tokio::test]
async fn rejects_declared_oversize_without_waiting_for_body() {
    let (url, task) = serve(
        "HTTP/1.1 200 OK\r\nContent-Length: 8388609\r\nConnection: close\r\n\r\n".into(),
        Vec::new(),
    )
    .await;
    let err = fetch_subscription(&sub(url)).await.unwrap_err();
    assert!(err.to_string().contains("size limit"));
    task.await.unwrap();
}
#[tokio::test]
async fn rejects_chunked_oversize_without_content_length() {
    let mut wire = Vec::new();
    for _ in 0..129 {
        wire.extend_from_slice(b"10000\r\n");
        wire.extend(vec![b'a'; 65536]);
        wire.extend_from_slice(b"\r\n");
    }
    wire.extend_from_slice(b"0\r\n\r\n");
    let (url, task) = serve(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".into(),
        wire,
    )
    .await;
    let err = fetch_subscription(&sub(url)).await.unwrap_err();
    assert!(err.to_string().contains("size limit"));
    task.await.unwrap();
}
#[tokio::test]
async fn empty_refresh_is_an_error_and_does_not_mutate_saved_state() {
    let body = b"proxies: []\n".to_vec();
    let (url, task) = serve(
        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()),
        body,
    )
    .await;
    assert!(
        fetch_subscription(&sub(url))
            .await
            .unwrap_err()
            .to_string()
            .contains("no usable nodes")
    );
    task.await.unwrap();
    let mut config = AppConfig::default();
    config
        .subscriptions
        .push(sub("https://example.org/synthetic".into()));
    let before = serde_yaml::to_string(&config).unwrap();
    assert!(!apply_subscription_refresh(
        &mut config,
        vec![("https://example.org/synthetic".into(), Vec::new())]
    ));
    assert_eq!(before, serde_yaml::to_string(&config).unwrap());
}

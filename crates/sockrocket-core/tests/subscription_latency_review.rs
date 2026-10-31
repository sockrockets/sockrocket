//! Loopback-only subscription latency and fallback regressions.
use sockrocket_core::{Subscription, fetch_subscription_with};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

const BODY: &str = "proxies:\n  - name: fixture\n    type: ss\n    server: 192.0.2.1\n    port: 8388\n    cipher: aes-128-gcm\n    password: fixture\n";

fn sub(url: String) -> Subscription {
    Subscription {
        name: "fixture".into(),
        url,
        format: "auto".into(),
        last_updated: None,
        refresh_interval_hours: 24,
        refresh_revision: String::new(),
    }
}

async fn http(delay: Duration, status: u16) -> (String, tokio::task::JoinHandle<()>) {
    let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = server.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut client, _) = server.accept().await.unwrap();
        let mut request = [0; 4096];
        let _ = client.read(&mut request).await;
        tokio::time::sleep(delay).await;
        let response = format!(
            "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{BODY}",
            BODY.len()
        );
        let _ = client.write_all(response.as_bytes()).await;
    });
    (format!("http://{addr}/private-fixture-token"), task)
}

async fn socks(
    status: u16,
) -> (
    std::net::SocketAddr,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    socks_responses(vec![(status, BODY)], Duration::from_millis(50)).await
}

async fn socks_responses(
    responses: Vec<(u16, &'static str)>,
    delay: Duration,
) -> (
    std::net::SocketAddr,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = server.local_addr().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let count = requests.clone();
    let task = tokio::spawn(async move {
        loop {
            let (mut client, _) = server.accept().await.unwrap();
            let mut hello = [0; 2];
            if client.read_exact(&mut hello).await.is_err() {
                continue;
            } // Availability probe.
            assert_eq!(hello[0], 5);
            let mut methods = vec![0; hello[1] as usize];
            client.read_exact(&mut methods).await.unwrap();
            client.write_all(&[5, 0]).await.unwrap();
            let mut header = [0; 4];
            client.read_exact(&mut header).await.unwrap();
            let len = match header[3] {
                1 => 4,
                3 => client.read_u8().await.unwrap() as usize,
                4 => 16,
                _ => panic!("SOCKS address"),
            };
            client.read_exact(&mut vec![0; len + 2]).await.unwrap();
            client
                .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 80])
                .await
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(client.read_u8().await.unwrap());
                assert!(request.len() < 8192);
            }
            let index = count.fetch_add(1, Ordering::SeqCst);
            let (status, body) = responses[index.min(responses.len() - 1)];
            tokio::time::sleep(delay).await;
            let response = format!(
                "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = client.write_all(response.as_bytes()).await;
        }
    });
    (addr, requests, task)
}

#[tokio::test]
async fn slow_direct_path_does_not_delay_a_healthy_local_proxy() {
    let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/private-fixture-token",
        server.local_addr().unwrap()
    );
    let direct = tokio::spawn(async move {
        let (mut client, _) = server.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(client.read_u8().await.unwrap());
            assert!(request.len() < 8192);
        }
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), client.read(&mut [0]))
                .await
                .expect("losing direct request was not cancelled")
                .unwrap(),
            0
        );
    });
    let (proxy, requests, proxy_task) = socks(200).await;
    let started = Instant::now();
    let nodes = tokio::time::timeout(
        Duration::from_secs(2),
        fetch_subscription_with(&sub(url), Some(proxy)),
    )
    .await
    .expect("a working proxy waited for the slow direct path")
    .unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    eprintln!(
        "slow-direct healthy-proxy elapsed_ms={}",
        started.elapsed().as_millis()
    );
    direct.await.unwrap();
    proxy_task.abort();
}

#[tokio::test]
async fn a_fast_valid_route_wins_without_waiting_for_the_other_route() {
    let (url, direct) = http(Duration::ZERO, 200).await;
    let (proxy, requests, proxy_task) = socks(200).await;
    assert_eq!(
        fetch_subscription_with(&sub(url), Some(proxy))
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(requests.load(Ordering::SeqCst) <= 1);
    direct.abort();
    proxy_task.abort();
}

#[tokio::test]
async fn direct_http_errors_allow_a_successful_proxy_download() {
    let (url, direct) = http(Duration::ZERO, 403).await;
    let (proxy, requests, proxy_task) = socks(200).await;
    let nodes = fetch_subscription_with(&sub(url), Some(proxy))
        .await
        .unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    direct.await.unwrap();
    proxy_task.abort();
}

#[tokio::test]
async fn proxy_failure_does_not_abort_a_direct_download_that_can_finish() {
    let (url, direct) = http(Duration::from_millis(900), 200).await;
    let (proxy, requests, proxy_task) = socks(403).await;
    assert_eq!(
        fetch_subscription_with(&sub(url), Some(proxy))
            .await
            .unwrap()
            .len(),
        1
    );
    assert!((1..=4).contains(&requests.load(Ordering::SeqCst)));
    direct.await.unwrap();
    proxy_task.abort();
}

async fn http_responses(
    responses: Vec<(u16, &'static str)>,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let requests = count.clone();
    let task = tokio::spawn(async move {
        loop {
            let (mut client, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                match client.read_u8().await {
                    Ok(byte) => request.push(byte),
                    Err(_) => break,
                }
                if request.ends_with(b"\r\n\r\n") || request.len() >= 8192 {
                    break;
                }
            }
            if !request.ends_with(b"\r\n\r\n") {
                continue;
            }
            let index = requests.fetch_add(1, Ordering::SeqCst);
            let (status, body) = responses[index.min(responses.len() - 1)];
            let response = format!(
                "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = client.write_all(response.as_bytes()).await;
        }
    });
    (format!("http://{addr}/private-fixture-token"), count, task)
}

#[tokio::test]
async fn a_direct_http_200_block_page_does_not_beat_a_valid_proxy_subscription() {
    let (url, count, direct) = http_responses(vec![(200, "<html>Access denied</html>")]).await;
    let (proxy, requests, proxy_task) = socks(200).await;
    let nodes = fetch_subscription_with(&sub(url), Some(proxy))
        .await
        .unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    direct.abort();
    proxy_task.abort();
}

#[tokio::test]
async fn a_direct_subscription_can_succeed_on_the_third_retry() {
    let (url, requests, direct) =
        http_responses(vec![(503, ""), (503, ""), (503, ""), (200, BODY)]).await;
    let nodes = fetch_subscription_with(&sub(url), None).await.unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(requests.load(Ordering::SeqCst), 4);
    direct.abort();
}

#[tokio::test]
async fn a_proxy_subscription_can_succeed_on_the_third_retry_while_direct_fails() {
    let (url, direct_requests, direct) = http_responses(vec![(403, "")]).await;
    let (proxy, requests, proxy_task) = socks_responses(
        vec![(503, ""), (503, ""), (503, ""), (200, BODY)],
        Duration::from_millis(20),
    )
    .await;
    let nodes = fetch_subscription_with(&sub(url), Some(proxy))
        .await
        .unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(requests.load(Ordering::SeqCst), 4);
    assert!((1..=4).contains(&direct_requests.load(Ordering::SeqCst)));
    direct.abort();
    proxy_task.abort();
}

#[tokio::test]
async fn both_http_routes_stop_after_three_retries_and_keep_errors_private() {
    let (url, direct_requests, direct) = http_responses(vec![(403, "")]).await;
    let (proxy, requests, proxy_task) = socks(503).await;
    let error = fetch_subscription_with(&sub(url), Some(proxy))
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("403"));
    assert!(!format!("{error:#}").contains("private-fixture-token"));
    assert_eq!(direct_requests.load(Ordering::SeqCst), 4);
    assert_eq!(requests.load(Ordering::SeqCst), 4);
    direct.abort();
    proxy_task.abort();
}

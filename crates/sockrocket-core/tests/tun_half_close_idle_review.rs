//! Bound silent half-closed relays without shortening established idle time.
use sockrocket_core::proxy::relay::relay_bidirectional_with_idle_timeout;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{Duration, timeout};

#[tokio::test(start_paused = true)]
async fn either_eof_bounds_a_silent_half_closed_relay() {
    for upstream_eof in [false, true] {
        let (mut client, mut local) = tokio::io::duplex(1024);
        let (mut remote, mut upstream) = tokio::io::duplex(1024);
        if upstream_eof {
            upstream.shutdown().await.unwrap();
        } else {
            client.shutdown().await.unwrap();
        }
        let error = timeout(
            Duration::from_secs(95),
            relay_bidirectional_with_idle_timeout(
                &mut local,
                &mut remote,
                Duration::from_secs(1800),
            ),
        )
        .await
        .expect("half-closed flow retained the established 30-minute idle budget")
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    }
}

#[tokio::test(start_paused = true)]
async fn established_idle_connections_keep_their_long_idle_budget() {
    let (mut client, mut local) = tokio::io::duplex(1024);
    let (mut remote, mut upstream) = tokio::io::duplex(1024);
    let task = tokio::spawn(async move {
        relay_bidirectional_with_idle_timeout(&mut local, &mut remote, Duration::from_secs(1800))
            .await
    });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(120)).await;
    tokio::task::yield_now().await;
    assert!(!task.is_finished());
    client.write_all(b"still alive").await.unwrap();
    let mut bytes = [0; 11];
    upstream.read_exact(&mut bytes).await.unwrap();
    client.shutdown().await.unwrap();
    upstream.shutdown().await.unwrap();
    assert_eq!(task.await.unwrap().unwrap(), (11, 0));
}

#[tokio::test(start_paused = true)]
async fn eof_after_established_idle_gets_a_fresh_grace_period() {
    let (mut client, mut local) = tokio::io::duplex(1024);
    let (mut remote, mut upstream) = tokio::io::duplex(1024);
    let task = tokio::spawn(async move {
        relay_bidirectional_with_idle_timeout(&mut local, &mut remote, Duration::from_secs(1800))
            .await
    });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(120)).await;
    client.shutdown().await.unwrap();
    assert_eq!(upstream.read(&mut [0]).await.unwrap(), 0);
    tokio::time::advance(Duration::from_secs(30)).await;
    tokio::task::yield_now().await;
    assert!(
        !task.is_finished(),
        "EOF inherited an already-expired idle age"
    );
    timeout(Duration::from_secs(65), task)
        .await
        .expect("half-close grace did not expire")
        .unwrap()
        .unwrap_err();
}

#[tokio::test(start_paused = true)]
async fn response_progress_extends_half_close_grace_until_clean_completion() {
    for upstream_eof in [false, true] {
        let (mut client, mut local) = tokio::io::duplex(1024);
        let (mut remote, mut upstream) = tokio::io::duplex(1024);
        let task = tokio::spawn(async move {
            relay_bidirectional_with_idle_timeout(
                &mut local,
                &mut remote,
                Duration::from_secs(1800),
            )
            .await
        });
        let (sender, receiver) = if upstream_eof {
            upstream.shutdown().await.unwrap();
            assert_eq!(client.read(&mut [0]).await.unwrap(), 0);
            (&mut client, &mut upstream)
        } else {
            client.shutdown().await.unwrap();
            assert_eq!(upstream.read(&mut [0]).await.unwrap(), 0);
            (&mut upstream, &mut client)
        };
        for _ in 0..4 {
            tokio::time::advance(Duration::from_secs(45)).await;
            sender.write_all(b"response").await.unwrap();
            let mut bytes = [0; 8];
            receiver.read_exact(&mut bytes).await.unwrap();
            assert_eq!(&bytes, b"response");
            assert!(!task.is_finished());
        }
        sender.shutdown().await.unwrap();
        let expected = if upstream_eof { (32, 0) } else { (0, 32) };
        assert_eq!(task.await.unwrap().unwrap(), expected);
    }
}

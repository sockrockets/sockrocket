//! Memory-only TCP lifecycle regression; no device, real sockets or routes.
use etherparse::{PacketBuilder, SlicedPacket, TransportSlice};
use ipstack::{IpStack, IpStackConfig, IpStackStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::time::{Duration, timeout};
fn packet(seq: u32, ack: Option<u32>, syn: bool, fin: bool, payload: &[u8]) -> Vec<u8> {
    let b = PacketBuilder::ipv4([192, 0, 2, 2], [198, 51, 100, 2], 64).tcp(12345, 443, seq, 65535);
    let b = if syn { b.syn() } else { b };
    let b = if fin { b.fin() } else { b };
    let b = if let Some(a) = ack { b.ack(a) } else { b };
    let b = if payload.is_empty() { b } else { b.psh() };
    let mut out = Vec::new();
    b.write(&mut out, payload).unwrap();
    out
}
async fn read_packet(client: &mut DuplexStream) -> Vec<u8> {
    let mut header = [0; 20];
    client.read_exact(&mut header).await.unwrap();
    let size = u16::from_be_bytes([header[2], header[3]]) as usize;
    let mut out = header.to_vec();
    out.resize(size, 0);
    client.read_exact(&mut out[20..]).await.unwrap();
    out
}
async fn connected() -> (IpStack, DuplexStream, ipstack::IpStackTcpStream, u32) {
    connected_with_config(IpStackConfig::default()).await
}
async fn connected_with_config(
    config: IpStackConfig,
) -> (IpStack, DuplexStream, ipstack::IpStackTcpStream, u32) {
    let (device, mut client) = tokio::io::duplex(65536);
    let mut stack = IpStack::new(config, device);
    client
        .write_all(&packet(1000, None, true, false, &[]))
        .await
        .unwrap();
    let IpStackStream::Tcp(mut stream) = stack.accept().await.unwrap() else {
        panic!("expected TCP")
    };
    let synack = read_packet(&mut client).await;
    let parsed = SlicedPacket::from_ip(&synack).unwrap();
    let Some(TransportSlice::Tcp(h)) = parsed.transport else {
        panic!("expected TCP")
    };
    let server_seq = h.sequence_number();
    client
        .write_all(&packet(
            1001,
            Some(server_seq.wrapping_add(1)),
            false,
            false,
            b"x",
        ))
        .await
        .unwrap();
    let mut marker = [0];
    stream.read_exact(&mut marker).await.unwrap();
    assert_eq!(&marker, b"x");
    read_packet(&mut client).await; // ACK for the marker; handshake is complete.
    (stack, client, stream, server_seq)
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_completes_after_local_fin_without_waiting_for_peer_fin() {
    let (_stack, mut client, mut stream, server_seq) = connected().await;
    timeout(Duration::from_millis(150), stream.shutdown())
        .await
        .expect("shutdown waits for peer FIN and retains a TUN slot")
        .unwrap();
    let fin = read_packet(&mut client).await;
    let fin = SlicedPacket::from_ip(&fin).unwrap();
    let Some(TransportSlice::Tcp(h)) = fin.transport else {
        panic!("expected TCP")
    };
    assert!(h.fin());
    assert!(
        stream.write_all(b"late").await.is_err(),
        "write-half shutdown must reject later writes"
    );
    client
        .write_all(&packet(
            1002,
            Some(server_seq.wrapping_add(2)),
            false,
            false,
            b"tail",
        ))
        .await
        .unwrap();
    let mut tail = [0; 4];
    timeout(Duration::from_secs(1), stream.read_exact(&mut tail))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&tail, b"tail");
}
#[tokio::test]
async fn dropping_a_tcp_stream_never_blocks_or_panics_on_current_thread_runtime() {
    let (_stack, _client, stream, _) = connected().await;
    drop(stream);
    tokio::task::yield_now().await;
}
#[tokio::test(start_paused = true)]
async fn missing_payload_ack_has_bounded_shutdown_drain_without_read_polling() {
    let (_stack, mut client, mut stream, _) = connected().await;
    stream.write_all(b"payload").await.unwrap();
    read_packet(&mut client).await;
    let waiter = tokio::spawn(async move { stream.shutdown().await });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(4999)).await;
    assert!(!waiter.is_finished());
    tokio::time::advance(Duration::from_millis(2)).await;
    let error = waiter.await.unwrap().unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
}
#[tokio::test]
async fn ack_wakes_pending_shutdown_without_waiting_for_drain_deadline() {
    let (_stack, mut client, mut stream, server_seq) = connected().await;
    stream.write_all(b"payload").await.unwrap();
    read_packet(&mut client).await;
    let waiter = tokio::spawn(async move {
        let result = stream.shutdown().await;
        (result, stream)
    });
    tokio::task::yield_now().await;
    assert!(!waiter.is_finished());
    client
        .write_all(&packet(
            1002,
            Some(server_seq.wrapping_add(8)),
            false,
            false,
            &[],
        ))
        .await
        .unwrap();
    let (result, _stream) = timeout(Duration::from_millis(200), waiter)
        .await
        .expect("payload ACK lost its shutdown wakeup")
        .unwrap();
    result.unwrap();
}
#[tokio::test]
async fn repeated_fin_without_peer_fin_recycles_more_than_router_session_budget() {
    for _ in 0..512 {
        let (_stack, _client, mut stream, _) = connected().await;
        timeout(Duration::from_millis(150), stream.shutdown())
            .await
            .expect("completed relay retained session slot")
            .unwrap();
        drop(stream);
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn spurious_read_polls_do_not_extend_idle_connection_lifetime() {
    let mut config = IpStackConfig::default();
    let mut tcp = ipstack::TcpConfig::default();
    tcp.timeout = Duration::from_secs(10);
    config.with_tcp_config(tcp);
    let (_stack, _client, mut stream, _) = connected_with_config(config).await;
    let mut buf = [0u8; 1];
    for _ in 0..9 {
        tokio::time::advance(Duration::from_secs(1)).await;
        std::future::poll_fn(|cx| {
            use tokio::io::AsyncRead;
            let mut read = tokio::io::ReadBuf::new(&mut buf);
            assert!(
                std::pin::Pin::new(&mut stream)
                    .poll_read(cx, &mut read)
                    .is_pending()
            );
            std::task::Poll::Ready(())
        })
        .await;
    }
    tokio::time::advance(Duration::from_secs(2)).await;
    let error = timeout(Duration::from_millis(1), stream.read(&mut buf))
        .await
        .expect("pending polls reset the idle deadline")
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
}

#[tokio::test]
async fn peer_reset_wakes_a_pending_reader_without_another_packet() {
    let (_stack, mut client, mut stream, server_seq) = connected().await;
    let waiter = tokio::spawn(async move {
        let mut b = [0u8; 8];
        stream.read(&mut b).await
    });
    tokio::task::yield_now().await;
    let builder = PacketBuilder::ipv4([192, 0, 2, 2], [198, 51, 100, 2], 64)
        .tcp(12345, 443, 1002, 65535)
        .ack(server_seq.wrapping_add(1))
        .rst();
    let mut reset = Vec::new();
    builder.write(&mut reset, &[]).unwrap();
    client.write_all(&reset).await.unwrap();
    let result = timeout(Duration::from_millis(150), waiter)
        .await
        .expect("peer RST did not wake pending TCP read")
        .unwrap();
    assert!(result.is_err() || result.unwrap() == 0);
}
#[tokio::test]
async fn peer_fin_delivers_eof_while_preserving_the_response_direction() {
    let (_stack, mut client, mut stream, server_seq) = connected().await;
    client
        .write_all(&packet(
            1002,
            Some(server_seq.wrapping_add(1)),
            false,
            true,
            &[],
        ))
        .await
        .unwrap();
    let mut b = [0u8; 8];
    assert_eq!(
        timeout(Duration::from_millis(150), stream.read(&mut b))
            .await
            .expect("peer FIN retained the relay read half")
            .unwrap(),
        0
    );
    stream
        .write_all(b"response")
        .await
        .expect("peer FIN must not close our write half");
}

#[tokio::test]
async fn relay_must_finish_both_pending_write_shutdowns_before_returning() {
    use std::{
        pin::Pin,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        task::{Context, Poll},
    };
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
    struct EofWithDelayedShutdown {
        pending: bool,
        completed: Arc<AtomicUsize>,
    }
    impl AsyncRead for EofWithDelayedShutdown {
        fn poll_read(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }
    impl AsyncWrite for EofWithDelayedShutdown {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            b: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            Poll::Ready(Ok(b.len()))
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            if self.pending {
                self.pending = false;
                cx.waker().wake_by_ref();
                Poll::Pending
            } else {
                self.completed.fetch_add(1, Ordering::SeqCst);
                Poll::Ready(Ok(()))
            }
        }
    }
    let count = Arc::new(AtomicUsize::new(0));
    let mut a = EofWithDelayedShutdown {
        pending: true,
        completed: count.clone(),
    };
    let mut b = EofWithDelayedShutdown {
        pending: true,
        completed: count.clone(),
    };
    sockrocket_core::proxy::relay::relay_bidirectional(&mut a, &mut b)
        .await
        .unwrap();
    assert_eq!(
        count.load(Ordering::SeqCst),
        2,
        "relay reported success before write shutdown completed"
    );
}

#[tokio::test]
async fn fin_payload_waits_for_missing_data_and_duplicate_fin_does_not_close_reply() {
    let (_stack, mut client, mut stream, server_seq) = connected().await;
    client
        .write_all(&packet(
            1005,
            Some(server_seq.wrapping_add(1)),
            false,
            true,
            b"end",
        ))
        .await
        .unwrap();
    let mut b = [0u8; 8];
    assert!(
        timeout(Duration::from_millis(20), stream.read(&mut b))
            .await
            .is_err(),
        "out-of-order FIN lost preceding bytes"
    );
    client
        .write_all(&packet(
            1002,
            Some(server_seq.wrapping_add(1)),
            false,
            false,
            b"gap",
        ))
        .await
        .unwrap();
    let mut data = Vec::new();
    timeout(Duration::from_millis(150), stream.read_to_end(&mut data))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(data, b"gapend");
    client
        .write_all(&packet(
            1005,
            Some(server_seq.wrapping_add(1)),
            false,
            true,
            b"end",
        ))
        .await
        .unwrap();
    tokio::task::yield_now().await;
    assert_eq!(
        stream.read(&mut b).await.unwrap(),
        0,
        "EOF must stay sticky"
    );
    stream.write_all(b"response").await.unwrap();
}
#[tokio::test(start_paused = true)]
async fn peer_half_close_does_not_cut_off_a_delayed_response() {
    let (_stack, mut client, mut stream, server_seq) = connected().await;
    client
        .write_all(&packet(
            1002,
            Some(server_seq.wrapping_add(1)),
            false,
            true,
            &[],
        ))
        .await
        .unwrap();
    let mut b = [0u8; 1];
    assert_eq!(stream.read(&mut b).await.unwrap(), 0);
    tokio::time::advance(Duration::from_secs(6)).await;
    tokio::task::yield_now().await;
    stream
        .write_all(b"delayed response")
        .await
        .expect("old close-wait timer truncated a valid response");
}
#[tokio::test]
async fn reset_reclaims_actual_relay_tasks_over_768_connections() {
    for _ in 0..768 {
        let (_stack, mut client, mut stream, server_seq) = connected().await;
        let (mut upstream, _peer) = tokio::io::duplex(1024);
        let task = tokio::spawn(async move {
            sockrocket_core::proxy::relay::relay_bidirectional(&mut stream, &mut upstream).await
        });
        tokio::task::yield_now().await;
        let builder = PacketBuilder::ipv4([192, 0, 2, 2], [198, 51, 100, 2], 64)
            .tcp(12345, 443, 1002, 65535)
            .ack(server_seq.wrapping_add(1))
            .rst();
        let mut reset = Vec::new();
        builder.write(&mut reset, &[]).unwrap();
        client.write_all(&reset).await.unwrap();
        let err = timeout(Duration::from_millis(150), task)
            .await
            .expect("reset retained a real relay task")
            .unwrap()
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::ConnectionReset);
    }
}

#[tokio::test]
async fn peer_reset_wakes_a_writer_blocked_on_unacknowledged_payload() {
    let (_stack, mut client, mut stream, server_seq) = connected().await;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        stream.write_all(&vec![1u8; 16384]).await.unwrap();
        let _ = tx.send(());
        stream.write_all(b"blocked").await
    });
    rx.await.unwrap();
    tokio::task::yield_now().await;
    assert!(!task.is_finished());
    let builder = PacketBuilder::ipv4([192, 0, 2, 2], [198, 51, 100, 2], 64)
        .tcp(12345, 443, 1002, 65535)
        .ack(server_seq.wrapping_add(1))
        .rst();
    let mut reset = Vec::new();
    builder.write(&mut reset, &[]).unwrap();
    client.write_all(&reset).await.unwrap();
    timeout(Duration::from_millis(150), task)
        .await
        .expect("RST retained a blocked writer")
        .unwrap()
        .unwrap_err();
}
#[tokio::test(start_paused = true)]
async fn fin_retry_reuses_the_original_sequence_number() {
    let (_stack, mut client, mut stream, _) = connected().await;
    stream.shutdown().await.unwrap();
    let initial = read_packet(&mut client).await;
    let initial = SlicedPacket::from_ip(&initial).unwrap();
    let Some(TransportSlice::Tcp(a)) = initial.transport else {
        panic!("TCP")
    };
    let seq = a.sequence_number();
    assert!(a.fin());
    tokio::time::advance(Duration::from_millis(501)).await;
    let retry = read_packet(&mut client).await;
    let retry = SlicedPacket::from_ip(&retry).unwrap();
    let Some(TransportSlice::Tcp(b)) = retry.transport else {
        panic!("TCP")
    };
    assert!(b.fin());
    assert_eq!(seq, b.sequence_number());
}

#[tokio::test(start_paused = true)]
async fn half_closed_relay_with_a_silent_upstream_has_an_idle_bound() {
    let (mut client, mut local) = tokio::io::duplex(1024);
    let (mut remote, _upstream) = tokio::io::duplex(1024);
    client.shutdown().await.unwrap();
    let result = sockrocket_core::proxy::relay::relay_bidirectional_with_idle_timeout(
        &mut local,
        &mut remote,
        Duration::from_secs(10),
    )
    .await;
    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
}

#[tokio::test(start_paused = true)]
async fn silent_fin_wait_2_releases_the_actual_tcp_flow_before_established_idle_timeout() {
    let mut tcp = ipstack::TcpConfig::default();
    tcp.timeout = Duration::from_secs(1800);
    let mut config = IpStackConfig::default();
    config.with_tcp_config(tcp);
    let (stack, mut client, mut stream, server_seq) = connected_with_config(config).await;
    let (mut remote, mut upstream) = tokio::io::duplex(1024);
    let task = tokio::spawn(async move {
        sockrocket_core::proxy::relay::relay_bidirectional_with_idle_timeout(
            &mut stream,
            &mut remote,
            Duration::from_secs(1800),
        )
        .await
    });
    upstream.shutdown().await.unwrap();
    let fin = read_packet(&mut client).await;
    let Some(TransportSlice::Tcp(header)) = SlicedPacket::from_ip(&fin).unwrap().transport else {
        panic!("TCP FIN")
    };
    assert!(header.fin());
    client
        .write_all(&packet(
            1002,
            Some(server_seq.wrapping_add(2)),
            false,
            false,
            &[],
        ))
        .await
        .unwrap();
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
    assert_eq!(stack.tcp_health().0[4], 1, "expected FIN_WAIT_2");
    let error = timeout(Duration::from_secs(95), task)
        .await
        .expect("silent FIN_WAIT_2 retained the relay")
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("half-closed relay idle timeout"));
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
    assert_eq!(stack.tcp_health().0.iter().sum::<usize>(), 0);
}

#[tokio::test(start_paused = true)]
async fn payload_progress_keeps_the_idle_watchdog_alive() {
    let (mut client, mut local) = tokio::io::duplex(1024);
    let (mut remote, mut upstream) = tokio::io::duplex(1024);
    let task = tokio::spawn(async move {
        sockrocket_core::proxy::relay::relay_bidirectional_with_idle_timeout(
            &mut local,
            &mut remote,
            Duration::from_secs(10),
        )
        .await
    });
    tokio::task::yield_now().await;
    for _ in 0..3 {
        tokio::time::advance(Duration::from_secs(9)).await;
        client.write_all(b"abc").await.unwrap();
        let mut b = [0u8; 3];
        upstream.read_exact(&mut b).await.unwrap();
        assert_eq!(&b, b"abc");
        assert!(!task.is_finished());
    }
    client.shutdown().await.unwrap();
    upstream.shutdown().await.unwrap();
    assert_eq!(task.await.unwrap().unwrap(), (9, 0));
}
#[tokio::test(start_paused = true)]
async fn missing_fin_ack_wakes_the_remaining_read_half_after_retry_budget() {
    let (_stack, _client, mut stream, _) = connected().await;
    stream.shutdown().await.unwrap();
    let mut b = [0u8; 1];
    assert_eq!(
        timeout(Duration::from_secs(2), stream.read(&mut b))
            .await
            .expect("FIN retry exhaustion did not wake reader")
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn fin_on_the_handshake_ack_is_delivered_as_eof() {
    let (device, mut client) = tokio::io::duplex(65536);
    let mut stack = IpStack::new(IpStackConfig::default(), device);
    client
        .write_all(&packet(1000, None, true, false, &[]))
        .await
        .unwrap();
    let IpStackStream::Tcp(mut stream) = stack.accept().await.unwrap() else {
        panic!("TCP")
    };
    let synack = read_packet(&mut client).await;
    let parsed = SlicedPacket::from_ip(&synack).unwrap();
    let Some(TransportSlice::Tcp(h)) = parsed.transport else {
        panic!("TCP")
    };
    client
        .write_all(&packet(
            1001,
            Some(h.sequence_number().wrapping_add(1)),
            false,
            true,
            &[],
        ))
        .await
        .unwrap();
    let mut b = [0u8; 1];
    assert_eq!(
        timeout(Duration::from_millis(150), stream.read(&mut b))
            .await
            .unwrap()
            .unwrap(),
        0
    );
    stream.write_all(b"reply").await.unwrap();
}

#[tokio::test]
async fn slow_consumer_advertises_zero_window_then_reopens_without_data_loss() {
    let (stack, mut client, mut stream, server_seq) = connected().await;
    let payload = vec![42u8; 1024];
    let mut last_ack = 1002;
    let mut zero = false;
    for i in 0..16 {
        client
            .write_all(&packet(
                1002 + i * 1024,
                Some(server_seq + 1),
                false,
                false,
                &payload,
            ))
            .await
            .unwrap();
        let bytes = timeout(Duration::from_secs(1), read_packet(&mut client))
            .await
            .unwrap();
        let parsed = SlicedPacket::from_ip(&bytes).unwrap();
        let Some(TransportSlice::Tcp(header)) = parsed.transport else {
            panic!()
        };
        last_ack = last_ack.max(header.acknowledgment_number());
        zero |= header.window_size() == 0;
    }
    assert_eq!(
        last_ack,
        1002 + 16 * 1024,
        "acknowledged beyond bounded application buffer"
    );
    assert!(zero);
    let (states, _, read_bytes) = stack.tcp_health();
    assert_eq!(states[2], 1);
    assert_eq!(read_bytes, 16 * 1024);
    assert!(stack.packet_queue_usage().0 <= stack.packet_queue_usage().1);
    let mut accepted = vec![0u8; 16 * 1024];
    timeout(Duration::from_secs(1), stream.read_exact(&mut accepted))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(accepted, vec![42; 16 * 1024]);
    let update = timeout(Duration::from_secs(1), read_packet(&mut client))
        .await
        .unwrap();
    let parsed = SlicedPacket::from_ip(&update).unwrap();
    let Some(TransportSlice::Tcp(header)) = parsed.transport else {
        panic!()
    };
    assert!(header.window_size() > 0);
    for i in 16..20 {
        client
            .write_all(&packet(
                1002 + i * 1024,
                Some(server_seq + 1),
                false,
                false,
                &payload,
            ))
            .await
            .unwrap();
        loop {
            let bytes = timeout(Duration::from_secs(1), read_packet(&mut client))
                .await
                .unwrap();
            let parsed = SlicedPacket::from_ip(&bytes).unwrap();
            let Some(TransportSlice::Tcp(header)) = parsed.transport else {
                panic!()
            };
            if header.acknowledgment_number() >= 1002 + (i + 1) * 1024 {
                break;
            }
        }
    }
    let mut tail = vec![0; 4096];
    timeout(Duration::from_secs(1), stream.read_exact(&mut tail))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(tail, vec![42; 4096]);
}
#[tokio::test]
async fn tiny_segments_are_bounded_by_chunk_count_and_reopen_after_consumption() {
    let (stack, mut client, mut stream, server_seq) = connected().await;
    for i in 0..64 {
        client
            .write_all(&packet(1002 + i, Some(server_seq + 1), false, false, b"y"))
            .await
            .unwrap();
        let p = read_packet(&mut client).await;
        if i == 63 {
            let parsed = SlicedPacket::from_ip(&p).unwrap();
            let Some(TransportSlice::Tcp(h)) = parsed.transport else {
                panic!()
            };
            assert_eq!(h.window_size(), 0);
        }
    }
    assert_eq!(stack.tcp_health().2, 64);
    let mut bytes = [0; 64];
    timeout(Duration::from_secs(1), stream.read_exact(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&bytes, &[b'y'; 64]);
    assert_eq!(stack.tcp_health().2, 0);
    let p = timeout(Duration::from_secs(1), read_packet(&mut client))
        .await
        .unwrap();
    let parsed = SlicedPacket::from_ip(&p).unwrap();
    let Some(TransportSlice::Tcp(h)) = parsed.transport else {
        panic!()
    };
    assert!(h.window_size() > 0);
}

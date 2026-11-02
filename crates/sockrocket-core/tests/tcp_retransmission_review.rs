//! Lost-packet regressions using memory packets, without real sockets or routes.
use etherparse::{PacketBuilder, SlicedPacket, TransportSlice};
use ipstack::{IpStack, IpStackConfig, IpStackStream, IpStackTcpStream, TcpConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::time::{Duration, timeout};

fn packet(seq: u32, ack: Option<u32>, syn: bool, payload: &[u8]) -> Vec<u8> {
    let b = PacketBuilder::ipv4([192, 0, 2, 2], [198, 51, 100, 2], 64).tcp(12345, 443, seq, 65535);
    let b = if syn { b.syn() } else { b };
    let b = if let Some(ack) = ack { b.ack(ack) } else { b };
    let mut bytes = Vec::new();
    b.write(&mut bytes, payload).unwrap();
    bytes
}

async fn read_packet(client: &mut DuplexStream) -> Vec<u8> {
    let mut header = [0; 20];
    client.read_exact(&mut header).await.unwrap();
    let size = u16::from_be_bytes([header[2], header[3]]) as usize;
    let mut bytes = header.to_vec();
    bytes.resize(size, 0);
    client.read_exact(&mut bytes[20..]).await.unwrap();
    bytes
}

fn payload(bytes: &[u8]) -> &[u8] {
    let parsed = SlicedPacket::from_ip(bytes).unwrap();
    let Some(TransportSlice::Tcp(tcp)) = parsed.transport else {
        panic!("TCP packet")
    };
    tcp.payload()
}

async fn connected(retries: usize) -> (IpStack, DuplexStream, IpStackTcpStream, u32) {
    let mut tcp = TcpConfig::default();
    tcp.rto = Duration::from_millis(100);
    tcp.max_retransmit_count = retries;
    connected_with_tcp(tcp).await
}

async fn connected_with_tcp(tcp: TcpConfig) -> (IpStack, DuplexStream, IpStackTcpStream, u32) {
    let mut config = IpStackConfig::default();
    config.with_tcp_config(tcp);
    let (device, mut client) = tokio::io::duplex(65536);
    let mut stack = IpStack::new(config, device);
    client
        .write_all(&packet(1000, None, true, &[]))
        .await
        .unwrap();
    let IpStackStream::Tcp(mut stream) = stack.accept().await.unwrap() else {
        panic!("TCP stream")
    };
    let synack = read_packet(&mut client).await;
    let parsed = SlicedPacket::from_ip(&synack).unwrap();
    let Some(TransportSlice::Tcp(tcp)) = parsed.transport else {
        panic!("SYN-ACK")
    };
    let data_seq = tcp.sequence_number().wrapping_add(1);
    client
        .write_all(&packet(1001, Some(data_seq), false, b"x"))
        .await
        .unwrap();
    let mut marker = [0];
    stream.read_exact(&mut marker).await.unwrap();
    read_packet(&mut client).await;
    (stack, client, stream, data_seq)
}

#[tokio::test(start_paused = true)]
async fn lost_payload_retransmits_without_inbound_packets() {
    let (_stack, mut client, mut stream, _) = connected(3).await;
    stream.write_all(b"PING").await.unwrap();
    let original = read_packet(&mut client).await;
    let retry = timeout(Duration::from_millis(150), read_packet(&mut client))
        .await
        .expect("RTO must retransmit without another incoming packet");
    assert_eq!(payload(&retry), b"PING");
    let seq = |p: &[u8]| match SlicedPacket::from_ip(p).unwrap().transport {
        Some(TransportSlice::Tcp(tcp)) => tcp.sequence_number(),
        _ => panic!("TCP"),
    };
    assert_eq!(seq(&original), seq(&retry));
}

#[tokio::test(start_paused = true)]
async fn exhaustion_fails_pending_reader_and_reclaims_the_flow() {
    let (stack, mut client, mut stream, _) = connected(1).await;
    stream.write_all(b"lost").await.unwrap();
    read_packet(&mut client).await;
    let mut data = [0];
    let result = timeout(Duration::from_secs(1), stream.read(&mut data))
        .await
        .expect("retry exhaustion must wake the pending reader");
    assert!(
        result.is_err(),
        "a failed transfer must not look like clean EOF"
    );
    assert_eq!(stack.tcp_event_counts().task_errors, 1);
    assert_eq!(stack.tcp_event_counts().peer_resets, 0);
    assert_eq!(stack.tcp_health().0[2], 0);
    drop(stream);
    tokio::task::yield_now().await;
    assert_eq!(stack.tcp_health().0.iter().sum::<usize>(), 0);
}

#[tokio::test(start_paused = true)]
async fn final_retry_gets_its_full_timeout_and_can_still_be_acknowledged() {
    let (stack, mut client, mut stream, seq) = connected(1).await;
    stream.write_all(b"PING").await.unwrap();
    read_packet(&mut client).await;
    let retry = timeout(Duration::from_millis(150), read_packet(&mut client))
        .await
        .unwrap();
    assert_eq!(payload(&retry), b"PING");
    // Duplicate ACKs must not discard the last retry while it is in flight.
    client
        .write_all(&packet(1002, Some(seq), false, &[]))
        .await
        .unwrap();
    assert!(
        timeout(Duration::from_millis(150), read_packet(&mut client))
            .await
            .is_err()
    );
    client
        .write_all(&packet(1002, Some(seq.wrapping_add(4)), false, b"y"))
        .await
        .unwrap();
    let mut marker = [0];
    stream.read_exact(&mut marker).await.unwrap();
    assert_eq!(&marker, b"y");
    assert_eq!(stack.tcp_event_counts().task_errors, 0);
}

#[tokio::test(start_paused = true)]
async fn acknowledged_data_cancels_retries_and_a_later_write_rearms_the_timer() {
    let (stack, mut client, mut stream, seq) = connected(2).await;
    stream.write_all(b"first").await.unwrap();
    read_packet(&mut client).await;
    client
        .write_all(&packet(1002, Some(seq.wrapping_add(5)), false, b"y"))
        .await
        .unwrap();
    let mut marker = [0];
    stream.read_exact(&mut marker).await.unwrap();
    read_packet(&mut client).await; // ACK for y confirms processing.
    assert!(
        timeout(Duration::from_secs(1), read_packet(&mut client))
            .await
            .is_err()
    );
    stream.write_all(b"later").await.unwrap();
    read_packet(&mut client).await;
    let retry = timeout(Duration::from_millis(150), read_packet(&mut client))
        .await
        .unwrap();
    assert_eq!(payload(&retry), b"later");
    assert_eq!(stack.tcp_event_counts().task_errors, 0);
}

#[tokio::test(start_paused = true)]
async fn partial_ack_retransmits_only_the_unacknowledged_suffix() {
    let (_stack, mut client, mut stream, seq) = connected(2).await;
    stream.write_all(b"abcdef").await.unwrap();
    read_packet(&mut client).await;
    client
        .write_all(&packet(1002, Some(seq.wrapping_add(3)), false, b"y"))
        .await
        .unwrap();
    stream.read_exact(&mut [0]).await.unwrap();
    read_packet(&mut client).await;
    let retry = timeout(Duration::from_millis(150), read_packet(&mut client))
        .await
        .unwrap();
    assert_eq!(payload(&retry), b"def");
    let Some(TransportSlice::Tcp(tcp)) = SlicedPacket::from_ip(&retry).unwrap().transport else {
        panic!("TCP")
    };
    assert_eq!(tcp.sequence_number(), seq.wrapping_add(3));
}

#[tokio::test(start_paused = true)]
async fn exhaustion_sends_rst_to_the_peer() {
    let (_stack, mut client, mut stream, _) = connected(1).await;
    stream.write_all(b"lost").await.unwrap();
    read_packet(&mut client).await;
    let retry = timeout(Duration::from_millis(150), read_packet(&mut client))
        .await
        .unwrap();
    assert_eq!(payload(&retry), b"lost");
    let reset = timeout(Duration::from_millis(250), read_packet(&mut client))
        .await
        .unwrap();
    let Some(TransportSlice::Tcp(tcp)) = SlicedPacket::from_ip(&reset).unwrap().transport else {
        panic!("TCP")
    };
    assert!(tcp.rst());
}

#[tokio::test(start_paused = true)]
async fn exhaustion_wakes_a_writer_waiting_for_send_window_space() {
    let (stack, mut client, mut stream, _) = connected(1).await;
    stream.write_all(&vec![7; 16 * 1024]).await.unwrap();
    let mut received = 0;
    while received < 16 * 1024 {
        received += payload(&read_packet(&mut client).await).len();
    }
    let task = tokio::spawn(async move { stream.write_all(b"blocked writer").await });
    timeout(Duration::from_secs(1), task)
        .await
        .expect("retry exhaustion must wake blocked writers")
        .unwrap()
        .unwrap_err();
    assert_eq!(stack.tcp_event_counts().task_errors, 1);
}

#[tokio::test(start_paused = true)]
async fn exhaustion_releases_the_real_bidirectional_relay_task() {
    let (stack, mut client, mut stream, _) = connected(1).await;
    let (mut upstream, mut peer) = tokio::io::duplex(1024);
    let task = tokio::spawn(async move {
        sockrocket_core::proxy::relay::relay_bidirectional(&mut stream, &mut upstream).await
    });
    peer.write_all(b"lost response").await.unwrap();
    read_packet(&mut client).await;
    timeout(Duration::from_secs(1), task)
        .await
        .expect("failed flow retained a relay task")
        .unwrap()
        .unwrap_err();
    timeout(Duration::from_secs(1), async {
        while stack.tcp_health().0.iter().sum::<usize>() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("failed relay retained a TCP flow");
    assert_eq!(stack.tcp_event_counts().task_errors, 1);
}

#[tokio::test(start_paused = true)]
async fn window_probes_recover_a_lost_reopening_update_without_spending_data_retries() {
    let (stack, mut client, mut stream, seq) = connected(1).await;
    stream.write_all(b"held").await.unwrap();
    read_packet(&mut client).await;
    let b = PacketBuilder::ipv4([192, 0, 2, 2], [198, 51, 100, 2], 64)
        .tcp(12345, 443, 1002, 0)
        .ack(seq);
    let mut zero_window = Vec::new();
    b.write(&mut zero_window, b"y").unwrap();
    client.write_all(&zero_window).await.unwrap();
    stream.read_exact(&mut [0]).await.unwrap();
    read_packet(&mut client).await;
    let probe = timeout(Duration::from_millis(150), read_packet(&mut client))
        .await
        .unwrap();
    assert_eq!(
        payload(&probe),
        b"h",
        "window probes must be limited to one byte"
    );
    let Some(TransportSlice::Tcp(h)) = SlicedPacket::from_ip(&probe).unwrap().transport else {
        panic!("TCP")
    };
    assert!(h.ack() && !h.rst());
    assert_eq!(h.sequence_number(), seq);
    assert_eq!(stack.tcp_event_counts().task_errors, 0);
    client
        .write_all(&packet(1003, Some(seq), false, b"z"))
        .await
        .unwrap();
    stream.read_exact(&mut [0]).await.unwrap();
    let data = timeout(Duration::from_millis(150), async {
        loop {
            let p = read_packet(&mut client).await;
            if !payload(&p).is_empty() {
                break p;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(payload(&data), b"held");
}

#[tokio::test(start_paused = true)]
async fn duplicate_ack_burst_does_not_amplify_into_a_retransmit_storm() {
    let (stack, mut client, mut stream, seq) = connected(3).await;
    stream.write_all(b"PING").await.unwrap();
    read_packet(&mut client).await;
    for _ in 0..12 {
        client
            .write_all(&packet(1002, Some(seq), false, &[]))
            .await
            .unwrap();
        // Let the byte-stream test device consume one whole IP packet.
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
    }
    let retry = timeout(Duration::from_millis(50), read_packet(&mut client))
        .await
        .unwrap();
    assert_eq!(payload(&retry), b"PING");
    assert!(
        timeout(Duration::from_millis(50), read_packet(&mut client))
            .await
            .is_err(),
        "one fast retransmit is enough until another RTO or ACK progress"
    );
    assert_eq!(stack.tcp_event_counts().task_errors, 0);
}

#[tokio::test(start_paused = true)]
async fn silent_handshake_expires_without_any_application_io_polling() {
    let mut tcp = TcpConfig::default();
    tcp.timeout = Duration::from_secs(1800);
    let mut config = IpStackConfig::default();
    config.with_tcp_config(tcp);
    let (device, mut client) = tokio::io::duplex(65536);
    let mut stack = IpStack::new(config, device);
    client
        .write_all(&packet(1000, None, true, &[]))
        .await
        .unwrap();
    let IpStackStream::Tcp(mut stream) = stack.accept().await.unwrap() else {
        panic!("TCP")
    };
    read_packet(&mut client).await; // SYN-ACK; no final ACK.
    tokio::time::advance(Duration::from_secs(11)).await;
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
    assert_eq!(stack.tcp_health().0[1], 0, "handshake retained its slot");
    assert_eq!(stack.tcp_event_counts().task_errors, 1);
    timeout(Duration::from_millis(1), stream.read(&mut [0]))
        .await
        .expect("handshake expiry failed to publish terminal I/O")
        .unwrap_err();
}

#[tokio::test(start_paused = true)]
async fn repeated_syn_does_not_extend_the_handshake_deadline() {
    let (device, mut client) = tokio::io::duplex(65536);
    let mut stack = IpStack::new(IpStackConfig::default(), device);
    client
        .write_all(&packet(1000, None, true, &[]))
        .await
        .unwrap();
    let _stream = stack.accept().await.unwrap();
    read_packet(&mut client).await;
    tokio::time::advance(Duration::from_secs(9)).await;
    client
        .write_all(&packet(1000, None, true, &[]))
        .await
        .unwrap();
    let synack = read_packet(&mut client).await;
    let Some(TransportSlice::Tcp(tcp)) = SlicedPacket::from_ip(&synack).unwrap().transport else {
        panic!("SYN-ACK")
    };
    assert!(tcp.syn() && tcp.ack());
    tokio::time::advance(Duration::from_secs(2)).await;
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
    assert_eq!(stack.tcp_health().0[1], 0);
    assert_eq!(stack.tcp_event_counts().task_errors, 1);
}

#[tokio::test(start_paused = true)]
async fn stale_final_ack_cannot_bypass_handshake_expiry() {
    let mut tcp = TcpConfig::default();
    tcp.handshake_timeout = Duration::from_millis(100);
    let mut config = IpStackConfig::default();
    config.with_tcp_config(tcp);
    let (device, mut client) = tokio::io::duplex(65536);
    let mut stack = IpStack::new(config, device);
    client
        .write_all(&packet(1000, None, true, &[]))
        .await
        .unwrap();
    let _stream = stack.accept().await.unwrap();
    let synack = read_packet(&mut client).await;
    let Some(TransportSlice::Tcp(tcp)) = SlicedPacket::from_ip(&synack).unwrap().transport else {
        panic!("SYN-ACK")
    };
    let stale = tcp.sequence_number(); // Does not acknowledge the SYN byte.
    client
        .write_all(&packet(1001, Some(stale), false, &[]))
        .await
        .unwrap();
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
    assert_eq!(stack.tcp_health().0[1], 1);
    assert_eq!(stack.tcp_health().0[2], 0, "stale ACK completed handshake");
    tokio::time::advance(Duration::from_millis(150)).await;
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
    assert_eq!(stack.tcp_health().0[1], 0);
    assert_eq!(stack.tcp_health().0[2], 0);
    assert_eq!(stack.tcp_event_counts().task_errors, 1);
}

#[tokio::test(start_paused = true)]
async fn completed_handshake_disarms_its_deadline() {
    let (stack, mut client, mut stream, _) = connected(3).await;
    tokio::time::advance(Duration::from_secs(11)).await;
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
    assert_eq!(stack.tcp_health().0[2], 1);
    assert_eq!(stack.tcp_event_counts().task_errors, 0);
    stream.write_all(b"alive").await.unwrap();
    let data = read_packet(&mut client).await;
    assert_eq!(payload(&data), b"alive");
}

#[tokio::test(start_paused = true)]
async fn progress_ack_restarts_the_remaining_payload_timeout() {
    let (stack, mut client, mut stream, seq) = connected(1).await;
    stream.write_all(b"abcdef").await.unwrap();
    read_packet(&mut client).await;
    assert_eq!(payload(&read_packet(&mut client).await), b"abcdef");
    tokio::time::advance(Duration::from_millis(190)).await;
    client
        .write_all(&packet(1002, Some(seq.wrapping_add(3)), false, b"y"))
        .await
        .unwrap();
    stream.read_exact(&mut [0]).await.unwrap();
    read_packet(&mut client).await;
    assert!(
        timeout(Duration::from_millis(50), read_packet(&mut client))
            .await
            .is_err(),
        "ACK progress must restart the timer instead of using the old retry deadline"
    );
    client
        .write_all(&packet(1003, Some(seq.wrapping_add(6)), false, b"z"))
        .await
        .unwrap();
    let mut marker = [0];
    stream.read_exact(&mut marker).await.unwrap();
    assert_eq!(&marker, b"z");
    assert_eq!(stack.tcp_event_counts().task_errors, 0);
    assert_eq!(stack.tcp_payload_usage().0, 0);
}

#[tokio::test(start_paused = true)]
async fn default_retries_allow_a_twenty_second_blackout_then_recover() {
    let mut tcp = TcpConfig::default();
    tcp.timeout = Duration::from_secs(1800); // Match the TUN's idle policy.
    let (stack, mut client, mut stream, seq) = connected_with_tcp(tcp).await;
    stream.write_all(b"delayed").await.unwrap();
    read_packet(&mut client).await;
    for _ in 0..4 {
        let retry = read_packet(&mut client).await;
        assert_eq!(
            payload(&retry),
            b"delayed",
            "connection aborted after a brief blackout"
        );
    }
    // Retries occur at 1, 3, 7 and 15 seconds. Resume at 20 seconds.
    assert!(
        timeout(Duration::from_secs(5), read_packet(&mut client))
            .await
            .is_err()
    );
    client
        .write_all(&packet(1002, Some(seq.wrapping_add(7)), false, b"y"))
        .await
        .unwrap();
    let mut marker = [0];
    stream.read_exact(&mut marker).await.unwrap();
    assert_eq!(&marker, b"y");
    assert_eq!(stack.tcp_event_counts().task_errors, 0);
    assert_eq!(stack.tcp_payload_usage().0, 0);
}

#[tokio::test(start_paused = true)]
async fn default_retries_bound_a_permanently_unresponsive_peer_and_release_payload() {
    let mut tcp = TcpConfig::default();
    tcp.timeout = Duration::from_secs(1800);
    let (stack, mut client, mut stream, _) = connected_with_tcp(tcp).await;
    stream.write_all(b"lost").await.unwrap();
    read_packet(&mut client).await;
    let start = tokio::time::Instant::now();
    timeout(Duration::from_secs(130), async {
        loop {
            let bytes = read_packet(&mut client).await;
            let Some(TransportSlice::Tcp(h)) = SlicedPacket::from_ip(&bytes).unwrap().transport
            else {
                panic!("TCP")
            };
            if h.rst() {
                break;
            }
        }
    })
    .await
    .expect("retry policy retained an unresponsive flow indefinitely");
    assert!(
        start.elapsed() >= Duration::from_secs(100),
        "default policy aborted before the recommended failure threshold"
    );
    assert_eq!(stack.tcp_event_counts().task_errors, 1);
    assert_eq!(stack.tcp_payload_usage().0, 0);
    assert_eq!(stack.tcp_health().0[2], 0);
}

#[tokio::test(start_paused = true)]
async fn acknowledging_the_oldest_segment_preserves_connection_backoff() {
    let (stack, mut client, mut stream, seq) = connected(3).await;
    stream.write_all(b"a").await.unwrap();
    read_packet(&mut client).await;
    stream.write_all(b"b").await.unwrap();
    read_packet(&mut client).await;
    assert_eq!(payload(&read_packet(&mut client).await), b"a");
    client
        .write_all(&packet(1002, Some(seq.wrapping_add(1)), false, b"y"))
        .await
        .unwrap();
    stream.read_exact(&mut [0]).await.unwrap();
    read_packet(&mut client).await;
    assert!(
        timeout(Duration::from_millis(150), read_packet(&mut client))
            .await
            .is_err(),
        "advancing to the next segment collapsed the connection's 200 ms backoff"
    );
    let retry = timeout(Duration::from_millis(60), read_packet(&mut client))
        .await
        .unwrap();
    assert_eq!(payload(&retry), b"b");
    assert_eq!(stack.tcp_event_counts().task_errors, 0);
}

#[tokio::test(start_paused = true)]
async fn fast_retransmission_does_not_spend_timeout_retries_or_double_rto() {
    let (stack, mut client, mut stream, seq) = connected(1).await;
    stream.write_all(b"lost").await.unwrap();
    read_packet(&mut client).await;
    for _ in 0..3 {
        client
            .write_all(&packet(1002, Some(seq), false, &[]))
            .await
            .unwrap();
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
    }
    let fast = timeout(Duration::from_millis(50), read_packet(&mut client))
        .await
        .unwrap();
    assert_eq!(payload(&fast), b"lost");
    let retry = timeout(Duration::from_millis(110), read_packet(&mut client))
        .await
        .expect("fast retransmission incorrectly doubled the RTO");
    assert_eq!(
        payload(&retry),
        b"lost",
        "fast retransmission spent the timeout retry budget"
    );
    assert_eq!(stack.tcp_event_counts().task_errors, 0);
}

#[tokio::test(start_paused = true)]
async fn window_updates_do_not_poison_the_duplicate_ack_run() {
    let (stack, mut client, mut stream, seq) = connected(3).await;
    stream.write_all(b"lost").await.unwrap();
    read_packet(&mut client).await;
    for window in [60000, 59000, 58000, 58000, 58000, 58000] {
        let b = PacketBuilder::ipv4([192, 0, 2, 2], [198, 51, 100, 2], 64)
            .tcp(12345, 443, 1002, window)
            .ack(seq);
        let mut bytes = Vec::new();
        b.write(&mut bytes, &[]).unwrap();
        client.write_all(&bytes).await.unwrap();
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
    }
    let retry = timeout(Duration::from_millis(50), read_packet(&mut client))
        .await
        .expect("window updates prevented fast retransmission after three actual duplicate ACKs");
    assert_eq!(payload(&retry), b"lost");
    assert_eq!(stack.tcp_event_counts().task_errors, 0);
}

#[tokio::test(start_paused = true)]
async fn a_clean_rtt_sample_reduces_backoff_for_later_data() {
    let (stack, mut client, mut stream, seq) = connected(3).await;
    stream.write_all(b"a").await.unwrap();
    read_packet(&mut client).await;
    read_packet(&mut client).await; // RTO: connection backoff is now 200 ms.
    client
        .write_all(&packet(1002, Some(seq.wrapping_add(1)), false, b"y"))
        .await
        .unwrap();
    stream.read_exact(&mut [0]).await.unwrap();
    read_packet(&mut client).await;
    stream.write_all(b"fresh").await.unwrap();
    read_packet(&mut client).await;
    tokio::time::advance(Duration::from_millis(10)).await;
    client
        .write_all(&packet(1003, Some(seq.wrapping_add(6)), false, b"z"))
        .await
        .unwrap();
    stream.read_exact(&mut [0]).await.unwrap();
    read_packet(&mut client).await;
    stream.write_all(b"last").await.unwrap();
    read_packet(&mut client).await;
    let retry = timeout(Duration::from_millis(110), read_packet(&mut client))
        .await
        .expect("a clean RTT sample did not reduce the backed-off timer");
    assert_eq!(payload(&retry), b"last");
    assert_eq!(stack.tcp_event_counts().task_errors, 0);
}

#[tokio::test(start_paused = true)]
async fn an_ack_after_retransmission_is_not_a_clean_rtt_sample() {
    let (_stack, mut client, mut stream, seq) = connected(3).await;
    stream.write_all(b"a").await.unwrap();
    read_packet(&mut client).await;
    read_packet(&mut client).await;
    tokio::time::advance(Duration::from_millis(90)).await;
    client
        .write_all(&packet(1002, Some(seq.wrapping_add(1)), false, b"y"))
        .await
        .unwrap();
    stream.read_exact(&mut [0]).await.unwrap();
    read_packet(&mut client).await;
    stream.write_all(b"b").await.unwrap();
    read_packet(&mut client).await;
    assert!(
        timeout(Duration::from_millis(150), read_packet(&mut client))
            .await
            .is_err()
    );
    let retry = timeout(Duration::from_millis(60), read_packet(&mut client))
        .await
        .expect("an ambiguous ACK changed the RTO measurement");
    assert_eq!(payload(&retry), b"b");
}

fn window_packet(seq: u32, ack: u32, window: u16, data: &[u8]) -> Vec<u8> {
    let b = PacketBuilder::ipv4([192, 0, 2, 2], [198, 51, 100, 2], 64)
        .tcp(12345, 443, seq, window)
        .ack(ack);
    let mut bytes = Vec::new();
    b.write(&mut bytes, data).unwrap();
    bytes
}

#[tokio::test(start_paused = true)]
async fn a_zero_window_blocked_writer_probes_even_with_no_inflight_data() {
    let (stack, mut client, mut stream, seq) = connected(1).await;
    client
        .write_all(&window_packet(1002, seq, 0, b"y"))
        .await
        .unwrap();
    stream.read_exact(&mut [0]).await.unwrap();
    read_packet(&mut client).await;
    let writer = tokio::spawn(async move {
        let result = stream.write_all(b"next").await;
        (stream, result)
    });
    let probe = timeout(Duration::from_millis(150), read_packet(&mut client))
        .await
        .expect("blocked writer never armed a window probe");
    let Some(TransportSlice::Tcp(h)) = SlicedPacket::from_ip(&probe).unwrap().transport else {
        panic!("TCP")
    };
    assert!(h.payload().is_empty() && h.ack() && !h.rst());
    assert_eq!(h.sequence_number(), seq.wrapping_sub(1));
    client
        .write_all(&window_packet(1003, seq, 65535, &[]))
        .await
        .unwrap();
    let data = timeout(Duration::from_millis(20), read_packet(&mut client))
        .await
        .unwrap();
    assert_eq!(payload(&data), b"next");
    let (mut stream, result) = writer.await.unwrap();
    result.unwrap();
    client
        .write_all(&packet(1003, Some(seq.wrapping_add(4)), false, b"z"))
        .await
        .unwrap();
    stream.read_exact(&mut [0]).await.unwrap();
    assert_eq!(stack.tcp_event_counts().task_errors, 0);
    assert_eq!(stack.tcp_payload_usage().0, 0);
}

#[tokio::test(start_paused = true)]
async fn unanswered_window_probes_fail_the_flow_and_release_held_payload() {
    let (stack, mut client, mut stream, seq) = connected(1).await;
    stream.write_all(b"held").await.unwrap();
    read_packet(&mut client).await;
    client
        .write_all(&window_packet(1002, seq, 0, b"y"))
        .await
        .unwrap();
    stream.read_exact(&mut [0]).await.unwrap();
    read_packet(&mut client).await;
    let probe = timeout(Duration::from_millis(150), read_packet(&mut client))
        .await
        .unwrap();
    assert_eq!(payload(&probe), b"h");
    let reset = timeout(Duration::from_millis(250), read_packet(&mut client))
        .await
        .expect("unanswered window probes retained a dead flow");
    let Some(TransportSlice::Tcp(h)) = SlicedPacket::from_ip(&reset).unwrap().transport else {
        panic!("TCP")
    };
    assert!(h.rst());
    assert_eq!(stack.tcp_event_counts().task_errors, 1);
    assert_eq!(stack.tcp_payload_usage().0, 0);
}

#[tokio::test(start_paused = true)]
async fn responsive_closed_window_does_not_exhaust_data_or_probe_retries() {
    let (stack, mut client, mut stream, seq) = connected(1).await;
    stream.write_all(b"held").await.unwrap();
    read_packet(&mut client).await;
    client
        .write_all(&window_packet(1002, seq, 0, b"y"))
        .await
        .unwrap();
    stream.read_exact(&mut [0]).await.unwrap();
    read_packet(&mut client).await;
    for _ in 0..4 {
        let probe = timeout(Duration::from_secs(2), read_packet(&mut client))
            .await
            .unwrap();
        let Some(TransportSlice::Tcp(h)) = SlicedPacket::from_ip(&probe).unwrap().transport else {
            panic!("TCP")
        };
        assert!(
            h.payload() == b"h" && h.ack() && !h.rst(),
            "closed-window ACKs triggered full data retries or an abort"
        );
        client
            .write_all(&window_packet(1003, seq, 0, &[]))
            .await
            .unwrap();
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
    }
    assert_eq!(stack.tcp_event_counts().task_errors, 0);
    client
        .write_all(&window_packet(1003, seq, 65535, &[]))
        .await
        .unwrap();
    let data = timeout(Duration::from_millis(150), read_packet(&mut client))
        .await
        .unwrap();
    assert_eq!(payload(&data), b"held");
    client
        .write_all(&packet(1003, Some(seq.wrapping_add(4)), false, b"z"))
        .await
        .unwrap();
    stream.read_exact(&mut [0]).await.unwrap();
    assert_eq!(stack.tcp_payload_usage().0, 0);
}

#[tokio::test(start_paused = true)]
async fn timeout_retransmission_reuses_full_send_window_credit() {
    let (stack, mut client, mut stream, _) = connected(3).await;
    stream.write_all(&vec![7; 16 * 1024]).await.unwrap();
    let original = read_packet(&mut client).await;
    let first = payload(&original).to_vec();
    let mut received = first.len();
    while received < 16 * 1024 {
        received += payload(&read_packet(&mut client).await).len();
    }
    let retry = timeout(Duration::from_millis(150), read_packet(&mut client))
        .await
        .unwrap();
    assert_eq!(
        payload(&retry),
        first,
        "full send credit turned an RTO retry into an empty segment"
    );
    assert_eq!(stack.tcp_payload_usage().0, 16 * 1024);
}

#[tokio::test(start_paused = true)]
async fn fast_retransmission_reuses_full_send_window_credit() {
    let (stack, mut client, mut stream, seq) = connected(3).await;
    stream.write_all(&vec![7; 16 * 1024]).await.unwrap();
    let original = read_packet(&mut client).await;
    let first = payload(&original).to_vec();
    let mut received = first.len();
    while received < 16 * 1024 {
        received += payload(&read_packet(&mut client).await).len();
    }
    for _ in 0..3 {
        client
            .write_all(&packet(1002, Some(seq), false, &[]))
            .await
            .unwrap();
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
    }
    let retry = timeout(Duration::from_millis(50), read_packet(&mut client))
        .await
        .unwrap();
    assert_eq!(
        payload(&retry),
        first,
        "full send credit turned a fast retry into an empty segment"
    );
    assert_eq!(stack.tcp_payload_usage().0, 16 * 1024);
}

#[tokio::test(start_paused = true)]
async fn partial_retry_progress_survives_a_shrunken_receive_window() {
    let (stack, mut client, mut stream, seq) = connected(1).await;
    stream.write_all(b"abcdefgh").await.unwrap();
    read_packet(&mut client).await;
    client
        .write_all(&window_packet(1002, seq, 2, &[]))
        .await
        .unwrap();
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
    for i in 0..4u32 {
        let retry = timeout(Duration::from_secs(2), read_packet(&mut client))
            .await
            .unwrap();
        assert_eq!(
            payload(&retry),
            &b"abcdefgh"[i as usize * 2..i as usize * 2 + 2]
        );
        client
            .write_all(&window_packet(
                1002 + i,
                seq.wrapping_add((i + 1) * 2),
                2,
                b"y",
            ))
            .await
            .unwrap();
        stream.read_exact(&mut [0]).await.unwrap();
        read_packet(&mut client).await;
    }
    assert_eq!(stack.tcp_event_counts().task_errors, 0);
    assert_eq!(stack.tcp_payload_usage().0, 0);
}

#[tokio::test(start_paused = true)]
async fn duplicate_received_payload_still_advances_the_peer_ack_state() {
    let (stack, mut client, mut stream, seq) = connected(3).await;
    client
        .write_all(&window_packet(1002, seq, 4, b"y"))
        .await
        .unwrap();
    stream.read_exact(&mut [0]).await.unwrap();
    read_packet(&mut client).await;
    stream.write_all(b"data").await.unwrap();
    read_packet(&mut client).await;
    // The upstream byte is a duplicate, but the downstream ACK is new.
    client
        .write_all(&window_packet(1002, seq.wrapping_add(4), 4, b"y"))
        .await
        .unwrap();
    read_packet(&mut client).await;
    timeout(Duration::from_millis(20), stream.write_all(b"next"))
        .await
        .expect("duplicate payload discarded the ACK and left phantom bytes in flight")
        .unwrap();
    let next = read_packet(&mut client).await;
    assert_eq!(payload(&next), b"next");
    client
        .write_all(&window_packet(1003, seq.wrapping_add(8), 4, b"z"))
        .await
        .unwrap();
    stream.read_exact(&mut [0]).await.unwrap();
    assert_eq!(stack.tcp_payload_usage().0, 0);
}

#[tokio::test(start_paused = true)]
async fn a_fin_after_already_delivered_payload_is_not_discarded_as_a_duplicate() {
    let (stack, mut client, mut stream, seq) = connected(3).await;
    let b = PacketBuilder::ipv4([192, 0, 2, 2], [198, 51, 100, 2], 64)
        .tcp(12345, 443, 1001, 65535)
        .ack(seq)
        .fin();
    let mut bytes = Vec::new();
    b.write(&mut bytes, b"x").unwrap();
    client.write_all(&bytes).await.unwrap();
    let read = timeout(Duration::from_millis(100), stream.read(&mut [0]))
        .await
        .expect("the new FIN was discarded with already-delivered data")
        .unwrap();
    assert_eq!(read, 0);
    assert_eq!(stack.tcp_event_counts().task_errors, 0);
    stream.write_all(b"reply").await.unwrap();
}

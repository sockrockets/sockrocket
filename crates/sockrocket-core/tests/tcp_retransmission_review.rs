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
async fn closed_receive_window_does_not_consume_retry_budget() {
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
    assert!(
        timeout(Duration::from_secs(1), read_packet(&mut client))
            .await
            .is_err()
    );
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

//! Memory-packet regressions for send-window credit and invalid ACK handling.
use etherparse::{PacketBuilder, SlicedPacket, TransportSlice};
use ipstack::{IpStack, IpStackConfig, IpStackStream, IpStackTcpStream, TcpConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::time::{Duration, timeout};

fn packet(seq: u32, ack: Option<u32>, syn: bool, window: u16, data: &[u8]) -> Vec<u8> {
    let p = PacketBuilder::ipv4([192, 0, 2, 2], [198, 51, 100, 2], 64).tcp(12345, 443, seq, window);
    let p = if syn { p.syn() } else { p };
    let p = if let Some(ack) = ack { p.ack(ack) } else { p };
    let mut bytes = Vec::new();
    p.write(&mut bytes, data).unwrap();
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

fn tcp(bytes: &[u8]) -> etherparse::TcpSlice<'_> {
    let Some(TransportSlice::Tcp(tcp)) = SlicedPacket::from_ip(bytes).unwrap().transport else {
        panic!("TCP expected")
    };
    tcp
}

async fn read_data_packet(client: &mut DuplexStream) -> Vec<u8> {
    loop {
        let bytes = read_packet(client).await;
        if !tcp(&bytes).payload().is_empty() {
            return bytes;
        }
    }
}

async fn connected(window: u16, cap: u32) -> (IpStack, DuplexStream, IpStackTcpStream, u32) {
    let mut config = TcpConfig::default();
    config.rto = Duration::from_millis(100);
    config.max_unacked_bytes = cap;
    let mut stack_config = IpStackConfig::default();
    stack_config.with_tcp_config(config);
    let (device, mut client) = tokio::io::duplex(65536);
    let mut stack = IpStack::new(stack_config, device);
    client
        .write_all(&packet(1000, None, true, window, &[]))
        .await
        .unwrap();
    let IpStackStream::Tcp(mut stream) = stack.accept().await.unwrap() else {
        panic!("TCP stream expected")
    };
    let synack = read_packet(&mut client).await;
    let seq = tcp(&synack).sequence_number().wrapping_add(1);
    client
        .write_all(&packet(1001, Some(seq), false, window, b"x"))
        .await
        .unwrap();
    stream.read_exact(&mut [0]).await.unwrap();
    read_packet(&mut client).await;
    (stack, client, stream, seq)
}

async fn check_remaining_credit(window: u16, cap: u32) {
    let (_stack, mut client, mut stream, seq) = connected(window, cap).await;
    assert_eq!(stream.write(b"abcd").await.unwrap(), 4);
    assert_eq!(tcp(&read_data_packet(&mut client).await).payload(), b"abcd");
    assert_eq!(stream.write(b"efgh").await.unwrap(), 2);
    let second = read_data_packet(&mut client).await;
    assert_eq!(tcp(&second).sequence_number(), seq.wrapping_add(4));
    assert_eq!(tcp(&second).payload(), b"ef");
    assert!(
        timeout(Duration::from_millis(20), stream.write(b"i"))
            .await
            .is_err()
    );
    client
        .write_all(&packet(
            1002,
            Some(seq.wrapping_add(6)),
            false,
            window,
            b"y",
        ))
        .await
        .unwrap();
    stream.read_exact(&mut [0]).await.unwrap();
    read_packet(&mut client).await;
    assert_eq!(stream.write(b"gh").await.unwrap(), 2);
    assert_eq!(tcp(&read_data_packet(&mut client).await).payload(), b"gh");
}

#[tokio::test(start_paused = true)]
async fn consecutive_writes_use_remaining_peer_window_and_resume_after_ack() {
    check_remaining_credit(6, 16384).await;
}

#[tokio::test(start_paused = true)]
async fn consecutive_writes_use_remaining_per_flow_budget() {
    check_remaining_credit(65535, 6).await;
}

async fn check_future_ack(data: &[u8]) {
    let (stack, mut client, mut stream, seq) = connected(65535, 16384).await;
    stream.write_all(b"PING").await.unwrap();
    read_packet(&mut client).await;
    assert_eq!(stack.tcp_payload_usage().0, 4);
    // Neither this impossible ACK nor its zero window/payload may mutate state.
    client
        .write_all(&packet(1002, Some(seq.wrapping_add(504)), false, 0, data))
        .await
        .unwrap();
    let reply = timeout(Duration::from_millis(20), read_packet(&mut client))
        .await
        .unwrap();
    assert!(tcp(&reply).ack());
    assert_eq!(tcp(&reply).acknowledgment_number(), 1002);
    assert_eq!(tcp(&reply).sequence_number(), seq.wrapping_add(4));
    assert!(tcp(&reply).payload().is_empty());
    assert_eq!(
        stack.tcp_payload_usage().0,
        4,
        "invalid ACK freed payload credit"
    );
    let retry = timeout(Duration::from_millis(150), read_packet(&mut client))
        .await
        .expect("invalid ACK erased or paused retransmission");
    assert_eq!(tcp(&retry).sequence_number(), seq);
    assert_eq!(tcp(&retry).payload(), b"PING");
    client
        .write_all(&packet(1002, Some(seq.wrapping_add(4)), false, 65535, b"y"))
        .await
        .unwrap();
    let mut marker = [0];
    stream.read_exact(&mut marker).await.unwrap();
    assert_eq!(&marker, b"y", "payload from invalid ACK was delivered");
    read_packet(&mut client).await;
    assert_eq!(stack.tcp_payload_usage().0, 0);
    assert_eq!(stream.write(b"next").await.unwrap(), 4);
    assert_eq!(tcp(&read_packet(&mut client).await).payload(), b"next");
}

#[tokio::test(start_paused = true)]
async fn future_ack_preserves_payload_credit_window_and_retransmission() {
    check_future_ack(&[]).await;
}

#[tokio::test(start_paused = true)]
async fn future_ack_with_payload_is_rejected_before_delivery() {
    check_future_ack(b"invalid").await;
}

#[tokio::test(start_paused = true)]
async fn segment_without_ack_cannot_free_or_deliver_payload() {
    let (stack, mut client, mut stream, seq) = connected(65535, 16384).await;
    stream.write_all(b"PING").await.unwrap();
    read_packet(&mut client).await;
    client
        .write_all(&packet(1002, None, false, 0, b"invalid"))
        .await
        .unwrap();
    let retry = timeout(Duration::from_millis(150), read_packet(&mut client))
        .await
        .unwrap();
    assert_eq!(tcp(&retry).payload(), b"PING");
    assert_eq!(stack.tcp_payload_usage().0, 4);
    client
        .write_all(&packet(1002, Some(seq.wrapping_add(4)), false, 65535, b"y"))
        .await
        .unwrap();
    let mut marker = [0];
    stream.read_exact(&mut marker).await.unwrap();
    assert_eq!(&marker, b"y");
    assert_eq!(stack.tcp_event_counts().task_errors, 0);
}

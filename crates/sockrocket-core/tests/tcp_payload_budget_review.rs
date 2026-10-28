//! Shared payload pressure using real TCP state machines and memory packets.
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

async fn connected(
    pool: std::sync::Arc<ipstack::TcpPayloadBudget>,
) -> (IpStack, DuplexStream, IpStackTcpStream, u32) {
    let mut tcp = TcpConfig::default();
    tcp.rto = Duration::from_secs(100);
    tcp.payload_budget = pool;
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
async fn acknowledgement_on_one_flow_wakes_another_budget_blocked_writer() {
    let pool = ipstack::TcpPayloadBudget::new(8);
    let (_first_stack, mut first_client, mut first, seq) = connected(pool.clone()).await;
    let (_second_stack, mut second_client, mut second, _) = connected(pool.clone()).await;
    first.write_all(b"12345678").await.unwrap();
    assert_eq!(payload(&read_packet(&mut first_client).await), b"12345678");
    assert_eq!(pool.usage().0, 8);
    let writer = tokio::spawn(async move {
        second.write_all(b"x").await.unwrap();
        second
    });
    tokio::task::yield_now().await;
    assert!(!writer.is_finished(), "shared limit was ignored");
    first_client
        .write_all(&packet(1002, Some(seq.wrapping_add(8)), false, &[]))
        .await
        .unwrap();
    let second = timeout(Duration::from_millis(20), writer)
        .await
        .expect("ACK failed to wake another flow")
        .unwrap();
    assert_eq!(payload(&read_packet(&mut second_client).await), b"x");
    assert_eq!(pool.usage().0, 1);
    drop(first);
    drop(second);
    timeout(Duration::from_millis(20), async {
        while pool.usage().0 != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(start_paused = true)]
async fn receive_overflow_drops_before_ack_and_retransmission_succeeds_after_consumption() {
    let pool = ipstack::TcpPayloadBudget::new(8);
    let (_first_stack, mut first_client, mut first, first_seq) = connected(pool.clone()).await;
    let (_second_stack, mut second_client, mut second, second_seq) = connected(pool.clone()).await;
    first_client
        .write_all(&packet(1002, Some(first_seq), false, b"123456"))
        .await
        .unwrap();
    read_packet(&mut first_client).await;
    assert_eq!(pool.usage().0, 6);
    second_client
        .write_all(&packet(1002, Some(second_seq), false, b"abcd"))
        .await
        .unwrap();
    let mut data = [0; 4];
    assert!(
        timeout(Duration::from_millis(10), second.read_exact(&mut data))
            .await
            .is_err()
    );
    let mut first_data = [0; 6];
    first.read_exact(&mut first_data).await.unwrap();
    assert_eq!(&first_data, b"123456");
    second_client
        .write_all(&packet(1002, Some(second_seq), false, b"abcd"))
        .await
        .unwrap();
    timeout(Duration::from_millis(20), second.read_exact(&mut data))
        .await
        .expect("rejected payload was incorrectly acknowledged")
        .unwrap();
    assert_eq!(&data, b"abcd");
    assert_eq!(pool.usage().0, 0);
}

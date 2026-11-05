//! Respect the LAN TCP peer's advertised receive segment size independently
//! of the TUN MTU. All packets stay in memory; no host network changes.
use etherparse::{PacketBuilder, SlicedPacket, TcpOptionElement, TransportSlice};
use ipstack::{IpStack, IpStackConfig, IpStackStream, IpStackTcpStream, TcpConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::time::{Duration, timeout};

fn packet(ipv6: bool, seq: u32, ack: Option<u32>, syn: bool, mss: &[u16], data: &[u8]) -> Vec<u8> {
    let ip = if ipv6 {
        PacketBuilder::ipv6(
            "2001:db8::1"
                .parse::<std::net::Ipv6Addr>()
                .unwrap()
                .octets(),
            "2001:db8::2"
                .parse::<std::net::Ipv6Addr>()
                .unwrap()
                .octets(),
            64,
        )
    } else {
        PacketBuilder::ipv4([192, 0, 2, 1], [198, 51, 100, 1], 64)
    };
    let tcp = ip.tcp(12345, 443, seq, 65535);
    let tcp = if syn { tcp.syn() } else { tcp };
    let tcp = if let Some(ack) = ack {
        tcp.ack(ack)
    } else {
        tcp
    };
    let tcp = if mss.is_empty() {
        tcp
    } else {
        tcp.options(
            &mss.iter()
                .map(|m| TcpOptionElement::MaximumSegmentSize(*m))
                .collect::<Vec<_>>(),
        )
        .unwrap()
    };
    let mut bytes = Vec::new();
    tcp.write(&mut bytes, data).unwrap();
    bytes
}

async fn read_packet(peer: &mut DuplexStream) -> Vec<u8> {
    let mut first = [0; 6];
    peer.read_exact(&mut first).await.unwrap();
    let len = if first[0] >> 4 == 4 {
        u16::from_be_bytes([first[2], first[3]]) as usize
    } else {
        40 + u16::from_be_bytes([first[4], first[5]]) as usize
    };
    let mut bytes = first.to_vec();
    bytes.resize(len, 0);
    peer.read_exact(&mut bytes[6..]).await.unwrap();
    bytes
}

fn tcp(bytes: &[u8]) -> etherparse::TcpSlice<'_> {
    let Some(TransportSlice::Tcp(tcp)) = SlicedPacket::from_ip(bytes).unwrap().transport else {
        panic!("TCP expected")
    };
    tcp
}

async fn connected(ipv6: bool, mss: &[u16]) -> (IpStack, DuplexStream, IpStackTcpStream, u32) {
    let mut config = IpStackConfig::default();
    config.mtu(1500).unwrap();
    let mut settings = TcpConfig::default();
    settings.rto = Duration::from_millis(100);
    config.with_tcp_config(settings);
    let (device, mut peer) = tokio::io::duplex(65536);
    let mut stack = IpStack::new(config, device);
    peer.write_all(&packet(ipv6, 1000, None, true, mss, &[]))
        .await
        .unwrap();
    let IpStackStream::Tcp(mut stream) = stack.accept().await.unwrap() else {
        panic!("TCP expected")
    };
    let response = read_packet(&mut peer).await;
    let seq = tcp(&response).sequence_number().wrapping_add(1);
    peer.write_all(&packet(ipv6, 1001, Some(seq), false, &[], b"x"))
        .await
        .unwrap();
    stream.read_exact(&mut [0]).await.unwrap();
    read_packet(&mut peer).await;
    (stack, peer, stream, seq)
}

#[tokio::test(start_paused = true)]
async fn peer_mss_limits_first_write_and_retransmission_for_both_families() {
    for ipv6 in [false, true] {
        let (stack, mut peer, mut stream, seq) = connected(ipv6, &[900]).await;
        let accepted = stream.write(&[7; 3000]).await.unwrap();
        assert_eq!(accepted, 3000, "batch should accept multiple MSS segments");
        let first = read_packet(&mut peer).await;
        assert_eq!(tcp(&first).payload(), &[7; 900]);
        for expected in [900, 900, 300] {
            let segment = read_packet(&mut peer).await;
            assert_eq!(tcp(&segment).payload().len(), expected);
        }
        let retry = timeout(Duration::from_millis(150), read_packet(&mut peer))
            .await
            .unwrap();
        assert_eq!(tcp(&retry).payload(), &[7; 900]);
        assert_eq!(tcp(&retry).sequence_number(), seq);
        assert_eq!(stack.tcp_payload_usage().0, 3000);
    }
}

#[tokio::test(start_paused = true)]
async fn absent_mss_uses_ipv4_and_ipv6_protocol_defaults() {
    for (ipv6, expected) in [(false, 536), (true, 1220)] {
        let (_stack, mut peer, mut stream, _) = connected(ipv6, &[]).await;
        assert_eq!(stream.write(&[7; 3000]).await.unwrap(), 3000);
        let mut received = 0;
        while received < 3000 {
            let bytes = read_packet(&mut peer).await;
            let len = tcp(&bytes).payload().len();
            assert_eq!(len, expected.min(3000 - received));
            received += len;
        }
    }
}

#[tokio::test(start_paused = true)]
async fn mtu_still_caps_large_mss_and_duplicate_options_choose_the_smaller_limit() {
    for (ipv6, offered, expected) in [
        (false, vec![2000], 1460),
        (true, vec![2000], 1440),
        (false, vec![1200, 400], 400),
        (true, vec![1200, 400], 400),
        (false, vec![0], 536),
    ] {
        let (_stack, mut peer, mut stream, _) = connected(ipv6, &offered).await;
        assert_eq!(stream.write(&[7; 3000]).await.unwrap(), 3000);
        let mut received = 0;
        while received < 3000 {
            let bytes = read_packet(&mut peer).await;
            let len = tcp(&bytes).payload().len();
            assert_eq!(len, expected.min(3000 - received));
            received += len;
        }
    }
}

#[tokio::test(start_paused = true)]
async fn later_mss_options_do_not_raise_the_limit_and_segmented_bytes_remain_lossless() {
    let (stack, mut peer, mut stream, seq) = connected(false, &[900]).await;
    peer.write_all(&packet(false, 1002, Some(seq), false, &[2000], b"y"))
        .await
        .unwrap();
    stream.read_exact(&mut [0]).await.unwrap();
    read_packet(&mut peer).await;
    let expected: Vec<_> = (0..5000).map(|i| (i % 251) as u8).collect();
    stream.write_all(&expected).await.unwrap();
    let mut received = Vec::new();
    while received.len() < expected.len() {
        let bytes = read_packet(&mut peer).await;
        let segment = tcp(&bytes);
        assert!(segment.payload().len() <= 900);
        assert_eq!(
            segment.sequence_number(),
            seq.wrapping_add(received.len() as u32)
        );
        received.extend_from_slice(segment.payload());
    }
    assert_eq!(received, expected);
    peer.write_all(&packet(
        false,
        1003,
        Some(seq.wrapping_add(5000)),
        false,
        &[],
        &[],
    ))
    .await
    .unwrap();
    for _ in 0..200 {
        if stack.tcp_payload_usage().0 == 0 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(stack.tcp_payload_usage().0, 0);
}

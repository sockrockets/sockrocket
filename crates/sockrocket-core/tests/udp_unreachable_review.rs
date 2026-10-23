use std::{
    net::Ipv4Addr,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::mpsc,
    time::{Duration, timeout},
};

struct Device {
    input: mpsc::Receiver<Vec<u8>>,
    output: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl AsyncRead for Device {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.input.poll_recv(cx) {
            Poll::Ready(Some(packet)) => {
                buf.put_slice(&packet);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(None) => Poll::Ready(Ok(())),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncWrite for Device {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.output.lock().unwrap().push(buf.to_vec());
        Poll::Ready(Ok(buf.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

fn udp_packet() -> Vec<u8> {
    let builder = etherparse::PacketBuilder::ipv4(
        Ipv4Addr::new(192, 0, 2, 10).octets(),
        Ipv4Addr::new(198, 18, 0, 2).octets(),
        64,
    )
    .udp(40000, 443);
    let mut packet = Vec::new();
    builder.write(&mut packet, b"quic").unwrap();
    packet
}

async fn accept_udp(
    packet: Vec<u8>,
) -> (
    ipstack::IpStackUdpStream,
    Arc<Mutex<Vec<Vec<u8>>>>,
    ipstack::IpStack,
    mpsc::Sender<Vec<u8>>,
) {
    let (tx, rx) = mpsc::channel(4);
    let output = Arc::new(Mutex::new(Vec::new()));
    let mut stack = ipstack::IpStack::new(
        Default::default(),
        Device {
            input: rx,
            output: output.clone(),
        },
    );
    tx.send(packet).await.unwrap();
    let ipstack::IpStackStream::Udp(udp) = timeout(Duration::from_secs(2), stack.accept())
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("expected a UDP flow");
    };
    (udp, output, stack, tx)
}

async fn first_output(output: &Arc<Mutex<Vec<Vec<u8>>>>) -> Vec<u8> {
    timeout(Duration::from_secs(2), async {
        loop {
            if let Some(packet) = output.lock().unwrap().first().cloned() {
                return packet;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn ipv6_port_unreachable_has_a_valid_checksum() {
    let client: std::net::Ipv6Addr = "2001:db8::10".parse().unwrap();
    let target: std::net::Ipv6Addr = "2001:2:0:4765::c612:2".parse().unwrap();
    let mut packet = Vec::new();
    etherparse::PacketBuilder::ipv6(client.octets(), target.octets(), 64)
        .udp(40000, 443)
        .write(&mut packet, b"quic")
        .unwrap();
    let (udp, output, _stack, _tx) = accept_udp(packet).await;
    udp.reject_port_unreachable().unwrap();
    let reply = first_output(&output).await;
    let parsed = etherparse::SlicedPacket::from_ip(&reply).unwrap();
    let etherparse::NetSlice::Ipv6(ip) = parsed.net.unwrap() else {
        panic!("expected IPv6");
    };
    assert_eq!(ip.header().source(), target.octets());
    assert_eq!(ip.header().destination(), client.octets());
    let etherparse::TransportSlice::Icmpv6(icmp) = parsed.transport.unwrap() else {
        panic!("expected ICMPv6");
    };
    let expected = icmp
        .header()
        .icmp_type
        .calc_checksum(target.octets(), client.octets(), icmp.payload())
        .unwrap();
    assert_eq!(icmp.checksum(), expected);
}

#[tokio::test]
async fn multicast_flows_get_no_icmp_error() {
    let mut packet = Vec::new();
    etherparse::PacketBuilder::ipv4([192, 0, 2, 10], [239, 255, 255, 250], 1)
        .udp(40000, 1900)
        .write(&mut packet, b"ssdp")
        .unwrap();
    let (udp, output, _stack, _tx) = accept_udp(packet).await;
    assert!(udp.reject_port_unreachable().is_err());
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(output.lock().unwrap().is_empty());
}

#[tokio::test]
async fn unsupported_udp_replies_with_port_unreachable() {
    let (tx, rx) = mpsc::channel(4);
    let output = Arc::new(Mutex::new(Vec::new()));
    let mut stack = ipstack::IpStack::new(
        Default::default(),
        Device {
            input: rx,
            output: output.clone(),
        },
    );
    tx.send(udp_packet()).await.unwrap();
    let ipstack::IpStackStream::Udp(udp) = timeout(Duration::from_secs(2), stack.accept())
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("expected a UDP flow");
    };
    udp.reject_port_unreachable().unwrap();
    let reply = timeout(Duration::from_secs(2), async {
        loop {
            if let Some(packet) = output.lock().unwrap().first().cloned() {
                return packet;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let parsed = etherparse::SlicedPacket::from_ip(&reply).unwrap();
    let etherparse::NetSlice::Ipv4(ip) = parsed.net.unwrap() else {
        panic!("expected IPv4");
    };
    assert_eq!(ip.header().source(), Ipv4Addr::new(198, 18, 0, 2).octets());
    assert_eq!(
        ip.header().destination(),
        Ipv4Addr::new(192, 0, 2, 10).octets()
    );
    let etherparse::TransportSlice::Icmpv4(icmp) = parsed.transport.unwrap() else {
        panic!("expected ICMPv4");
    };
    assert!(matches!(
        icmp.icmp_type(),
        etherparse::Icmpv4Type::DestinationUnreachable(
            etherparse::icmpv4::DestUnreachableHeader::Port
        )
    ));
}

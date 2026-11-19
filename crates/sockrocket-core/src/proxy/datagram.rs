//! Datagram transport preserves message boundaries; it is never a TCP stream.
use anyhow::Result;
use std::{
    future::{Future, poll_fn},
    pin::Pin,
    sync::{Mutex, atomic::Ordering},
    time::Duration,
};
#[cfg(test)]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;

pub type DatagramFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;
pub trait Datagram: Send + Sync {
    fn send<'a>(&'a self, packet: &'a [u8]) -> DatagramFuture<'a, ()>;
    fn recv<'a>(&'a self, packet: &'a mut [u8]) -> DatagramFuture<'a, usize>;
}
pub type BoxDatagram = Box<dyn Datagram>;

/// The carrier discarded this individual message before changing association state.
#[derive(Debug)]
pub(super) struct PacketRejected(pub &'static str);
impl std::fmt::Display for PacketRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for PacketRejected {}

pub(super) fn copy_received(data: &[u8], buffer: &mut [u8]) -> Result<usize> {
    if data.len() > buffer.len() {
        return Err(PacketRejected("UDP receive buffer too small").into());
    }
    buffer[..data.len()].copy_from_slice(data);
    Ok(data.len())
}

fn native_packet_error(error: std::io::Error) -> anyhow::Error {
    // Windows reports WSAEMSGSIZE on truncated receives. Native sends may
    // also report the path MTU. Neither event closes a connected UDP socket.
    #[cfg(unix)]
    const MESSAGE_TOO_LARGE: i32 = libc::EMSGSIZE;
    #[cfg(windows)]
    const MESSAGE_TOO_LARGE: i32 = 10040;
    #[cfg(any(unix, windows))]
    if error.raw_os_error() == Some(MESSAGE_TOO_LARGE) {
        return PacketRejected("UDP datagram exceeds the socket payload limit").into();
    }
    error.into()
}

impl Datagram for UdpSocket {
    fn send<'a>(&'a self, packet: &'a [u8]) -> DatagramFuture<'a, ()> {
        Box::pin(async move {
            let sent = UdpSocket::send(self, packet)
                .await
                .map_err(native_packet_error)?;
            anyhow::ensure!(sent == packet.len(), "Partial UDP datagram write");
            Ok(())
        })
    }
    fn recv<'a>(&'a self, packet: &'a mut [u8]) -> DatagramFuture<'a, usize> {
        Box::pin(async move {
            UdpSocket::recv(self, packet)
                .await
                .map_err(native_packet_error)
        })
    }
}

pub(super) async fn connect_direct(host: &str, port: u16) -> Result<BoxDatagram> {
    // A device direct rule may restore a Fake-IP to its domain. On Merlin
    // the OS resolver can return that token again; use the same direct DNS
    // policy as TCP and never send UDP back into the fake-address pool.
    let ips = if let Some(result) = crate::dns::scoped_direct_resolve(host).await {
        Some(result?)
    } else {
        super::transport::resolve_server_ips(host).await
    };
    let resolved = if let Some(ips) = ips {
        ips.into_iter()
            .map(|ip| std::net::SocketAddr::new(ip, port))
            .collect::<Vec<_>>()
    } else {
        tokio::net::lookup_host((host, port)).await?.collect()
    };
    connect_direct_candidates(resolved, |addr| async move {
        let socket = UdpSocket::bind(if addr.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        })
        .await?;
        super::outbound_bind::apply_to_udp_socket(&socket, addr.is_ipv6())?;
        socket.connect(addr).await?;
        Ok(Box::new(socket) as BoxDatagram)
    })
    .await
}

async fn connect_direct_candidates<T, F: Future<Output = Result<T>>>(
    resolved: Vec<std::net::SocketAddr>,
    connect: impl Fn(std::net::SocketAddr) -> F,
) -> Result<T> {
    let mut last_error = None;
    for addr in resolved {
        if crate::dns::fakeip::fake_ipv4(addr.ip()).is_some() {
            continue;
        }
        // No application datagram has been sent yet. A local bind or route
        // error can safely advance to the next address without replaying data.
        match connect(addr).await {
            Ok(socket) => return Ok(socket),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        anyhow::anyhow!("No real UDP destination address (Fake-IP cannot be dialed directly)")
    }))
}

pub(super) async fn relay_tun(
    client: &mut ipstack::IpStackUdpStream,
    connection: &super::ConnectionGuard,
    remote: &dyn Datagram,
) -> Result<()> {
    let limit = client.max_payload_len();
    // Both directions only borrow the stream for one nonblocking poll. A
    // pending send never owns the stream while waiting for queue capacity.
    let local = Mutex::new(connection.observe(client));
    relay_packets(&local, limit, remote).await
}

trait LocalDatagram: Sync {
    fn receive(&self) -> impl Future<Output = Result<Option<Vec<u8>>>> + Send;
    fn send(&self, packet: &[u8]) -> impl Future<Output = Result<()>> + Send;
}

impl LocalDatagram for Mutex<super::ObservedIo<&mut ipstack::IpStackUdpStream>> {
    async fn receive(&self) -> Result<Option<Vec<u8>>> {
        Ok(poll_fn(|cx| {
            let mut io = self.lock().unwrap();
            io.stream.poll_recv_datagram(cx).map_ok(|packet| {
                if let Some(ref p) = packet {
                    io.counts.0.fetch_add(p.len() as u64, Ordering::Relaxed);
                    io.record_bytes(p.len() as u64, 0);
                }
                packet
            })
        })
        .await?)
    }
    async fn send(&self, packet: &[u8]) -> Result<()> {
        Ok(poll_fn(|cx| {
            let mut io = self.lock().unwrap();
            io.stream.poll_send_datagram(cx, packet).map_ok(|()| {
                io.counts
                    .1
                    .fetch_add(packet.len() as u64, Ordering::Relaxed);
                io.record_bytes(0, packet.len() as u64);
            })
        })
        .await?)
    }
}

// The duplex harness drives stalled reads/writes independently in timer tests.
#[cfg(test)]
pub(super) async fn relay(
    client: &mut tokio::io::DuplexStream,
    remote: &dyn Datagram,
) -> Result<()> {
    struct Duplex<'a> {
        reader: tokio::sync::Mutex<tokio::io::ReadHalf<&'a mut tokio::io::DuplexStream>>,
        writer: tokio::sync::Mutex<tokio::io::WriteHalf<&'a mut tokio::io::DuplexStream>>,
    }
    impl LocalDatagram for Duplex<'_> {
        async fn receive(&self) -> Result<Option<Vec<u8>>> {
            let mut bytes = vec![0; 4096];
            let n = self.reader.lock().await.read(&mut bytes).await?;
            bytes.truncate(n);
            Ok((n != 0).then_some(bytes))
        }
        async fn send(&self, packet: &[u8]) -> Result<()> {
            Ok(self.writer.lock().await.write_all(packet).await?)
        }
    }
    let (reader, writer) = tokio::io::split(client);
    relay_packets(
        &Duplex {
            reader: reader.into(),
            writer: writer.into(),
        },
        4096,
        remote,
    )
    .await
}

async fn relay_packets(
    local: &impl LocalDatagram,
    limit: usize,
    remote: &dyn Datagram,
) -> Result<()> {
    const IDLE: Duration = Duration::from_secs(30);
    // A blocked/rate-limited upload must not stop draining the bounded QUIC
    // receive queue. Keep each receive future alive until its direction exits.
    #[derive(Clone, Copy)]
    struct Progress {
        last: tokio::time::Instant,
        writing: usize,
    }
    let (progress, mut changed) = tokio::sync::watch::channel(Progress {
        last: tokio::time::Instant::now(),
        writing: 0,
    });
    let upload = async {
        loop {
            let Some(packet) = local.receive().await? else {
                return Ok::<_, anyhow::Error>(());
            };
            progress.send_modify(|state| {
                state.last = tokio::time::Instant::now();
                state.writing += 1;
            });
            let result = tokio::time::timeout(IDLE, remote.send(&packet))
                .await
                .map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::TimedOut, "UDP upstream write stalled")
                })?;
            if let Err(error) = result
                && !error.is::<PacketRejected>()
            {
                return Err(error);
            }
            progress.send_modify(|state| {
                state.last = tokio::time::Instant::now();
                state.writing -= 1;
            });
            tokio::task::consume_budget().await;
        }
    };
    let download = async {
        // A native UDP socket can truncate to buffer length. One sentinel
        // byte distinguishes an exact fit from every oversized message.
        let mut inbound = vec![0; limit + 1];
        loop {
            let size = match remote.recv(&mut inbound).await {
                Ok(size) if size <= limit => size,
                Ok(_) => {
                    tokio::task::consume_budget().await;
                    continue;
                }
                Err(error) if error.is::<PacketRejected>() => {
                    tokio::task::consume_budget().await;
                    continue;
                }
                Err(error) => return Err::<(), _>(error),
            };
            progress.send_modify(|state| {
                state.last = tokio::time::Instant::now();
                state.writing += 1;
            });
            tokio::time::timeout(IDLE, local.send(&inbound[..size]))
                .await
                .map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::TimedOut, "UDP client write stalled")
                })??;
            progress.send_modify(|state| {
                state.last = tokio::time::Instant::now();
                state.writing -= 1;
            });
            tokio::task::consume_budget().await;
        }
    };
    let idle = async {
        loop {
            let Progress { last, writing } = *changed.borrow_and_update();
            if writing == 0 {
                let deadline = last + IDLE;
                if tokio::time::Instant::now() >= deadline {
                    return Ok(());
                }
                tokio::select! {
                    _ = tokio::time::sleep_until(deadline) => {},
                    _ = changed.changed() => {},
                }
            } else {
                // In-flight writes have their own hard deadline. Let that
                // error win instead of misreporting a stalled write as idle.
                let _ = changed.changed().await;
            }
        }
    };
    tokio::select! {
        result = upload => result,
        result = download => result,
        result = idle => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ControlledDatagram {
        send_started: tokio::sync::Notify,
        release_send: tokio::sync::Notify,
        received: tokio::sync::Mutex<tokio::sync::mpsc::Receiver<Vec<u8>>>,
        sent: tokio::sync::mpsc::Sender<Vec<u8>>,
        stall_send: bool,
        receive_started: tokio::sync::Notify,
    }
    impl Datagram for ControlledDatagram {
        fn send<'a>(&'a self, packet: &'a [u8]) -> DatagramFuture<'a, ()> {
            Box::pin(async move {
                self.send_started.notify_one();
                if self.stall_send {
                    self.release_send.notified().await;
                }
                self.sent.send(packet.to_vec()).await?;
                Ok(())
            })
        }
        fn recv<'a>(&'a self, packet: &'a mut [u8]) -> DatagramFuture<'a, usize> {
            Box::pin(async move {
                let data = self.received.lock().await.recv().await.unwrap();
                packet[..data.len()].copy_from_slice(&data);
                self.receive_started.notify_one();
                Ok(data.len())
            })
        }
    }
    fn controlled_datagram(
        stall_send: bool,
    ) -> (
        std::sync::Arc<ControlledDatagram>,
        tokio::sync::mpsc::Sender<Vec<u8>>,
        tokio::sync::mpsc::Receiver<Vec<u8>>,
    ) {
        let (incoming, received) = tokio::sync::mpsc::channel(128);
        let (sent, outgoing) = tokio::sync::mpsc::channel(128);
        (
            std::sync::Arc::new(ControlledDatagram {
                send_started: tokio::sync::Notify::new(),
                release_send: tokio::sync::Notify::new(),
                received: tokio::sync::Mutex::new(received),
                sent,
                stall_send,
                receive_started: tokio::sync::Notify::new(),
            }),
            incoming,
            outgoing,
        )
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_udp_upload_does_not_block_download() {
        let (mut local, mut peer) = tokio::io::duplex(4096);
        let (remote, incoming, _outgoing) = controlled_datagram(true);
        let task = tokio::spawn({
            let remote = remote.clone();
            async move { relay(&mut local, remote.as_ref()).await }
        });
        peer.write_all(b"upload").await.unwrap();
        remote.send_started.notified().await;
        // A receive burst must drain even while the upload is rate-limited.
        for _ in 0..96 {
            incoming.send(vec![42; 32]).await.unwrap();
        }
        let mut received = [0; 96 * 32];
        let result =
            tokio::time::timeout(Duration::from_millis(10), peer.read_exact(&mut received)).await;
        task.abort();
        let _ = task.await;
        result
            .expect("blocked upload stopped UDP receive queue draining")
            .unwrap();
        assert_eq!(received, [42; 96 * 32]);
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_udp_download_does_not_block_upload() {
        let (mut local, mut peer) = tokio::io::duplex(1);
        let (remote, incoming, mut outgoing) = controlled_datagram(false);
        let task = tokio::spawn({
            let remote = remote.clone();
            async move { relay(&mut local, remote.as_ref()).await }
        });
        incoming.send(b"download".to_vec()).await.unwrap();
        remote.receive_started.notified().await;
        tokio::task::yield_now().await;
        // Do not consume the download; the client write remains blocked.
        peer.write_all(b"x").await.unwrap();
        let result = tokio::time::timeout(Duration::from_millis(10), outgoing.recv()).await;
        task.abort();
        let _ = task.await;
        assert_eq!(
            result.expect("blocked download stopped UDP upload"),
            Some(b"x".to_vec())
        );
    }

    #[tokio::test(start_paused = true)]
    async fn udp_activity_in_either_direction_preserves_relay_until_shared_idle_deadline() {
        for download_only in [true, false] {
            let (mut local, mut peer) = tokio::io::duplex(64);
            let (remote, incoming, mut outgoing) = controlled_datagram(false);
            let task = tokio::spawn(async move { relay(&mut local, remote.as_ref()).await });
            for _ in 0..5 {
                tokio::time::advance(Duration::from_secs(10)).await;
                if download_only {
                    incoming.send(b"x".to_vec()).await.unwrap();
                    let mut packet = [0];
                    peer.read_exact(&mut packet).await.unwrap();
                    assert_eq!(packet, *b"x");
                } else {
                    peer.write_all(b"x").await.unwrap();
                    assert_eq!(outgoing.recv().await, Some(b"x".to_vec()));
                }
                assert!(!task.is_finished(), "one-way traffic is not idle");
            }
            // Both directions are now quiet; retaining the channel senders
            // ensures EOF cannot masquerade as idle expiration.
            tokio::time::advance(Duration::from_secs(29)).await;
            tokio::task::yield_now().await;
            assert!(!task.is_finished());
            tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .expect("idle relay retained its session")
                .unwrap()
                .unwrap();
        }
    }

    #[tokio::test(start_paused = true)]
    async fn udp_download_progress_does_not_extend_stalled_upload_deadline() {
        let (mut local, mut peer) = tokio::io::duplex(64);
        let (remote, incoming, _outgoing) = controlled_datagram(true);
        let task = tokio::spawn({
            let remote = remote.clone();
            async move { relay(&mut local, remote.as_ref()).await }
        });
        peer.write_all(b"upload").await.unwrap();
        remote.send_started.notified().await;
        for _ in 0..5 {
            tokio::time::advance(Duration::from_secs(5)).await;
            incoming.send(b"x".to_vec()).await.unwrap();
            let mut packet = [0];
            peer.read_exact(&mut packet).await.unwrap();
        }
        let error = tokio::time::timeout(Duration::from_secs(6), task)
            .await
            .expect("download traffic renewed a stalled upload")
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("UDP upstream write stalled"));
    }
    struct StalledDatagram {
        receive: bool,
    }
    impl Datagram for StalledDatagram {
        fn send<'a>(&'a self, _: &'a [u8]) -> DatagramFuture<'a, ()> {
            Box::pin(std::future::pending())
        }
        fn recv<'a>(&'a self, packet: &'a mut [u8]) -> DatagramFuture<'a, usize> {
            Box::pin(async move {
                if self.receive {
                    packet[0] = 42;
                    Ok(1)
                } else {
                    std::future::pending().await
                }
            })
        }
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_upstream_send_cannot_retain_a_udp_relay_forever() {
        let (mut local, mut peer) = tokio::io::duplex(16);
        peer.write_all(b"packet").await.unwrap();
        let remote = StalledDatagram { receive: false };
        let error = tokio::time::timeout(Duration::from_secs(31), relay(&mut local, &remote))
            .await
            .expect("UDP send bypassed the relay deadline")
            .unwrap_err();
        assert!(error.to_string().contains("UDP upstream write stalled"));
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_client_write_cannot_retain_a_udp_relay_forever() {
        let (mut local, _peer) = tokio::io::duplex(1);
        let remote = StalledDatagram { receive: true };
        let error = tokio::time::timeout(Duration::from_secs(31), relay(&mut local, &remote))
            .await
            .expect("UDP client write bypassed the relay deadline")
            .unwrap_err();
        assert!(error.to_string().contains("UDP client write stalled"));
    }

    #[tokio::test]
    async fn tun_udp_preserves_empty_packets_and_survives_oversize_replies() {
        use crate::proxy::stream_datagram::{Framing, StreamDatagram};

        tokio::time::timeout(Duration::from_secs(5), async {
            for v6 in [false, true] {
                let mtu = 1400;
                let ip_header = if v6 { 40 } else { 20 };
                let limit = usize::from(mtu) - ip_header - 8;
                let packet = |payload: &[u8]| {
                    let builder = if v6 {
                        etherparse::PacketBuilder::ipv6([1; 16], [2; 16], 64)
                    } else {
                        etherparse::PacketBuilder::ipv4([192, 0, 2, 1], [192, 0, 2, 2], 64)
                    };
                    let mut wire = Vec::new();
                    builder.udp(40000, 443).write(&mut wire, payload).unwrap();
                    wire
                };
                let mut cfg = ipstack::IpStackConfig::default();
                cfg.mtu(mtu).unwrap();
                let (mut device, tun) = tokio::io::duplex(32 * 1024);
                let mut stack = ipstack::IpStack::new(cfg, tun);
                device.write_all(&packet(&[])).await.unwrap();
                let ipstack::IpStackStream::Udp(mut udp) = stack.accept().await.unwrap() else {
                    panic!("expected TUN UDP flow");
                };
                let (wire, mut peer) = tokio::io::duplex(32 * 1024);
                let remote = StreamDatagram::boxed(Box::new(wire), Framing::Length16);
                let stats = super::super::ProxyStats::default();
                let guard = stats.track_active("test".into(), "TUN UDP");
                let task =
                    tokio::spawn(async move { relay_tun(&mut udp, &guard, remote.as_ref()).await });

                // The first and subsequent zero-length datagrams must reach the carrier.
                assert_eq!(peer.read_u16().await.unwrap(), 0);
                for payload in [&b""[..], &b"up"[..]] {
                    device.write_all(&packet(payload)).await.unwrap();
                    assert_eq!(usize::from(peer.read_u16().await.unwrap()), payload.len());
                    let mut data = vec![0; payload.len()];
                    peer.read_exact(&mut data).await.unwrap();
                    assert_eq!(data, payload);
                }
                // The first oversize frame fits the sentinel buffer; the second
                // is rejected by the carrier. Neither can appear on the TUN.
                for size in [limit + 1, limit + 2, 0, limit, 4] {
                    peer.write_u16(size as u16).await.unwrap();
                    peer.write_all(&vec![42; size]).await.unwrap();
                }
                for size in [0, limit, 4] {
                    let mut wire = vec![0; ip_header + 8 + size];
                    device.read_exact(&mut wire).await.unwrap();
                    let parsed = etherparse::SlicedPacket::from_ip(&wire).unwrap();
                    let Some(etherparse::TransportSlice::Udp(reply)) = parsed.transport else {
                        panic!("expected UDP reply");
                    };
                    assert_eq!(reply.payload(), &vec![42; size]);
                    assert_eq!(
                        (reply.source_port(), reply.destination_port()),
                        (443, 40000)
                    );
                    if v6 {
                        let checksum = reply
                            .to_header()
                            .calc_checksum_ipv6_raw([2; 16], [1; 16], reply.payload())
                            .unwrap();
                        assert_eq!(checksum, reply.checksum());
                    }
                }
                assert_eq!(stats.bytes_sent(), 2);
                assert_eq!(stats.bytes_received(), (limit + 4) as u64);
                task.abort();
                let _ = task.await;
            }
        })
        .await
        .expect("TUN UDP did not preserve the association");
    }

    struct TcpOnly;
    impl super::super::connector::Outbound for TcpOnly {
        fn connect(
            &self,
            _: &str,
            _: u16,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = anyhow::Result<super::super::connector::BoxProxyStream>,
                    > + Send
                    + '_,
            >,
        > {
            panic!("UDP must never invoke the TCP connector");
        }
        fn name(&self) -> &str {
            "tcp-only"
        }
    }

    #[tokio::test]
    async fn unsupported_udp_never_falls_back_to_tcp() {
        use super::super::connector::{Outbound, SharedOutbound, SwappableOutbound};
        let wrapped = SharedOutbound(std::sync::Arc::new(TcpOnly)).with_retry(3);
        let outbound = SwappableOutbound::new(wrapped);
        assert!(outbound.connect_datagram("192.0.2.1", 443).await.is_err());
    }

    #[tokio::test]
    async fn direct_udp_tries_next_address_after_local_route_failure() {
        let real = "127.0.0.1:53".parse().unwrap();
        let candidates = vec![
            "198.18.0.3:53".parse().unwrap(),
            "[2001:db8::1]:53".parse().unwrap(),
            real,
        ];
        let tried = std::cell::RefCell::new(Vec::new());
        let connected = connect_direct_candidates(candidates, |addr| {
            tried.borrow_mut().push(addr);
            std::future::ready(if addr == real {
                Ok(addr)
            } else {
                Err(anyhow::anyhow!("no IPv6 route"))
            })
        })
        .await
        .unwrap();
        assert_eq!(connected, real);
        assert_eq!(
            tried.into_inner(),
            ["[2001:db8::1]:53".parse().unwrap(), real]
        );
    }

    #[tokio::test]
    async fn direct_udp_preserves_separate_messages() {
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = server.local_addr().unwrap();
        use super::super::connector::{Outbound, SharedOutbound, SwappableOutbound};
        let outbound = SwappableOutbound::new(SharedOutbound::direct().with_retry(3));
        let client = outbound
            .connect_datagram("127.0.0.1", addr.port())
            .await
            .unwrap();
        for packet in [&b"first"[..], &b"second packet"[..], &b""[..]] {
            client.send(packet).await.unwrap();
            let mut buf = [0; 100];
            let (n, peer) = server.recv_from(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], packet);
            server.send_to(&buf[..n], peer).await.unwrap();
            let n = client.recv(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], packet);
        }
    }
}

#[cfg(test)]
mod fake_direct_tests {
    #[tokio::test]
    async fn direct_udp_rejects_unrestored_fake_addresses() {
        assert!(super::connect_direct("198.18.0.42", 443).await.is_err());
    }
}

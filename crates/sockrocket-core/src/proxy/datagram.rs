//! Datagram transport preserves message boundaries; it is never a TCP stream.
use anyhow::Result;
use std::{future::Future, pin::Pin, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::UdpSocket,
};

pub type DatagramFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;
pub trait Datagram: Send + Sync {
    fn send<'a>(&'a self, packet: &'a [u8]) -> DatagramFuture<'a, ()>;
    fn recv<'a>(&'a self, packet: &'a mut [u8]) -> DatagramFuture<'a, usize>;
}
pub type BoxDatagram = Box<dyn Datagram>;

impl Datagram for UdpSocket {
    fn send<'a>(&'a self, packet: &'a [u8]) -> DatagramFuture<'a, ()> {
        Box::pin(async move {
            let sent = UdpSocket::send(self, packet).await?;
            anyhow::ensure!(sent == packet.len(), "Partial UDP datagram write");
            Ok(())
        })
    }
    fn recv<'a>(&'a self, packet: &'a mut [u8]) -> DatagramFuture<'a, usize> {
        Box::pin(async move { Ok(UdpSocket::recv(self, packet).await?) })
    }
}

pub(super) async fn connect_direct(host: &str, port: u16) -> Result<BoxDatagram> {
    // A device direct rule may restore a Fake-IP to its domain. On Merlin
    // the OS resolver can return that token again; use the same bootstrap
    // path as direct TCP and never send UDP back into the fake-address pool.
    let resolved = if let Some(ips) = super::transport::resolve_server_ips(host).await {
        ips.into_iter()
            .map(|ip| std::net::SocketAddr::new(ip, port))
            .collect::<Vec<_>>()
    } else {
        tokio::net::lookup_host((host, port)).await?.collect()
    };
    let addr = resolved
        .into_iter()
        .find(|addr| crate::dns::fakeip::fake_ipv4(addr.ip()).is_none())
        .ok_or_else(|| {
            anyhow::anyhow!("No real UDP destination address (Fake-IP cannot be dialed directly)")
        })?;
    let socket = UdpSocket::bind(if addr.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    })
    .await?;
    super::outbound_bind::apply_to_udp_socket(&socket, addr.is_ipv6())?;
    socket.connect(addr).await?;
    Ok(Box::new(socket))
}

pub(super) async fn relay<S: AsyncRead + AsyncWrite + Unpin>(
    client: &mut S,
    remote: &dyn Datagram,
) -> Result<()> {
    const IDLE: Duration = Duration::from_secs(30);
    let (mut reader, mut writer) = tokio::io::split(client);
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
        let mut outbound = vec![0; 65535];
        loop {
            let size = reader.read(&mut outbound).await?;
            if size == 0 {
                return Ok::<_, anyhow::Error>(());
            }
            progress.send_modify(|state| {
                state.last = tokio::time::Instant::now();
                state.writing += 1;
            });
            tokio::time::timeout(IDLE, remote.send(&outbound[..size]))
                .await
                .map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::TimedOut, "UDP upstream write stalled")
                })??;
            progress.send_modify(|state| {
                state.last = tokio::time::Instant::now();
                state.writing -= 1;
            });
            tokio::task::consume_budget().await;
        }
    };
    let download = async {
        let mut inbound = vec![0; 65535];
        loop {
            let size = match remote.recv(&mut inbound).await {
                Ok(size) => size,
                Err(error) => return Err::<(), _>(error),
            };
            progress.send_modify(|state| {
                state.last = tokio::time::Instant::now();
                state.writing += 1;
            });
            tokio::time::timeout(IDLE, writer.write_all(&inbound[..size]))
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

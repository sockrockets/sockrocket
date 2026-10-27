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
    let mut outbound = vec![0; 65535];
    let mut inbound = vec![0; 65535];
    loop {
        tokio::select! {
            packet = client.read(&mut outbound) => {
                let size = packet?;
                if size == 0 { return Ok(()); }
                tokio::time::timeout(Duration::from_secs(30), remote.send(&outbound[..size]))
                    .await.map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "UDP upstream write stalled"))??;
            }
            packet = remote.recv(&mut inbound) => {
                let size = packet?;
                tokio::time::timeout(Duration::from_secs(30), client.write_all(&inbound[..size]))
                    .await.map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "UDP client write stalled"))??;
            }
            _ = tokio::time::sleep(Duration::from_secs(30)) => return Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

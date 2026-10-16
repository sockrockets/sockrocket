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
    let addr = tokio::net::lookup_host((host, port))
        .await?
        .next()
        .ok_or_else(|| anyhow::anyhow!("No UDP destination address"))?;
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
                remote.send(&outbound[..size]).await?;
            }
            packet = remote.recv(&mut inbound) => {
                let size = packet?;
                client.write_all(&inbound[..size]).await?;
            }
            _ = tokio::time::sleep(Duration::from_secs(30)) => return Ok(()),
        }
    }
}

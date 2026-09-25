use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use anyhow::{Context as _, Result, bail};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf, WriteHalf};

use crate::config::model::TransportConfig;

use super::connector::{BoxProxyStream, Outbound};
use super::pool::ConnPool;
use super::transport::TlsConnFactory;

/// Factory that creates TCP+TLS connections to the VLESS proxy server.
/// VLESS outbound connector.
pub struct VlessOutbound {
    server: String,
    port: u16,
    uuid: [u8; 16],
    pool: ConnPool,
}

impl VlessOutbound {
    pub fn new(
        server: &str,
        port: u16,
        uuid_str: &str,
        transport: Option<&TransportConfig>,
    ) -> Result<Self> {
        Self::build(server, port, uuid_str, transport, true)
    }

    /// Probe/latency variant: the connection pool never warms in the
    /// background, so a one-shot latency test doesn't pre-connect a full
    /// pool of TLS handshakes at the server.
    pub(crate) fn new_unwarmed(
        server: &str,
        port: u16,
        uuid_str: &str,
        transport: Option<&TransportConfig>,
    ) -> Result<Self> {
        Self::build(server, port, uuid_str, transport, false)
    }

    fn build(
        server: &str,
        port: u16,
        uuid_str: &str,
        transport: Option<&TransportConfig>,
        warm_pool: bool,
    ) -> Result<Self> {
        let uuid = uuid::Uuid::parse_str(uuid_str)?;

        let factory = Arc::new(TlsConnFactory::from_transport(
            server, port, transport, false,
        ));
        let pool = if warm_pool {
            ConnPool::new(factory)
        } else {
            ConnPool::lazy(factory)
        };

        Ok(Self {
            server: server.to_string(),
            port,
            uuid: *uuid.as_bytes(),
            pool,
        })
    }
}

impl Outbound for VlessOutbound {
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
        let target_host = host.to_string();
        Box::pin(async move {
            let mut stream = self.pool.get().await.with_context(|| {
                format!(
                    "VLESS: failed to connect to {}:{} (target: {}:{})",
                    self.server, self.port, target_host, port
                )
            })?;

            let header = build_vless_request(&self.uuid, &target_host, port)?;
            stream.write_all(&header).await?;
            stream.flush().await?;

            // Lazy response header: the 2-byte response header (+ addons) is
            // consumed on the first read instead of blocking connect() for an
            // extra RTT. A malformed header still surfaces as a read error.
            Ok(Box::new(VlessStream::new(stream)) as BoxProxyStream)
        })
    }

    fn name(&self) -> &str {
        "vless"
    }
}

/// Build VLESS request header.
///
/// Format:
/// [version(1)] [UUID(16)] [addon_len(1)] [addons...]
/// [command(1)] [port(2, big-endian)] [addr_type(1)] [addr_data...]
fn build_vless_request(uuid: &[u8; 16], host: &str, port: u16) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(64);

    // Version
    buf.push(0x00);
    // UUID
    buf.extend_from_slice(uuid);
    // Addon length (no addons)
    buf.push(0x00);
    // Command: TCP (0x01)
    buf.push(0x01);
    // Port (big-endian)
    buf.extend_from_slice(&port.to_be_bytes());

    // Address
    if let Ok(ipv4) = host.parse::<std::net::Ipv4Addr>() {
        buf.push(0x01); // IPv4
        buf.extend_from_slice(&ipv4.octets());
    } else if let Ok(ipv6) = host.parse::<std::net::Ipv6Addr>() {
        buf.push(0x03); // IPv6
        buf.extend_from_slice(&ipv6.octets());
    } else {
        // Domain name
        buf.push(0x02); // Domain
        let domain_bytes = host.as_bytes();
        anyhow::ensure!(
            domain_bytes.len() <= 255,
            "Domain name too long for VLESS: {} bytes (max 255)",
            domain_bytes.len()
        );
        buf.push(domain_bytes.len() as u8);
        buf.extend_from_slice(domain_bytes);
    }

    Ok(buf)
}

/// Read VLESS response header.
///
/// Format: [version(1)] [addon_len(1)] [addons...]
async fn read_vless_response<R: AsyncRead + Unpin>(reader: &mut R) -> Result<()> {
    let version = reader.read_u8().await?;
    if version != 0x00 {
        bail!("Unexpected VLESS response version: {}", version);
    }
    let addon_len = reader.read_u8().await?;
    if addon_len > 0 {
        let mut addon = vec![0u8; addon_len as usize];
        reader.read_exact(&mut addon).await?;
    }
    Ok(())
}

/// In-flight consumption of the VLESS response header; owns the read half
/// until done and returns it once the header has been consumed.
type VlessHandshake =
    Pin<Box<dyn Future<Output = io::Result<tokio::io::ReadHalf<BoxProxyStream>>> + Send>>;

/// VLESS stream that consumes the response header lazily on the first read.
///
/// The read half is split off so writes never wait for the response header
/// (the server may delay it until the target is connected).
struct VlessStream {
    writer: WriteHalf<BoxProxyStream>,
    /// Read half — taken by the handshake future on the first read and
    /// returned once the response header has been consumed.
    reader: Option<tokio::io::ReadHalf<BoxProxyStream>>,
    /// In-flight header consumption; owns the read half until done.
    handshake: Option<VlessHandshake>,
    /// Set once the response header has been consumed.
    handshake_done: bool,
    /// Set after the handshake failed once; subsequent reads return EOF.
    failed: bool,
}

impl VlessStream {
    fn new(stream: BoxProxyStream) -> Self {
        let (reader, writer) = tokio::io::split(stream);
        Self {
            writer,
            reader: Some(reader),
            handshake: None,
            handshake_done: false,
            failed: false,
        }
    }
}

impl AsyncRead for VlessStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            let this = &mut *self;
            if let Some(fut) = &mut this.handshake {
                match fut.as_mut().poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Ok(reader)) => {
                        this.reader = Some(reader);
                        this.handshake = None;
                        this.handshake_done = true;
                    }
                    Poll::Ready(Err(e)) => {
                        this.handshake = None;
                        this.failed = true;
                        return Poll::Ready(Err(e));
                    }
                }
            } else if this.handshake_done {
                let reader = this.reader.as_mut().expect("handshake done implies reader");
                return Pin::new(reader).poll_read(cx, buf);
            } else if this.failed {
                // Handshake already reported its error; read as EOF now.
                return Poll::Ready(Ok(()));
            } else {
                // First read: start consuming the response header.
                let mut reader = this.reader.take().expect("reader present before handshake");
                this.handshake = Some(Box::pin(async move {
                    read_vless_response(&mut reader)
                        .await
                        .map(|_| reader)
                        .map_err(io::Error::other)
                }));
            }
        }
    }
}

impl AsyncWrite for VlessStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.writer).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.writer).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.writer).poll_shutdown(cx)
    }
}

impl Unpin for VlessStream {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_vless_request_domain() {
        let uuid = [0x01u8; 16];
        let buf = build_vless_request(&uuid, "example.com", 443).unwrap();

        assert_eq!(buf[0], 0x00); // version
        assert_eq!(&buf[1..17], &[0x01u8; 16]); // UUID
        assert_eq!(buf[17], 0x00); // no addons
        assert_eq!(buf[18], 0x01); // TCP command
        assert_eq!(u16::from_be_bytes([buf[19], buf[20]]), 443); // port
        assert_eq!(buf[21], 0x02); // domain type
        assert_eq!(buf[22], 11); // "example.com" length
        assert_eq!(&buf[23..34], b"example.com");
    }

    #[test]
    fn test_build_vless_request_ipv4() {
        let uuid = [0xAA; 16];
        let buf = build_vless_request(&uuid, "1.2.3.4", 80).unwrap();

        assert_eq!(buf[21], 0x01); // IPv4 type
        assert_eq!(&buf[22..26], &[1, 2, 3, 4]); // IPv4 addr
    }

    #[test]
    fn test_build_vless_request_ipv6() {
        let uuid = [0xBB; 16];
        let buf = build_vless_request(&uuid, "::1", 8080).unwrap();

        assert_eq!(buf[21], 0x03); // IPv6 type
        let expected_ipv6 = std::net::Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1).octets();
        assert_eq!(&buf[22..38], &expected_ipv6);
    }

    /// The lazy response header: the stream is usable immediately after the
    /// request header is written; the first read transparently consumes the
    /// response header (version + addon_len + addons) before the payload.
    #[tokio::test]
    async fn vless_stream_consumes_response_header_lazily() {
        let (client, mut server) = tokio::io::duplex(64 * 1024);
        let server_task = tokio::spawn(async move {
            // Response: version 0, addon_len 2, addon bytes, then payload.
            server.write_all(&[0x00, 0x02, 0xAA, 0xBB]).await.unwrap();
            server.write_all(b"hello").await.unwrap();
        });

        let mut stream = VlessStream::new(Box::new(client) as BoxProxyStream);

        // Writes work before any read (never blocked on the response header).
        stream.write_all(b"ping").await.unwrap();

        let mut buf = [0u8; 5];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");

        server_task.await.unwrap();
    }

    /// A bad response header must surface as an error on the first read,
    /// not a silent EOF.
    #[tokio::test]
    async fn vless_stream_bad_response_version_errors_on_first_read() {
        let (client, mut server) = tokio::io::duplex(1024);
        let server_task = tokio::spawn(async move {
            server.write_all(&[0x01, 0x00]).await.unwrap(); // version 1: invalid
        });

        let mut stream = VlessStream::new(Box::new(client) as BoxProxyStream);
        let mut buf = [0u8; 16];
        let err = stream.read(&mut buf).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("Unexpected VLESS response version"),
            "unexpected error: {err}"
        );
        server_task.await.unwrap();
    }
}

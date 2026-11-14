//! Mieru v3 TCP carrier, with one session per upstream connection.
use super::{
    connector::{BoxProxyStream, Outbound, UPSTREAM_CONNECT_TIMEOUT},
    transport,
};
use crate::config::model::Node;
use anyhow::{Result, anyhow, ensure};
use bytes::Bytes;
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, aead::Aead};
use futures_util::{
    stream,
    task::{ArcWake, AtomicWaker, waker_ref},
};
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, ready},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_util::io::{StreamReader, poll_write_buf};

pub(super) struct Mieru {
    endpoint: String,
    username: String,
    password_hash: [u8; 32],
}

fn now() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

fn current_minute() -> Result<u32> {
    Ok((now()? / 60) as u32)
}

struct Direction {
    cipher: XChaCha20Poly1305,
    nonce: [u8; 24],
}
impl Direction {
    fn advance(&mut self) {
        for n in self.nonce.iter_mut().rev() {
            *n = n.wrapping_add(1);
            if *n != 0 {
                break;
            }
        }
    }
    fn seal(&mut self, plain: &[u8]) -> Result<Vec<u8>> {
        let result = self
            .cipher
            .encrypt((&self.nonce).into(), plain)
            .map_err(|_| anyhow!("Mieru encryption failed"))?;
        self.advance();
        Ok(result)
    }
    fn open(&mut self, sealed: &[u8]) -> Result<Vec<u8>> {
        let result = self
            .cipher
            .decrypt((&self.nonce).into(), sealed)
            .map_err(|_| anyhow!("Mieru frame authentication failed"))?;
        self.advance();
        Ok(result)
    }
    fn frame(&mut self, id: u32, seq: u32, kind: u8, body: &[u8]) -> Result<Vec<u8>> {
        let mut header = [0u8; 32];
        header[0] = kind;
        header[2..6].copy_from_slice(&current_minute()?.to_be_bytes());
        header[6..10].copy_from_slice(&id.to_be_bytes());
        header[10..14].copy_from_slice(&seq.to_be_bytes());
        let offset = if kind == 6 { 22 } else { 15 };
        header[offset..offset + 2].copy_from_slice(&u16::try_from(body.len())?.to_be_bytes());
        let mut frame = self.seal(&header)?;
        if !body.is_empty() {
            frame.extend(self.seal(body)?);
        }
        Ok(frame)
    }
    async fn write(
        &mut self,
        w: &mut (impl AsyncWrite + Unpin),
        id: u32,
        seq: u32,
        kind: u8,
        body: &[u8],
    ) -> Result<()> {
        w.write_all(&self.frame(id, seq, kind, body)?).await?;
        Ok(())
    }
    async fn read(&mut self, r: &mut (impl AsyncRead + Unpin), id: u32) -> Result<(u8, Vec<u8>)> {
        let mut raw = [0; 48];
        r.read_exact(&mut raw).await?;
        let h = self.open(&raw)?;
        let minute = u32::from_be_bytes(h[2..6].try_into()?);
        ensure!(
            current_minute()?.abs_diff(minute) <= 1,
            "Invalid Mieru frame timestamp"
        );
        ensure!(h[6..10] == id.to_be_bytes(), "Mieru session mismatch");
        let (prefix, len, suffix) = match h[0] {
            3..=5 => {
                ensure!(h[14] == 0, "Mieru session refused");
                let len = u16::from_be_bytes([h[15], h[16]]);
                ensure!(len <= 1024, "Mieru session payload exceeds limit");
                (0, len, h[17])
            }
            7 | 9 => {
                let len = u16::from_be_bytes([h[22], h[23]]);
                ensure!(len <= 32768, "Mieru data payload exceeds limit");
                ensure!(h[0] != 9 || len == 0, "Unexpected Mieru ACK payload");
                (h[21], len, h[24])
            }
            _ => return Err(anyhow!("Unsupported Mieru frame")),
        };
        let mut padding = [0; 255];
        r.read_exact(&mut padding[..prefix as usize]).await?;
        let body = if len > 0 {
            let mut bytes = vec![0; usize::from(len) + 16];
            r.read_exact(&mut bytes).await?;
            self.open(&bytes)?
        } else {
            Vec::new()
        };
        r.read_exact(&mut padding[..suffix as usize]).await?;
        Ok((h[0], body))
    }
}
impl Mieru {
    pub(super) fn new(node: &Node, username: &str, password: &str) -> Self {
        Self {
            endpoint: transport::format_host_port(&node.server, node.port),
            username: username.into(),
            password_hash: Sha256::new()
                .chain_update(password)
                .chain_update([0])
                .chain_update(username)
                .finalize()
                .into(),
        }
    }
    async fn dial(&self, host: &str, port: u16) -> Result<BoxProxyStream> {
        let stream =
            transport::connect_server_tcp(&self.endpoint, UPSTREAM_CONNECT_TIMEOUT).await?;
        tokio::time::timeout(UPSTREAM_CONNECT_TIMEOUT, self.handshake(stream, host, port))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Mieru handshake timed out"))?
    }
    async fn handshake<S>(&self, mut stream: S, host: &str, port: u16) -> Result<BoxProxyStream>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let mut request = vec![5, 1, 0];
        request.extend(super::anytls::socks_address(host, port)?);
        let epoch = ((now()? + 60) / 120) * 120;
        let key = pbkdf2::pbkdf2_hmac_array::<Sha256, 32>(
            &self.password_hash,
            &Sha256::digest(epoch.to_be_bytes()),
            64,
        );
        let mut nonce = rand::random::<[u8; 24]>();
        let hint = Sha256::new()
            .chain_update(&self.username)
            .chain_update(&nonce[..16])
            .finalize();
        nonce[20..].copy_from_slice(&hint[..4]);
        let mut tx = Direction {
            cipher: XChaCha20Poly1305::new((&key).into()),
            nonce,
        };
        let id = rand::random_range(1..=u32::MAX);
        stream.write_all(&nonce).await?;
        tx.write(&mut stream, id, 0, 2, &request).await?;
        let mut server_nonce = [0; 24];
        stream.read_exact(&mut server_nonce).await?;
        ensure!(nonce != server_nonce, "Invalid Mieru response nonce");
        let mut rx = Direction {
            cipher: tx.cipher.clone(),
            nonce: server_nonce,
        };
        let mut reply = Vec::new();
        // The SOCKS reply and the first application bytes can share a segment.
        // Accumulate only until the variable-length SOCKS address is complete.
        let consumed = loop {
            if reply.len() >= 5 {
                ensure!(reply[..3] == [5, 0, 0], "Mieru destination refused");
                let needed = match reply[3] {
                    1 => 10,
                    4 => 22,
                    3 => 7 + usize::from(reply[4]),
                    _ => return Err(anyhow!("Invalid Mieru reply address")),
                };
                if reply.len() >= needed {
                    break needed;
                }
            }
            let (kind, part) = rx.read(&mut stream, id).await?;
            ensure!(matches!(kind, 3 | 7 | 9), "Mieru closed during handshake");
            reply.extend(part);
        };
        let (reader, writer) = tokio::io::split(stream);
        // The pinned stream retains partially read records if a caller cancels a read.
        let records = stream::try_unfold((reader, rx), move |(mut reader, mut rx)| async move {
            loop {
                let (kind, body) = rx.read(&mut reader, id).await.map_err(io::Error::other)?;
                match kind {
                    7 => return Ok(Some((Bytes::from(body), (reader, rx)))),
                    9 => continue,
                    4 | 5 => return Ok(None),
                    _ => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Unexpected Mieru session frame",
                        ));
                    }
                }
            }
        });
        let prefix = Bytes::from(reply).slice(consumed..);
        let input = io::Cursor::new(prefix).chain(StreamReader::new(Box::pin(records)));
        Ok(Box::new(MieruStream {
            reader: input,
            carrier: writer,
            tx,
            id,
            seq: 1,
            pending: Bytes::new(),
            write_waiters: Default::default(),
            closed: false,
            write_lost: false,
        }))
    }
}
// Read and write stay independent when the carrier is flow-controlled. Poll
// pending output on reads as well, so write_all followed by read can progress
// even if the last accepted record did not initially fit in the socket.
struct MieruStream<R, W> {
    reader: R,
    carrier: W,
    tx: Direction,
    id: u32,
    seq: u32,
    pending: Bytes,
    write_waiters: Arc<WriteWaiters>,
    closed: bool,
    write_lost: bool,
}
// Both halves may drive a pending carrier write. Keep a stable carrier waker:
// most transports retain only the most recent waker for each direction.
#[derive(Default)]
struct WriteWaiters {
    reader: AtomicWaker,
    writer: AtomicWaker,
}
impl ArcWake for WriteWaiters {
    fn wake_by_ref(this: &Arc<Self>) {
        this.reader.wake();
        this.writer.wake();
    }
}

impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> AsyncRead for MieruStream<R, W> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        self.write_waiters.reader.register(cx.waker());
        if let Poll::Ready(Err(e)) = self.drain() {
            return Poll::Ready(Err(e));
        }
        let before = buf.filled().len();
        let result = Pin::new(&mut self.reader).poll_read(cx, buf);
        // A Mieru peer close ends the entire session. Never accept more
        // application writes after an authenticated close or a frame error.
        if result.is_ready() && buf.filled().len() == before {
            self.closed = true;
            if !self.pending.is_empty() {
                self.write_lost = true;
                self.pending = Bytes::new();
            }
            self.write_waiters.writer.wake();
        }
        result
    }
}
// Queue at most one encrypted record. A pending transport write stays owned
// by this stream, so dropping the stream also releases the carrier socket.
impl<R, W: AsyncWrite + Unpin> MieruStream<R, W> {
    fn drain(&mut self) -> Poll<io::Result<()>> {
        let waker = waker_ref(&self.write_waiters);
        let mut cx = Context::from_waker(&waker);
        while !self.pending.is_empty() {
            if ready!(poll_write_buf(
                Pin::new(&mut self.carrier),
                &mut cx,
                &mut self.pending
            ))? == 0
            {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
        }
        Poll::Ready(Ok(()))
    }
}

impl<R: Unpin, W: AsyncWrite + Unpin> AsyncWrite for MieruStream<R, W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.as_mut().get_mut();
        if this.closed {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        this.write_waiters.writer.register(cx.waker());
        ready!(this.drain())?;
        let size = data.len().min(16384);
        if size == 0 {
            return Poll::Ready(Ok(0));
        }
        this.pending = this
            .tx
            .frame(this.id, this.seq, 6, &data[..size])
            .map_err(io::Error::other)?
            .into();
        this.seq = this.seq.wrapping_add(1);
        // Start transmission before the caller switches from writing to reading.
        if let Poll::Ready(Err(e)) = this.drain() {
            return Poll::Ready(Err(e));
        }
        Poll::Ready(Ok(size))
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.write_lost {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        self.write_waiters.writer.register(cx.waker());
        ready!(self.drain())?;
        Pin::new(&mut self.carrier).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Mieru close and carrier FIN terminate both directions. Preserve the
        // receive side until the peer ends its session or the owner drops it.
        self.closed = true;
        self.poll_flush(cx)
    }
}
impl Outbound for Mieru {
    fn name(&self) -> &str {
        "mieru"
    }
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
        let host = host.to_owned();
        Box::pin(async move { self.dial(&host, port).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Independent libsodium XChaCha20-Poly1305 vectors for consecutive TCP records.
    #[test]
    fn mieru_implicit_nonce_vectors() -> Result<()> {
        let key: [u8; 32] = std::array::from_fn(|i| i as u8);
        let mut sender = Direction {
            cipher: XChaCha20Poly1305::new((&key).into()),
            nonce: std::array::from_fn(|i| i as u8),
        };
        assert_eq!(
            hex::encode(sender.seal(&key)?),
            "9ec30d7c94d78ba93b4d2cc5c75fa6e75b563ab6e9c30bfc670325ad21b0eb4630ed4d7a137d844f4270ec7ba32ca9bd"
        );
        assert_eq!(
            hex::encode(sender.seal(b"ping")?),
            "618f80e69e100079351fc3e605de1b6ea3621ce5"
        );
        Ok(())
    }

    #[tokio::test]
    async fn mieru_rejects_invalid_metadata_before_reading_body() -> Result<()> {
        let minute = current_minute()?;
        for (kind, timestamp, len, expected) in [
            (3, minute - 2, 0u16, "timestamp"),
            (7, minute + 3, 0, "timestamp"),
            (3, minute, 1025, "session payload"),
            (7, minute, 32769, "data payload"),
            (9, minute, 1, "ACK payload"),
        ] {
            let mut header = [0; 32];
            header[0] = kind;
            header[2..6].copy_from_slice(&timestamp.to_be_bytes());
            let offset = if kind == 3 { 15 } else { 22 };
            header[offset..offset + 2].copy_from_slice(&len.to_be_bytes());
            let direction = || Direction {
                cipher: XChaCha20Poly1305::new((&[17; 32]).into()),
                nonce: [23; 24],
            };
            let wire = direction().seal(&header)?;
            let err = direction().read(&mut wire.as_slice(), 0).await.unwrap_err();
            assert!(err.to_string().contains(expected), "{err}");
        }
        Ok(())
    }
    fn test_stream<R, W>(reader: R, carrier: W) -> MieruStream<R, W> {
        MieruStream {
            reader,
            carrier,
            id: 1,
            seq: 1,
            closed: false,
            write_lost: false,
            pending: Bytes::new(),
            write_waiters: Default::default(),
            tx: Direction {
                cipher: XChaCha20Poly1305::new((&[7; 32]).into()),
                nonce: [11; 24],
            },
        }
    }
    #[tokio::test]
    async fn mieru_flush_reports_accepted_bytes_lost_on_peer_close() {
        let (carrier, _blocked_peer) = tokio::io::duplex(1);
        let mut stream = test_stream(tokio::io::empty(), carrier);
        stream.write_all(b"data").await.unwrap();
        assert!(
            Pin::new(&mut stream)
                .poll_flush(&mut Context::from_waker(std::task::Waker::noop()))
                .is_pending()
        );
        assert_eq!(stream.read(&mut [0]).await.unwrap(), 0);
        assert_eq!(
            stream.flush().await.unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
        assert_eq!(stream.read(&mut [0]).await.unwrap(), 0);
        assert_eq!(
            stream.shutdown().await.unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
    }
    #[tokio::test]
    async fn mieru_split_writer_wakes_after_read_polls_blocked_carrier() {
        use std::{
            sync::atomic::{AtomicBool, Ordering},
            task::{Wake, Waker},
        };
        #[derive(Default)]
        struct Signal(AtomicBool);
        impl Wake for Signal {
            fn wake(self: Arc<Self>) {
                self.0.store(true, Ordering::Release);
            }
        }
        let (reader, _silent) = tokio::io::duplex(1);
        let (carrier, mut peer) = tokio::io::duplex(1);
        let (mut input, mut output) = tokio::io::split(test_stream(reader, carrier));
        output.write_all(b"data").await.unwrap();
        let signal = Arc::new(Signal::default());
        let writer_waker = Waker::from(signal.clone());
        let reader_waker = Waker::from(Arc::new(Signal::default()));
        assert!(
            Pin::new(&mut output)
                .poll_flush(&mut Context::from_waker(&writer_waker))
                .is_pending()
        );
        let mut storage = [0];
        let mut buf = tokio::io::ReadBuf::new(&mut storage);
        assert!(
            Pin::new(&mut input)
                .poll_read(&mut Context::from_waker(&reader_waker), &mut buf)
                .is_pending()
        );
        peer.read_u8().await.unwrap();
        assert!(signal.0.load(Ordering::Acquire));
    }
    #[tokio::test]
    async fn mieru_handshake_and_stream() -> Result<()> {
        let node = crate::config::v2ray::parse_proxy_uri(
            "mierus://test:local@localhost?profile=test&protocol=TCP&port=1080",
        )?;
        let client = Mieru::new(&node, "test", "local");
        let (local, mut peer) = tokio::io::duplex(16);
        let (closed, wait_for_shutdown) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let mut nonce = [0; 24];
            peer.read_exact(&mut nonce).await?;
            let digest = Sha256::new()
                .chain_update(b"test")
                .chain_update(&nonce[..16])
                .finalize();
            ensure!(nonce[20..] == digest[..4]);
            let epoch = ((now()? + 60) / 120) * 120;
            let key = pbkdf2::pbkdf2_hmac_array::<Sha256, 32>(
                &Sha256::digest(b"local\0test"),
                &Sha256::digest(epoch.to_be_bytes()),
                64,
            );
            let cipher = XChaCha20Poly1305::new((&key).into());
            let mut rx = Direction {
                cipher: cipher.clone(),
                nonce,
            };
            let mut raw = [0; 48];
            peer.read_exact(&mut raw).await?;
            let header = rx.open(&raw)?;
            ensure!(header[0] == 2 && header[10..14] == [0; 4]);
            let id = u32::from_be_bytes(header[6..10].try_into()?);
            let mut req = vec![0; usize::from(u16::from_be_bytes(header[15..17].try_into()?)) + 16];
            peer.read_exact(&mut req).await?;
            ensure!(rx.open(&req)? == b"\x05\x01\x00\x03\x04test\x01\xbb");
            let mut tx = Direction {
                cipher,
                nonce: [23; 24],
            };
            peer.write_all(&tx.nonce).await?;
            tx.write(&mut peer, id, 0, 3, &[5, 0, 0, 1, 0, 0, 0, 0, 0, 0])
                .await?;
            peer.read_exact(&mut raw).await?;
            let header = rx.open(&raw)?;
            ensure!(
                header[0] == 6
                    && header[10..14] == 1u32.to_be_bytes()
                    && header[22..24] == 4u16.to_be_bytes()
            );
            let mut body = [0; 20];
            peer.read_exact(&mut body).await?;
            ensure!(rx.open(&body)? == b"ping");
            wait_for_shutdown.await?;
            let mut header = [0; 32];
            header[0] = 7;
            header[2..6].copy_from_slice(&current_minute()?.to_be_bytes());
            header[6..10].copy_from_slice(&id.to_be_bytes());
            header[22..24].copy_from_slice(&4u16.to_be_bytes());
            peer.write_all(&tx.seal(&header)?).await?;
            peer.write_all(&tx.seal(b"pong")?).await?;
            // A bad record after valid data must surface as a read error.
            peer.write_all(&[0; 48]).await?;
            Ok::<_, anyhow::Error>(())
        });
        let exchange = async {
            let mut socket = client.handshake(local, "test", 443).await?;
            socket.write_all(b"ping").await?;
            socket.shutdown().await?;
            assert_eq!(
                socket.write(b"extra").await.unwrap_err().kind(),
                io::ErrorKind::BrokenPipe
            );
            closed.send(()).unwrap();
            let mut response = [0; 4];
            socket.read_exact(&mut response).await?;
            ensure!(&response == b"pong");
            assert!(
                socket
                    .read(&mut response)
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("authentication")
            );
            server.await??;
            Ok::<_, anyhow::Error>(())
        };
        tokio::time::timeout(std::time::Duration::from_secs(3), exchange).await??;
        Ok(())
    }
}

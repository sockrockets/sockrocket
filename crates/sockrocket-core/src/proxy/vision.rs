//! Native VLESS Vision framing and TLS-to-raw transitions.
//! Wire compatibility reference: Xray-core proxy/proxy.go (Vision), not a
//! runtime dependency. Outer TLS reads are limited to one record so a Direct
//! marker cannot accidentally consume following raw inner-TLS records.
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub(super) const FLOW: &str = "xtls-rprx-vision";
const CHUNK: usize = 16 * 1024;
const MAX_FRAME: usize = 21 + 2 * u16::MAX as usize;

pub(super) struct RecordIo<T> {
    pub io: T,
    header: [u8; 5],
    header_len: usize,
    remaining: usize,
}
impl<T> RecordIo<T> {
    pub fn new(io: T) -> Self {
        Self {
            io,
            header: [0; 5],
            header_len: 0,
            remaining: 0,
        }
    }
    fn at_boundary(&self) -> bool {
        self.header_len == 0 && self.remaining == 0
    }
}
impl<T: AsyncRead + Unpin> AsyncRead for RecordIo<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let header = this.remaining == 0;
        let max = if header {
            5 - this.header_len
        } else {
            this.remaining
        };
        let limit = max.min(buf.remaining());
        let mut limited = ReadBuf::new(buf.initialize_unfilled_to(limit));
        match Pin::new(&mut this.io).poll_read(cx, &mut limited) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Ready(Ok(())) => {
                let bytes = limited.filled();
                let n = bytes.len();
                if n == 0 && !this.at_boundary() {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "Truncated outer TLS record",
                    )));
                }
                if header {
                    this.header[this.header_len..this.header_len + n].copy_from_slice(bytes);
                    this.header_len += n;
                    if this.header_len == 5 {
                        this.remaining =
                            u16::from_be_bytes([this.header[3], this.header[4]]) as usize;
                        if this.remaining == 0 {
                            this.header_len = 0;
                        }
                    }
                } else {
                    this.remaining -= n;
                    if this.remaining == 0 {
                        this.header_len = 0;
                    }
                }
                // The bounded read initialized this prefix of the caller buffer.
                buf.advance(n);
                Poll::Ready(Ok(()))
            }
        }
    }
}
impl<T: AsyncWrite + Unpin> AsyncWrite for RecordIo<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().io).poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_shutdown(cx)
    }
}

#[derive(Default)]
struct Records {
    bytes: Vec<u8>,
    handshake: Vec<u8>,
    enabled: bool,
    seen_tls: bool,
    failed: bool,
}
impl Records {
    fn feed(&mut self, bytes: &[u8], server: bool) -> bool {
        if self.failed {
            return false;
        }
        if self.bytes.len() + bytes.len() > 131072 {
            self.failed = true;
            self.bytes.clear();
            return false;
        }
        self.bytes.extend_from_slice(bytes);
        let mut last_application = false;
        while self.bytes.len() >= 5 {
            if !matches!(self.bytes[0], 20..=23) || self.bytes[1] != 3 {
                self.failed = true;
                self.bytes.clear();
                break;
            }
            let size = u16::from_be_bytes([self.bytes[3], self.bytes[4]]) as usize;
            if size > 18432 {
                self.failed = true;
                self.bytes.clear();
                break;
            }
            if self.bytes.len() < size + 5 {
                return false;
            }
            self.seen_tls = true;
            let kind = self.bytes[0];
            last_application = kind == 23;
            if server && kind == 22 && !self.enabled && self.handshake.len() + size <= 65536 {
                self.handshake.extend_from_slice(&self.bytes[5..5 + size]);
                if let Some(enabled) = server_hello(&self.handshake) {
                    self.enabled = enabled;
                    self.handshake.clear();
                }
            }
            self.bytes.drain(..size + 5);
        }
        last_application && self.bytes.is_empty()
    }
}
fn server_hello(bytes: &[u8]) -> Option<bool> {
    if bytes.len() < 4 {
        return None;
    }
    let size = ((bytes[1] as usize) << 16) | ((bytes[2] as usize) << 8) | bytes[3] as usize;
    if size > 65532 {
        return Some(false);
    }
    if bytes.len() < size + 4 {
        return None;
    }
    if bytes[0] != 2 || size < 40 {
        return Some(false);
    }
    let b = &bytes[4..4 + size];
    let offset = 35usize.checked_add(*b.get(34)? as usize)?;
    let cipher = u16::from_be_bytes([*b.get(offset)?, *b.get(offset + 1)?]);
    if !matches!(cipher, 0x1301..=0x1304) {
        return Some(false);
    }
    let mut at = offset + 5;
    let end = at + u16::from_be_bytes([*b.get(offset + 3)?, *b.get(offset + 4)?]) as usize;
    if end > b.len() {
        return Some(false);
    }
    while at + 4 <= end {
        let key = u16::from_be_bytes([b[at], b[at + 1]]);
        let size = u16::from_be_bytes([b[at + 2], b[at + 3]]) as usize;
        at += 4;
        if at + size > end {
            return Some(false);
        }
        if key == 43 {
            return Some(size == 2 && b[at..at + size] == [3, 4]);
        }
        at += size;
    }
    Some(false)
}

pub(super) struct VisionStream<T> {
    tls: craft_tls::client::TlsStream<RecordIo<T>>,
    uuid: [u8; 16],
    response: Vec<u8>,
    response_left: Option<usize>,
    response_done: bool,
    encoded: Vec<u8>,
    first_read: bool,
    reading_padding: bool,
    raw_read: bool,
    ready: Vec<u8>,
    ready_at: usize,
    wire: Vec<u8>,
    wire_at: usize,
    write_command: u8,
    first_write: bool,
    writing_padding: bool,
    raw_write: bool,
    inbound: Records,
    outbound: Records,
    writes: usize,
    failed: bool,
}
impl<T: AsyncRead + AsyncWrite + Unpin> VisionStream<T> {
    pub fn new(tls: craft_tls::client::TlsStream<RecordIo<T>>, uuid: [u8; 16]) -> Self {
        Self {
            tls,
            uuid,
            response: Vec::new(),
            response_left: None,
            response_done: false,
            encoded: Vec::new(),
            first_read: true,
            reading_padding: true,
            raw_read: false,
            ready: Vec::new(),
            ready_at: 0,
            wire: Vec::new(),
            wire_at: 0,
            write_command: 0,
            first_write: true,
            writing_padding: true,
            raw_write: false,
            inbound: Records::default(),
            outbound: Records::default(),
            writes: 0,
            failed: false,
        }
    }
    fn drain_wire(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.wire_at < self.wire.len() {
            let n = std::task::ready!(
                Pin::new(&mut self.tls).poll_write(cx, &self.wire[self.wire_at..])
            )?;
            if n == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            self.wire_at += n;
        }
        if !self.wire.is_empty() && self.write_command == 2 {
            std::task::ready!(Pin::new(&mut self.tls).poll_flush(cx))?;
            self.raw_write = true;
        }
        self.wire.clear();
        self.wire_at = 0;
        Poll::Ready(Ok(()))
    }
    fn decode(&mut self) -> io::Result<bool> {
        if !self.reading_padding {
            self.ready = std::mem::take(&mut self.encoded);
            return Ok(!self.ready.is_empty());
        }
        let prefix = if self.first_read { 16 } else { 0 };
        if self.encoded.len() < prefix + 5 {
            return Ok(false);
        }
        if self.first_read && self.encoded[..16] != self.uuid {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid Vision UUID",
            ));
        }
        let command = self.encoded[prefix];
        if command > 2 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid Vision command",
            ));
        }
        let content =
            u16::from_be_bytes([self.encoded[prefix + 1], self.encoded[prefix + 2]]) as usize;
        let padding =
            u16::from_be_bytes([self.encoded[prefix + 3], self.encoded[prefix + 4]]) as usize;
        let size = prefix + 5 + content + padding;
        if self.encoded.len() < size {
            return Ok(false);
        }
        self.ready = self.encoded[prefix + 5..prefix + 5 + content].to_vec();
        self.inbound.feed(&self.ready, true);
        self.encoded.drain(..size);
        self.first_read = false;
        if command != 0 {
            self.reading_padding = false;
            self.ready.extend_from_slice(&self.encoded);
            self.encoded.clear();
            if command == 2 {
                if !self.tls.get_ref().0.at_boundary() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Vision direct transition inside outer TLS record",
                    ));
                }
                self.raw_read = true;
            }
        }
        Ok(!self.ready.is_empty())
    }
}
impl<T: AsyncRead + AsyncWrite + Unpin> AsyncRead for VisionStream<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        if this.failed {
            return Poll::Ready(Err(io::Error::other("Vision stream failed")));
        }
        let result = (|| {
            for _ in 0..64 {
                if this.ready_at < this.ready.len() {
                    let n = buf.remaining().min(this.ready.len() - this.ready_at);
                    buf.put_slice(&this.ready[this.ready_at..this.ready_at + n]);
                    this.ready_at += n;
                    return Poll::Ready(Ok(()));
                }
                this.ready.clear();
                this.ready_at = 0;
                if !this.response_done {
                    let mut bytes = [0; 255];
                    let need = this.response_left.unwrap_or(2 - this.response.len());
                    if need == 0 {
                        this.response_done = true;
                        continue;
                    }
                    let mut chunk = ReadBuf::new(&mut bytes[..need]);
                    std::task::ready!(Pin::new(&mut this.tls).poll_read(cx, &mut chunk))?;
                    if chunk.filled().is_empty() {
                        return Poll::Ready(Err(io::ErrorKind::UnexpectedEof.into()));
                    }
                    if let Some(left) = &mut this.response_left {
                        *left -= chunk.filled().len();
                    } else {
                        this.response.extend_from_slice(chunk.filled());
                        if this.response.len() == 2 {
                            if this.response[0] != 0 {
                                return Poll::Ready(Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "Invalid VLESS response",
                                )));
                            }
                            this.response_left = Some(this.response[1] as usize);
                        }
                    }
                    continue;
                }
                if this.raw_read {
                    use std::io::Read;
                    let mut bytes = [0; CHUNK];
                    let n = buf.remaining().min(CHUNK);
                    match this.tls.get_mut().1.reader().read(&mut bytes[..n]) {
                        Ok(n) if n > 0 => {
                            buf.put_slice(&bytes[..n]);
                            return Poll::Ready(Ok(()));
                        }
                        Err(e) if e.kind() != io::ErrorKind::WouldBlock => {
                            return Poll::Ready(Err(e));
                        }
                        _ => {}
                    }
                    return Pin::new(&mut this.tls.get_mut().0.io).poll_read(cx, buf);
                }
                let before = this.encoded.len();
                if this.decode()? {
                    continue;
                }
                if this.encoded.len() < before {
                    continue;
                }
                if this.raw_read {
                    continue;
                }
                if this.encoded.len() > MAX_FRAME {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Vision frame exceeds limit",
                    )));
                }
                let mut bytes = [0; 4096];
                let mut chunk = ReadBuf::new(&mut bytes);
                std::task::ready!(Pin::new(&mut this.tls).poll_read(cx, &mut chunk))?;
                if chunk.filled().is_empty() {
                    return if this.encoded.is_empty() {
                        Poll::Ready(Ok(()))
                    } else {
                        Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "Truncated Vision frame",
                        )))
                    };
                }
                this.encoded.extend_from_slice(chunk.filled());
            }
            cx.waker().wake_by_ref();
            Poll::Pending
        })();
        if matches!(result, Poll::Ready(Err(_))) {
            this.failed = true;
        }
        result
    }
}
impl<T: AsyncRead + AsyncWrite + Unpin> AsyncWrite for VisionStream<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.failed {
            return Poll::Ready(Err(io::Error::other("Vision stream failed")));
        }
        std::task::ready!(this.drain_wire(cx))?;
        if this.raw_write {
            return Pin::new(&mut this.tls.get_mut().0.io).poll_write(cx, buf);
        }
        if !this.writing_padding {
            return Pin::new(&mut this.tls).poll_write(cx, buf);
        }
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let n = buf.len().min(CHUNK);
        this.writes += 1;
        let application = this.outbound.feed(&buf[..n], false);
        let command = if application {
            if this.inbound.enabled { 2 } else { 1 }
        } else if this.writes >= 8 {
            1
        } else {
            0
        };
        let padding = if this.outbound.seen_tls && n < 900 {
            900 - n + (rand::random::<u8>() as usize)
        } else {
            rand::random::<u8>() as usize
        };
        if this.first_write {
            this.wire.extend_from_slice(&this.uuid);
            this.first_write = false;
        }
        this.wire.push(command);
        this.wire.extend_from_slice(&(n as u16).to_be_bytes());
        this.wire.extend_from_slice(&(padding as u16).to_be_bytes());
        this.wire.extend_from_slice(&buf[..n]);
        this.wire.resize(this.wire.len() + padding, 0);
        this.write_command = command;
        if command != 0 {
            this.writing_padding = false;
        }
        Poll::Ready(Ok(n))
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        std::task::ready!(this.drain_wire(cx))?;
        if this.raw_write {
            Pin::new(&mut this.tls.get_mut().0.io).poll_flush(cx)
        } else {
            Pin::new(&mut this.tls).poll_flush(cx)
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        std::task::ready!(this.drain_wire(cx))?;
        if this.raw_read || this.raw_write {
            if !this.raw_write {
                std::task::ready!(Pin::new(&mut this.tls).poll_flush(cx))?;
            }
            Pin::new(&mut this.tls.get_mut().0.io).poll_shutdown(cx)
        } else {
            Pin::new(&mut this.tls).poll_shutdown(cx)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    const ID: [u8; 16] = [7; 16];
    async fn pair() -> (
        VisionStream<tokio::io::DuplexStream>,
        tokio_rustls::server::TlsStream<tokio::io::DuplexStream>,
    ) {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let cert = rustls::pki_types::CertificateDer::from(
            include_bytes!("testdata/localhost-cert.der").to_vec(),
        );
        let key =
            rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
                include_bytes!("testdata/localhost-key.der").to_vec(),
            ));
        let mut config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .unwrap();
        config.send_tls13_tickets = 0;
        let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(config));
        let (client, server) = tokio::io::duplex(64);
        let task = tokio::spawn(async move { acceptor.accept(server).await.unwrap() });
        let config = crate::config::model::TlsConfig {
            sni: Some("localhost".into()),
            skip_cert_verify: true,
            alpn: None,
            fingerprint: None,
        };
        let connector = super::super::transport::build_tls_connector(Some(&config));
        let client = connector
            .connect(
                rustls_pki_types::ServerName::try_from("localhost").unwrap(),
                RecordIo::new(client),
            )
            .await
            .unwrap();
        (VisionStream::new(client, ID), task.await.unwrap())
    }
    fn frame(id: bool, command: u8, content: &[u8], padding: usize) -> Vec<u8> {
        let mut bytes = Vec::new();
        if id {
            bytes.extend_from_slice(&ID);
        }
        bytes.push(command);
        bytes.extend_from_slice(&(content.len() as u16).to_be_bytes());
        bytes.extend_from_slice(&(padding as u16).to_be_bytes());
        bytes.extend_from_slice(content);
        bytes.resize(bytes.len() + padding, 0);
        bytes
    }
    #[tokio::test]
    async fn fragmented_padding_multiple_blocks_and_end_keep_all_payload() {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (mut client, mut server) = pair().await;
            let writer = tokio::spawn(async move {
                server.write_all(&[0, 2, 0, 0]).await.unwrap();
                for byte in frame(true, 0, b"first", 31) {
                    server.write_all(&[byte]).await.unwrap();
                    server.flush().await.unwrap();
                }
                let mut next = frame(false, 0, b"", 0);
                next.extend(frame(false, 1, b"second", 0));
                next.extend(b"tail");
                server.write_all(&next).await.unwrap();
                server.flush().await.unwrap();
            });
            let mut bytes = [0; 15];
            client.read_exact(&mut bytes).await.unwrap();
            assert_eq!(&bytes, b"firstsecondtail");
            assert!(!client.raw_read);
            writer.await.unwrap();
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn direct_marker_preserves_decrypted_tail_and_coalesced_raw_tls_records() {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (mut client, mut server) = pair().await;
            let writer = tokio::spawn(async move {
                let mut wire = vec![0, 0];
                wire.extend(frame(true, 2, b"before", 3));
                wire.extend(b"tail");
                server.write_all(&wire).await.unwrap();
                server.flush().await.unwrap();
                server
                    .get_mut()
                    .0
                    .write_all(&[23, 3, 3, 0, 3, 9, 8, 7])
                    .await
                    .unwrap();
                server.get_mut().0.flush().await.unwrap();
            });
            let mut bytes = [0; 18];
            client.read_exact(&mut bytes).await.unwrap();
            assert_eq!(&bytes[..10], b"beforetail");
            assert_eq!(&bytes[10..], &[23, 3, 3, 0, 3, 9, 8, 7]);
            assert!(client.raw_read);
            writer.await.unwrap();
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn malformed_commands_and_truncated_frames_fail_closed() {
        for invalid in [
            frame(true, 9, b"bad", 0),
            frame(true, 0, b"truncated", 9)[..23].to_vec(),
        ] {
            let (mut client, mut server) = pair().await;
            let writer = tokio::spawn(async move {
                server.write_all(&[0, 0]).await.unwrap();
                server.write_all(&invalid).await.unwrap();
                server.shutdown().await.unwrap();
            });
            let mut bytes = [0; 64];
            assert!(client.read(&mut bytes).await.is_err());
            writer.await.unwrap();
        }
    }
    #[test]
    fn tls13_detection_handles_byte_fragments_and_rejects_tls12() {
        let mut body = vec![3, 3];
        body.extend([0; 32]);
        body.push(0);
        body.extend([0x13, 1, 0, 0, 6, 0, 43, 0, 2, 3, 4]);
        let mut handshake = vec![2, 0, 0, body.len() as u8];
        handshake.extend(body);
        let mut record = vec![22, 3, 3, 0, handshake.len() as u8];
        record.extend(handshake);
        let mut detector = Records::default();
        for byte in &record {
            detector.feed(&[*byte], true);
        }
        assert!(detector.enabled);
        *record.last_mut().unwrap() = 3;
        let mut detector = Records::default();
        detector.feed(&record, true);
        assert!(!detector.enabled);
    }
    #[tokio::test]
    async fn raw_downlink_half_close_flushes_pending_encrypted_uplink() {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let (mut client, mut server) = pair().await;
            client.raw_read = true;
            client.write_all(&[7; 4096]).await.unwrap();
            let reader = tokio::spawn(async move {
                let mut header = [0; 21];
                server.read_exact(&mut header).await.unwrap();
                assert_eq!(&header[..16], &ID);
                let size = u16::from_be_bytes([header[17], header[18]]) as usize;
                let padding = u16::from_be_bytes([header[19], header[20]]) as usize;
                let mut payload = vec![0; size];
                server.read_exact(&mut payload).await.unwrap();
                assert_eq!(payload, vec![7; 4096]);
                server.read_exact(&mut vec![0; padding]).await.unwrap();
            });
            client.shutdown().await.unwrap();
            assert!(!client.tls.get_ref().1.wants_write());
            reader.await.unwrap();
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn record_io_never_reads_ahead_and_rejects_truncated_record() {
        let (raw, mut peer) = tokio::io::duplex(64);
        let mut io = RecordIo::new(raw);
        peer.write_all(&[23, 3, 3, 0, 2, 1, 2, 23, 3, 3, 0, 1, 3])
            .await
            .unwrap();
        let mut bytes = [0; 64];
        assert_eq!(io.read(&mut bytes).await.unwrap(), 5);
        assert_eq!(io.read(&mut bytes).await.unwrap(), 2);
        assert!(io.at_boundary());
        assert_eq!(io.io.read(&mut bytes).await.unwrap(), 6);
        let (raw, mut peer) = tokio::io::duplex(64);
        let mut io = RecordIo::new(raw);
        peer.write_all(&[23, 3, 3, 0, 2, 1]).await.unwrap();
        peer.shutdown().await.unwrap();
        assert_eq!(io.read(&mut bytes).await.unwrap(), 5);
        assert_eq!(io.read(&mut bytes).await.unwrap(), 1);
        assert!(io.read(&mut bytes).await.is_err());
    }
}

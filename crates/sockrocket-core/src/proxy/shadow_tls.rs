//! Shadowsocks over shadow-tls v3 outbound.
//!
//! shadow-tls wraps a shadowsocks server behind a genuine-looking TLS 1.3
//! endpoint (SNI = plugin-opts.host). The client authenticates by signing
//! the ClientHello session id with HMAC-SHA1(password); the server proves
//! itself by splicing an HMAC into its first encrypted record. After the
//! TLS handshake the connection is dropped down to raw TCP and the
//! shadowsocks payload flows in fake TLS application-data frames with
//! rolling HMACs. Reference implementation:
//! <https://github.com/ihciah/shadow-tls> (src/client.rs, src/util.rs).

use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use hmac::{Hmac, Mac};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use shadowsocks::ProxyClientStream;
use shadowsocks::config::{ServerAddr, ServerConfig, ServerType};
use shadowsocks::context::{Context as SsContext, SharedContext};
use shadowsocks::crypto::CipherKind;
use shadowsocks::relay::Address;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

use craft_tls::rustls::ClientConfig;
use craft_tls::rustls::client::NoClientSessionStorage;
use craft_tls::rustls::craft::CHROME_112;
use craft_tls::rustls::shadow_tls::ShadowTlsConfig as CraftShadowTlsConfig;
use rustls_pki_types::ServerName;

use super::connector::{BoxProxyStream, Outbound};
use super::pool::{ConnFactory, ConnPool};
use super::transport::{InsecureVerifier, connect_tcp, tune_socket};

/// TLS record content types we care about.
const TLS_HANDSHAKE: u8 = 0x16;
const TLS_APPLICATION_DATA: u8 = 0x17;
const TLS_ALERT: u8 = 0x15;
/// Handshake message types.
const SERVER_HELLO: u8 = 0x02;
/// Sizes.
const TLS_HEADER_SIZE: usize = 5;
const HMAC_SIZE: usize = 4;
const TLS_HMAC_HEADER_SIZE: usize = TLS_HEADER_SIZE + HMAC_SIZE;
const TLS_RANDOM_SIZE: usize = 32;
/// Max payload per frame: the u16 length field covers HMAC + payload.
const MAX_FRAME_PAYLOAD: usize = u16::MAX as usize - HMAC_SIZE;
/// ServerHello.random offset within a TLS record (header + type + len + version).
const SERVER_RANDOM_IDX: usize = TLS_HEADER_SIZE + 1 + 3 + 2;
/// Extension type `supported_versions`.
const SUPPORTED_VERSIONS_TYPE: u16 = 43;
const TLS_13: u16 = 0x0304;
/// Frame record version bytes (0x0303 on the wire).
const FRAME_VERSION: [u8; 2] = [0x03, 0x03];

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

type HmacSha1 = Hmac<Sha1>;

/// HMAC-SHA1 keyed with the shadow-tls password.
fn hmac_password(password: &str, parts: &[&[u8]]) -> HmacSha1 {
    let mut mac = <HmacSha1 as Mac>::new_from_slice(password.as_bytes())
        .expect("HMAC accepts keys of any length");
    for part in parts {
        mac.update(part);
    }
    mac
}

/// First 4 bytes of the HMAC digest.
fn mac4(mac: &HmacSha1) -> [u8; HMAC_SIZE] {
    let bytes = mac.clone().finalize().into_bytes();
    let mut out = [0u8; HMAC_SIZE];
    out.copy_from_slice(&bytes[..HMAC_SIZE]);
    out
}

/// `SHA256(password || server_random)` — XOR key for the splice record.
fn kdf(password: &str, server_random: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    hasher.update(server_random);
    hasher.finalize().into()
}

/// Parse a ServerHello record and report whether it negotiates TLS 1.3.
fn support_tls13(record: &[u8]) -> bool {
    // Cursor into the record body after server_random: session_id (len-prefixed),
    // cipher_suite (2), compression (1), then extensions.
    let mut pos = SERVER_RANDOM_IDX + TLS_RANDOM_SIZE;
    if pos >= record.len() {
        return false;
    }
    let sid_len = record[pos] as usize;
    pos += 1 + sid_len + 3;
    if pos + 2 > record.len() {
        return false;
    }
    let ext_total = u16::from_be_bytes([record[pos], record[pos + 1]]) as usize;
    pos += 2;
    let end = (pos + ext_total).min(record.len());
    while pos + 4 <= end {
        let ext_type = u16::from_be_bytes([record[pos], record[pos + 1]]);
        let ext_len = u16::from_be_bytes([record[pos + 2], record[pos + 3]]) as usize;
        pos += 4;
        if pos + ext_len > end {
            return false;
        }
        if ext_type == SUPPORTED_VERSIONS_TYPE {
            return ext_len == 2 && u16::from_be_bytes([record[pos], record[pos + 1]]) == TLS_13;
        }
        pos += ext_len;
    }
    false
}

/// State extracted from the server's handshake flight.
struct HandshakeState {
    server_random: [u8; 32],
    /// `HMAC(password, server_random)` — verifies the spliced auth record.
    hmac_sr: HmacSha1,
    /// XOR key for the spliced auth record payload.
    key: [u8; 32],
    #[allow(dead_code)] // recorded at handshake time; reserved for v3 auth variants
    tls13: bool,
}

impl HandshakeState {
    fn new(password: &str, server_random: [u8; 32], tls13: bool) -> Self {
        let hmac_sr = hmac_password(password, &[&server_random]);
        let key = kdf(password, &server_random);
        Self {
            server_random,
            hmac_sr,
            key,
            tls13,
        }
    }
}

/// Read-side TLS record interceptor used during the TLS handshake.
///
/// rustls reads the server flight through this wrapper. It extracts the
/// server random from ServerHello and, on the server's first encrypted
/// record, verifies the shadow-tls splice HMAC, un-XORs the payload and
/// strips the 4 signature bytes so rustls sees a genuine TLS record.
struct HandshakeTap<S> {
    inner: S,
    password: Arc<str>,
    /// Cleaned record bytes waiting for rustls.
    pending: Vec<u8>,
    pending_pos: usize,
    /// Raw record assembly.
    hdr: [u8; TLS_HEADER_SIZE],
    hdr_pos: usize,
    body: Vec<u8>,
    body_pos: usize,
    state: Option<HandshakeState>,
    auth_checked: bool,
    authorized: bool,
}

impl<S> HandshakeTap<S> {
    fn new(inner: S, password: Arc<str>) -> Self {
        Self {
            inner,
            password,
            pending: Vec::new(),
            pending_pos: 0,
            hdr: [0u8; TLS_HEADER_SIZE],
            hdr_pos: 0,
            body: Vec::new(),
            body_pos: 0,
            state: None,
            auth_checked: false,
            authorized: false,
        }
    }

    fn state(&self) -> Option<&HandshakeState> {
        self.state.as_ref()
    }

    fn authorized(&self) -> bool {
        self.authorized
    }

    fn into_inner(self) -> S {
        self.inner
    }

    /// Inspect one complete record and queue the (possibly modified) bytes.
    fn process_record(&mut self, mut record: Vec<u8>) {
        match record[0] {
            TLS_HANDSHAKE
                if record.len() > SERVER_RANDOM_IDX + TLS_RANDOM_SIZE
                    && record[TLS_HEADER_SIZE] == SERVER_HELLO =>
            {
                let mut server_random = [0u8; TLS_RANDOM_SIZE];
                server_random.copy_from_slice(&record[SERVER_RANDOM_IDX..SERVER_RANDOM_IDX + 32]);
                let tls13 = support_tls13(&record);
                self.state = Some(HandshakeState::new(&self.password, server_random, tls13));
            }
            TLS_APPLICATION_DATA if self.state.is_some() && !self.auth_checked => {
                self.auth_checked = true;
                let state = self.state.as_ref().expect("checked above");
                if record.len() > TLS_HMAC_HEADER_SIZE {
                    let mut mac = state.hmac_sr.clone();
                    mac.update(&record[TLS_HMAC_HEADER_SIZE..]);
                    if mac4(&mac) == record[TLS_HEADER_SIZE..TLS_HMAC_HEADER_SIZE] {
                        // Un-XOR the payload with the cycling KDF key.
                        for (i, byte) in record[TLS_HMAC_HEADER_SIZE..].iter_mut().enumerate() {
                            *byte ^= state.key[i % state.key.len()];
                        }
                        // Strip the 4 signature bytes and fix the record length.
                        record.drain(TLS_HEADER_SIZE..TLS_HMAC_HEADER_SIZE);
                        let body_len = (record.len() - TLS_HEADER_SIZE) as u16;
                        record[3..TLS_HEADER_SIZE].copy_from_slice(&body_len.to_be_bytes());
                        self.authorized = true;
                    }
                }
            }
            _ => {}
        }
        self.pending = record;
        self.pending_pos = 0;
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for HandshakeTap<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = &mut *self;
        if this.pending_pos < this.pending.len() {
            let n = buf.remaining().min(this.pending.len() - this.pending_pos);
            buf.put_slice(&this.pending[this.pending_pos..this.pending_pos + n]);
            this.pending_pos += n;
            return Poll::Ready(Ok(()));
        }
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }

        loop {
            // hdr_pos == 0 marks the start of a NEW record; reset body
            // assembly progress there (and only there) so that a poll that
            // pended mid-body resumes instead of rewinding.
            if this.hdr_pos == 0 {
                this.body_pos = 0;
            }
            // Assemble one raw TLS record.
            while this.hdr_pos < TLS_HEADER_SIZE {
                let pos = this.hdr_pos;
                let mut read_buf = ReadBuf::new(&mut this.hdr[pos..]);
                match Pin::new(&mut this.inner).poll_read(cx, &mut read_buf) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                    Poll::Ready(Ok(())) => {
                        let n = read_buf.filled().len();
                        if n == 0 {
                            // Clean EOF between records.
                            return Poll::Ready(Ok(()));
                        }
                        this.hdr_pos += n;
                    }
                }
            }
            let body_len = u16::from_be_bytes([this.hdr[3], this.hdr[4]]) as usize;
            if this.body.len() != body_len {
                this.body.resize(body_len, 0);
            }
            while this.body_pos < body_len {
                let pos = this.body_pos;
                let mut read_buf = ReadBuf::new(&mut this.body[pos..]);
                match Pin::new(&mut this.inner).poll_read(cx, &mut read_buf) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                    Poll::Ready(Ok(())) => {
                        let n = read_buf.filled().len();
                        if n == 0 {
                            return Poll::Ready(Err(std::io::Error::new(
                                std::io::ErrorKind::UnexpectedEof,
                                "shadow-tls: EOF inside a TLS record",
                            )));
                        }
                        this.body_pos += n;
                    }
                }
            }

            this.hdr_pos = 0;
            let mut record = Vec::with_capacity(TLS_HEADER_SIZE + body_len);
            record.extend_from_slice(&this.hdr);
            record.extend_from_slice(&this.body);
            this.process_record(record);

            if this.pending_pos < this.pending.len() {
                let n = buf.remaining().min(this.pending.len() - this.pending_pos);
                buf.put_slice(&this.pending[this.pending_pos..this.pending_pos + n]);
                this.pending_pos += n;
                return Poll::Ready(Ok(()));
            }
            // The record vanished entirely (should not happen); read the next.
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for HandshakeTap<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Which HMAC direction a [`ShadowTlsIo`] frame codec uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // `Server` is only constructed from tests
enum FrameRole {
    /// Client side: writes "C" HMACs, verifies "S" HMACs.
    Client,
    /// Server side (used in tests): writes "S" HMACs, verifies "C" HMACs.
    Server,
}

impl FrameRole {
    fn write_label(self) -> &'static [u8] {
        match self {
            FrameRole::Client => b"C",
            FrameRole::Server => b"S",
        }
    }

    fn read_label(self) -> &'static [u8] {
        match self {
            FrameRole::Client => b"S",
            FrameRole::Server => b"C",
        }
    }
}

/// Post-handshake frame codec: shadowsocks payload wrapped in fake TLS
/// application-data records with rolling 4-byte HMAC-SHA1 tags.
struct ShadowTlsIo<S> {
    inner: S,
    #[allow(dead_code)] // HMAC labels are symmetric post-handshake; role kept for clarity
    role: FrameRole,
    /// Verified plaintext of the current frame.
    payload: Vec<u8>,
    payload_pos: usize,
    /// Incoming frame assembly.
    hdr: [u8; TLS_HEADER_SIZE],
    hdr_pos: usize,
    body: Vec<u8>,
    body_pos: usize,
    /// Rolling HMAC over all outgoing plaintext plus per-frame tags.
    write_hmac: HmacSha1,
    /// Rolling HMAC over all incoming plaintext plus per-frame tags.
    read_hmac: HmacSha1,
    /// `HMAC(password, server_random)`: frames verifying against this are
    /// skipped (server-side control data) until the first mismatch.
    ignore_hmac: Option<HmacSha1>,
    /// Outgoing frame being drained into the socket.
    write_pending: Vec<u8>,
    write_pos: usize,
    /// Set once a TLS alert record signalled the end of the stream.
    saw_alert: bool,
}

impl<S> ShadowTlsIo<S> {
    fn new(inner: S, password: &str, server_random: &[u8; 32], role: FrameRole) -> Self {
        Self {
            inner,
            role,
            payload: Vec::new(),
            payload_pos: 0,
            hdr: [0u8; TLS_HEADER_SIZE],
            hdr_pos: 0,
            body: Vec::new(),
            body_pos: 0,
            write_hmac: hmac_password(password, &[server_random, role.write_label()]),
            read_hmac: hmac_password(password, &[server_random, role.read_label()]),
            ignore_hmac: Some(hmac_password(password, &[server_random])),
            write_pending: Vec::new(),
            write_pos: 0,
            saw_alert: false,
        }
    }

    /// Wrap `payload` in one or more frames and queue them for writing.
    ///
    /// A frame length field is u16 covering HMAC + payload, so payloads
    /// larger than [`MAX_FRAME_PAYLOAD`] are split into multiple frames;
    /// the rolling HMAC chains across all of them.
    fn queue_frame(&mut self, payload: &[u8]) {
        self.write_pending.clear();
        for chunk in payload.chunks(MAX_FRAME_PAYLOAD) {
            self.write_hmac.update(chunk);
            let tag = mac4(&self.write_hmac);
            // Feed the tag back into the rolling HMAC (reference: `hmac.update(&hmac_val)`).
            self.write_hmac.update(&tag);

            self.write_pending
                .reserve(TLS_HMAC_HEADER_SIZE + chunk.len());
            self.write_pending.push(TLS_APPLICATION_DATA);
            self.write_pending.extend_from_slice(&FRAME_VERSION);
            self.write_pending
                .extend_from_slice(&((HMAC_SIZE + chunk.len()) as u16).to_be_bytes());
            self.write_pending.extend_from_slice(&tag);
            self.write_pending.extend_from_slice(chunk);
        }
        self.write_pos = 0;
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for ShadowTlsIo<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        loop {
            let this = &mut *self;
            if this.payload_pos < this.payload.len() {
                let n = buf.remaining().min(this.payload.len() - this.payload_pos);
                buf.put_slice(&this.payload[this.payload_pos..this.payload_pos + n]);
                this.payload_pos += n;
                return Poll::Ready(Ok(()));
            }
            if buf.remaining() == 0 || this.saw_alert {
                return Poll::Ready(Ok(()));
            }

            // hdr_pos == 0 marks the start of a NEW frame; reset body
            // assembly progress there (and only there) so that a poll that
            // pended mid-body resumes instead of rewinding. Clearing keeps
            // the capacity for reuse — the body buffer ping-pongs with the
            // payload buffer (see below) and is refilled via spare capacity.
            if this.hdr_pos == 0 {
                this.body_pos = 0;
                this.body.clear();
            }
            // Assemble one frame.
            while this.hdr_pos < TLS_HEADER_SIZE {
                let pos = this.hdr_pos;
                let mut read_buf = ReadBuf::new(&mut this.hdr[pos..]);
                match Pin::new(&mut this.inner).poll_read(cx, &mut read_buf) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                    Poll::Ready(Ok(())) => {
                        let n = read_buf.filled().len();
                        if n == 0 {
                            this.saw_alert = true;
                            return Poll::Ready(Ok(()));
                        }
                        this.hdr_pos += n;
                    }
                }
            }
            let body_len = u16::from_be_bytes([this.hdr[3], this.hdr[4]]) as usize;
            if body_len < HMAC_SIZE {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "shadow-tls: frame shorter than its HMAC",
                )));
            }
            // Fill through uninitialized spare capacity (same pattern as the
            // VMess pump tasks): a `resize(body_len, 0)` here would memset
            // the whole frame right before the reads overwrite it.
            this.body.reserve(body_len - this.body_pos);
            while this.body_pos < body_len {
                let pos = this.body_pos;
                let n = {
                    let chunk = &mut this.body.spare_capacity_mut()[..body_len - pos];
                    let mut read_buf = ReadBuf::uninit(chunk);
                    match Pin::new(&mut this.inner).poll_read(cx, &mut read_buf) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                        Poll::Ready(Ok(())) => read_buf.filled().len(),
                    }
                };
                if n == 0 {
                    return Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "shadow-tls: EOF inside a frame",
                    )));
                }
                // SAFETY: the ReadBuf above just initialized these `n` bytes
                // of spare capacity; `body.len()` always equals `body_pos`.
                unsafe { this.body.set_len(pos + n) };
                this.body_pos = pos + n;
            }
            this.hdr_pos = 0;

            let content_type = this.hdr[0];
            if content_type == TLS_ALERT {
                this.saw_alert = true;
                return Poll::Ready(Ok(()));
            }
            if content_type != TLS_APPLICATION_DATA
                || this.hdr[1] != FRAME_VERSION[0]
                || this.hdr[2] != FRAME_VERSION[1]
            {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("shadow-tls: unexpected frame content type {content_type:#x}"),
                )));
            }

            let (tag, data) = this.body.split_at(HMAC_SIZE);

            // Skip server control frames while they still verify against the
            // plain server-random HMAC.
            if let Some(ignore) = this.ignore_hmac.as_ref() {
                let mut mac = ignore.clone();
                mac.update(data);
                if mac4(&mac) == tag {
                    continue;
                }
                this.ignore_hmac = None;
            }

            this.read_hmac.update(data);
            let expected = mac4(&this.read_hmac);
            this.read_hmac.update(&expected);
            if expected != tag {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "shadow-tls: frame HMAC mismatch",
                )));
            }

            // Move the verified frame into the payload slot instead of
            // copying it out; the old payload allocation becomes the next
            // frame's body buffer, so the two buffers ping-pong without
            // allocating or copying in the steady state. `payload_pos`
            // starts past the HMAC tag.
            let frame = std::mem::take(&mut this.body);
            let old_payload = std::mem::replace(&mut this.payload, frame);
            this.body = old_payload;
            this.payload_pos = HMAC_SIZE;
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for ShadowTlsIo<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = &mut *self;
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        if this.write_pos == this.write_pending.len() {
            this.queue_frame(buf);
        }
        while this.write_pos < this.write_pending.len() {
            let pos = this.write_pos;
            match Pin::new(&mut this.inner).poll_write(cx, &this.write_pending[pos..]) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Ready(Ok(0)) => {
                    return Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "shadow-tls: failed to write frame",
                    )));
                }
                Poll::Ready(Ok(n)) => this.write_pos += n,
            }
        }
        let written = buf.len();
        this.write_pending.clear();
        this.write_pos = 0;
        Poll::Ready(Ok(written))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = &mut *self;
        while this.write_pos < this.write_pending.len() {
            let pos = this.write_pos;
            match Pin::new(&mut this.inner).poll_write(cx, &this.write_pending[pos..]) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Ready(Ok(0)) => {
                    return Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "shadow-tls: failed to flush frame",
                    )));
                }
                Poll::Ready(Ok(n)) => this.write_pos += n,
            }
        }
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.as_mut().poll_flush(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Ready(Ok(())) => {}
        }
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Factory that creates pooled shadow-tls relay connections.
///
/// Each connection is a completed TCP connect + TLS 1.3 handshake + splice
/// verification dropped down to the framed relay — an idle, unused stream,
/// so a pooled connection is directly reusable by a later `connect()` (the
/// splice/HMAC state machine starts fresh per connection and no bytes have
/// flowed yet). The pool's 120s stale eviction bounds how long an idle
/// connection is kept.
struct ShadowTlsConnFactory {
    server: String,
    port: u16,
    tls_host: String,
    tls_password: Arc<str>,
    connector: craft_tls::TlsConnector,
}

impl ShadowTlsConnFactory {
    /// TCP connect + shadow-tls handshake + drop to the framed relay.
    async fn open_relay(&self) -> Result<ShadowTlsIo<TcpStream>> {
        let addr = super::transport::format_host_port(&self.server, self.port);
        let tcp = connect_tcp(&addr, CONNECT_TIMEOUT).await?;
        tune_socket(&tcp);

        let tap = HandshakeTap::new(tcp, self.tls_password.clone());
        let server_name = ServerName::try_from(self.tls_host.clone())
            .with_context(|| format!("shadow-tls: invalid TLS host '{}'", self.tls_host))?;
        let tls = tokio::time::timeout(CONNECT_TIMEOUT, self.connector.connect(server_name, tap))
            .await
            .map_err(|_| anyhow::anyhow!("shadow-tls handshake with {} timed out", addr))?
            .with_context(|| format!("shadow-tls TLS handshake with {} failed", addr))?;

        let (tap, conn) = tls.into_inner();
        drop(conn);

        if !tap.authorized() {
            bail!(
                "shadow-tls authentication failed (server {} rejected the session id signature)",
                addr
            );
        }
        let server_random = tap
            .state()
            .expect("authorized implies handshake state")
            .server_random;
        Ok(ShadowTlsIo::new(
            tap.into_inner(),
            &self.tls_password,
            &server_random,
            FrameRole::Client,
        ))
    }
}

impl ConnFactory for ShadowTlsConnFactory {
    fn create(&self) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
        Box::pin(async move {
            let relay = self.open_relay().await?;
            Ok(Box::new(relay) as BoxProxyStream)
        })
    }
}

/// Shadowsocks + shadow-tls v3 outbound connector.
pub struct ShadowTlsOutbound {
    server: String,
    port: u16,
    server_config: ServerConfig,
    context: SharedContext,
    pool: ConnPool,
}

impl ShadowTlsOutbound {
    pub fn new(
        server: &str,
        port: u16,
        cipher: &str,
        password: &str,
        tls_host: &str,
        tls_password: &str,
        tls_skip_verify: bool,
    ) -> Result<Self> {
        Self::build(
            server,
            port,
            cipher,
            password,
            (tls_host, tls_password, tls_skip_verify),
            true,
        )
    }

    /// Probe/latency variant: the connection pool never warms in the
    /// background, so a one-shot latency test doesn't pre-connect a full
    /// pool of TLS handshakes at the server.
    pub(crate) fn new_unwarmed(
        server: &str,
        port: u16,
        cipher: &str,
        password: &str,
        tls_host: &str,
        tls_password: &str,
        tls_skip_verify: bool,
    ) -> Result<Self> {
        Self::build(
            server,
            port,
            cipher,
            password,
            (tls_host, tls_password, tls_skip_verify),
            false,
        )
    }

    /// `tls` is (handshake host/SNI, shadow-tls password, skip cert verify).
    fn build(
        server: &str,
        port: u16,
        cipher: &str,
        password: &str,
        tls: (&str, &str, bool),
        warm_pool: bool,
    ) -> Result<Self> {
        let (tls_host, tls_password, tls_skip_verify) = tls;
        let method = CipherKind::from_str(cipher)
            .map_err(|_| anyhow::anyhow!("Unknown SS cipher: {}", cipher))?;
        let server_addr: ServerAddr = (server.to_string(), port).into();
        let server_config = ServerConfig::new(server_addr, password, method)
            .map_err(|e| anyhow::anyhow!("Invalid SS server config: {}", e))?;

        let base_config = if tls_skip_verify {
            ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(InsecureVerifier))
                .with_no_client_auth()
        } else {
            ClientConfig::builder()
                .with_root_certificates(super::transport::get_root_cert_store().clone())
                .with_no_client_auth()
        };

        let mut tls_config = base_config.with_fingerprint(CHROME_112.builder());
        tls_config.shadow_tls = Some(Arc::new(CraftShadowTlsConfig {
            password: Arc::from(tls_password),
        }));
        // The signed session id must be fresh per handshake; a resumed session
        // would also send a PSK binder the shadow-tls server cannot answer.
        tls_config.resumption.store = Arc::new(NoClientSessionStorage);

        let factory = Arc::new(ShadowTlsConnFactory {
            server: server.to_string(),
            port,
            tls_host: tls_host.to_string(),
            tls_password: Arc::from(tls_password),
            connector: craft_tls::TlsConnector::from(Arc::new(tls_config)),
        });
        let pool = if warm_pool {
            ConnPool::new(factory)
        } else {
            ConnPool::lazy(factory)
        };

        Ok(Self {
            server: server.to_string(),
            port,
            server_config,
            context: SsContext::new_shared(ServerType::Local),
            pool,
        })
    }
}

impl Outbound for ShadowTlsOutbound {
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
        let addr = Address::DomainNameAddress(host.to_string(), port);
        let target_host = host.to_string();
        Box::pin(async move {
            let relay = self.pool.get().await.with_context(|| {
                format!(
                    "shadow-tls: failed to connect to {}:{} (target: {}:{})",
                    self.server, self.port, target_host, port
                )
            })?;
            let stream = ProxyClientStream::from_stream(
                self.context.clone(),
                relay,
                &self.server_config,
                addr,
            );
            Ok(Box::new(stream) as BoxProxyStream)
        })
    }

    fn name(&self) -> &str {
        "shadowsocks+shadow-tls"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _, duplex};

    /// Build a canned ServerHello record with a given random and a
    /// supported_versions extension negotiating TLS 1.3 (or absent).
    fn server_hello_record(server_random: [u8; 32], with_tls13_ext: bool) -> Vec<u8> {
        let mut body = Vec::new();
        body.push(SERVER_HELLO);
        body.extend_from_slice(&[0, 0, 0]); // length placeholder
        body.extend_from_slice(&[0x03, 0x03]); // server version
        body.extend_from_slice(&server_random);
        body.push(0); // empty session id echo
        body.extend_from_slice(&[0x13, 0x01]); // cipher suite
        body.push(0); // compression
        let mut exts = Vec::new();
        if with_tls13_ext {
            exts.extend_from_slice(&SUPPORTED_VERSIONS_TYPE.to_be_bytes());
            exts.extend_from_slice(&2u16.to_be_bytes());
            exts.extend_from_slice(&TLS_13.to_be_bytes());
        }
        body.extend_from_slice(&(exts.len() as u16).to_be_bytes());
        body.extend_from_slice(&exts);
        let len = (body.len() - 4) as u32;
        body[1..4].copy_from_slice(&len.to_be_bytes()[1..]);

        let mut record = Vec::new();
        record.push(TLS_HANDSHAKE);
        record.extend_from_slice(&FRAME_VERSION);
        record.extend_from_slice(&(body.len() as u16).to_be_bytes());
        record.extend_from_slice(&body);
        record
    }

    /// Build the spliced first-encrypted-record as the shadow-tls server
    /// would: [hdr][hmac4][XOR(payload)].
    fn spliced_record(password: &str, server_random: &[u8; 32], payload: &[u8]) -> Vec<u8> {
        let key = kdf(password, server_random);
        let mut body = Vec::with_capacity(HMAC_SIZE + payload.len());
        let mut mac = hmac_password(password, &[server_random]);
        let mut xored = payload.to_vec();
        for (i, b) in xored.iter_mut().enumerate() {
            *b ^= key[i % key.len()];
        }
        mac.update(&xored);
        body.extend_from_slice(&mac4(&mac));
        body.extend_from_slice(&xored);

        let mut record = Vec::new();
        record.push(TLS_APPLICATION_DATA);
        record.extend_from_slice(&FRAME_VERSION);
        record.extend_from_slice(&(body.len() as u16).to_be_bytes());
        record.extend_from_slice(&body);
        record
    }

    #[test]
    fn support_tls13_detection() {
        let sr = [7u8; 32];
        let rec = server_hello_record(sr, true);
        assert!(support_tls13(&rec));
        let rec = server_hello_record(sr, false);
        assert!(!support_tls13(&rec));
    }

    #[test]
    fn kdf_matches_reference() {
        // SHA256(password || server_random)
        let mut hasher = Sha256::new();
        hasher.update(b"10086");
        hasher.update([9u8; 32]);
        let expected: [u8; 32] = hasher.finalize().into();
        assert_eq!(kdf("10086", &[9u8; 32]), expected);
    }

    #[tokio::test]
    async fn tap_extracts_random_and_unwraps_splice() {
        let (client, mut server_side) = duplex(64 * 1024);
        let password = "10086";
        let sr = [42u8; 32];
        let payload = b"encrypted-flight-record";

        let sh = server_hello_record(sr, true);
        let splice = spliced_record(password, &sr, payload);

        let feeder = tokio::spawn({
            let sh = sh.clone();
            let splice = splice.clone();
            async move {
                server_side.write_all(&sh).await.unwrap();
                server_side.write_all(&splice).await.unwrap();
                server_side.flush().await.unwrap();
            }
        });

        let mut tap = HandshakeTap::new(client, Arc::from(password));
        // First read: ServerHello passes through untouched.
        let mut first = vec![0u8; sh.len()];
        tap.read_exact(&mut first).await.unwrap();
        assert_eq!(first, sh);
        // Second read: the spliced record, cleaned.
        let mut cleaned = vec![0u8; splice.len() - HMAC_SIZE];
        tap.read_exact(&mut cleaned).await.unwrap();

        feeder.await.unwrap();

        let state = tap.state().expect("state extracted");
        assert_eq!(state.server_random, sr);
        assert!(state.tls13);
        assert!(tap.authorized());

        // The cleaned record = record header with length shrunk by 4 +
        // the un-XORed payload.
        assert_eq!(cleaned[0], TLS_APPLICATION_DATA);
        let cleaned_len = u16::from_be_bytes([cleaned[3], cleaned[4]]) as usize;
        assert_eq!(cleaned_len, payload.len());
        assert_eq!(&cleaned[TLS_HEADER_SIZE..], payload);
    }

    #[tokio::test]
    async fn frame_codec_round_trip() {
        let (a, b) = duplex(64 * 1024);
        let password = "p@ss";
        let sr = [3u8; 32];
        let mut client_io = ShadowTlsIo::new(a, password, &sr, FrameRole::Client);
        let mut server_io = ShadowTlsIo::new(b, password, &sr, FrameRole::Server);

        let payloads: Vec<Vec<u8>> = vec![
            b"first".to_vec(),
            vec![0xAB; 1000],
            b"third frame payload".to_vec(),
        ];

        let writer = tokio::spawn({
            let payloads = payloads.clone();
            async move {
                for p in &payloads {
                    client_io.write_all(p).await.unwrap();
                }
                client_io.shutdown().await.unwrap();
            }
        });

        let mut received = Vec::new();
        server_io.read_to_end(&mut received).await.unwrap();
        writer.await.unwrap();

        let expected: Vec<u8> = payloads.concat();
        assert_eq!(received, expected);
    }

    #[tokio::test]
    async fn frame_codec_rejects_tampering() {
        let sr = [3u8; 32];
        // A writer with the WRONG password produces frames that must fail
        // the reader's HMAC verification.
        let (raw_client, b_server) = duplex(64 * 1024);
        let mut raw_io = ShadowTlsIo::new(raw_client, "wrong", &sr, FrameRole::Client);
        let send = tokio::spawn(async move {
            raw_io.write_all(b"tampered").await.unwrap();
        });

        let mut server_io = ShadowTlsIo::new(b_server, "right", &sr, FrameRole::Server);
        let mut buf = Vec::new();
        let result = server_io.read_to_end(&mut buf).await;
        send.await.unwrap();
        assert!(result.is_err());
    }

    /// Payloads larger than one frame's u16 length field (65531 bytes of
    /// payload) must be split into multiple frames, not truncated.
    #[tokio::test]
    async fn frame_codec_splits_oversized_payload() {
        let (a, b) = duplex(64 * 1024);
        let password = "p@ss";
        let sr = [5u8; 32];
        let mut client_io = ShadowTlsIo::new(a, password, &sr, FrameRole::Client);
        let mut server_io = ShadowTlsIo::new(b, password, &sr, FrameRole::Server);

        let payload = vec![0xAB; MAX_FRAME_PAYLOAD * 2 + 12345];

        let writer = tokio::spawn({
            let payload = payload.clone();
            async move {
                client_io.write_all(&payload).await.unwrap();
                client_io.shutdown().await.unwrap();
            }
        });

        let mut received = Vec::new();
        server_io.read_to_end(&mut received).await.unwrap();
        writer.await.unwrap();

        assert_eq!(received, payload);
    }

    /// Regression: consecutive frames with the SAME body length must each
    /// reset `body_pos`, otherwise the reader reuses stale body bytes and
    /// reports a bogus HMAC mismatch. Bulk SS traffic produces many
    /// equal-length frames in a row, so this is the common case.
    ///
    /// The duplex capacity (32) is smaller than one frame, which also forces
    /// every body read to pend mid-frame and resume — regressing THAT must
    /// not rewind `body_pos` either.
    #[tokio::test]
    async fn frame_codec_equal_length_frames() {
        let (a, b) = duplex(32);
        let password = "p@ss";
        let sr = [9u8; 32];
        let mut client_io = ShadowTlsIo::new(a, password, &sr, FrameRole::Client);
        let mut server_io = ShadowTlsIo::new(b, password, &sr, FrameRole::Server);

        let payloads: Vec<Vec<u8>> = (0..5)
            .map(|i| vec![0xA0 + i as u8; 64]) // identical lengths, distinct content
            .collect();

        let writer = tokio::spawn({
            let payloads = payloads.clone();
            async move {
                for p in &payloads {
                    client_io.write_all(p).await.unwrap();
                }
                client_io.shutdown().await.unwrap();
            }
        });

        let mut received = Vec::new();
        server_io.read_to_end(&mut received).await.unwrap();
        writer.await.unwrap();

        assert_eq!(received, payloads.concat());
    }
}

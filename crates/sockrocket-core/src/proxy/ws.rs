//! WebSocket (RFC 6455) client transport layer.
//!
//! Sits between the TLS/TCP transport and the proxy protocol: after the
//! HTTP Upgrade handshake completes, all protocol bytes (VMess/VLESS/Trojan
//! headers and payload) are carried inside WS binary frames. Frames are a
//! message boundary, the protocols are byte streams — reads forward each
//! frame payload in order (reassembling fragmented messages naturally),
//! writes wrap each caller chunk into one masked binary frame.
//!
//! Implemented by hand to avoid pulling in tokio-tungstenite (which would
//! re-introduce a TLS stack we don't need — TLS is handled one layer below
//! by craft-tls).

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use anyhow::{Context as _, Result, ensure};
use base64::Engine as _;
use rand::Rng as _;
use sha1::{Digest as _, Sha1};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

use crate::config::model::WsConfig;

use super::connector::BoxProxyStream;

/// RFC 6455 magic GUID for Sec-WebSocket-Accept computation.
const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

const OPCODE_CONT: u8 = 0x0;
const OPCODE_TEXT: u8 = 0x1;
const OPCODE_BINARY: u8 = 0x2;
const OPCODE_CLOSE: u8 = 0x8;
const OPCODE_PING: u8 = 0x9;
const OPCODE_PONG: u8 = 0xA;

/// Upper bound on a single frame payload (protects against OOM from a
/// hostile or broken peer announcing a huge length).
const MAX_FRAME_PAYLOAD: u64 = 64 * 1024 * 1024;
/// Upper bound on the HTTP Upgrade response header block.
const MAX_HANDSHAKE_RESPONSE: usize = 8192;
/// Max caller bytes taken per `poll_write`; each chunk becomes one frame
/// (the old write task's read-chunk size).
const WRITE_BUF_SIZE: usize = 16384;

/// Compute the expected Sec-WebSocket-Accept value for a client key:
/// base64(SHA-1(key + GUID)).
fn compute_accept_key(key: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(WS_GUID.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(hasher.finalize())
}

/// Normalize the WS request path: default "/" and ensure a leading slash.
fn normalize_path(path: Option<&str>) -> String {
    match path {
        Some(p) if !p.is_empty() => {
            if p.starts_with('/') {
                p.to_string()
            } else {
                format!("/{}", p)
            }
        }
        _ => "/".to_string(),
    }
}

/// Headers the handshake sets itself; duplicates from the user's custom
/// header map would corrupt the request and are skipped.
fn is_reserved_header(name: &str) -> bool {
    [
        "host",
        "upgrade",
        "connection",
        "sec-websocket-key",
        "sec-websocket-version",
    ]
    .iter()
    .any(|h| name.eq_ignore_ascii_case(h))
}

/// Build the HTTP Upgrade request. Returns the request bytes and the
/// Sec-WebSocket-Key (needed to validate the response).
fn build_handshake_request(
    host: &str,
    path: &str,
    headers: Option<&std::collections::HashMap<String, String>>,
) -> (Vec<u8>, String) {
    let key_bytes: [u8; 16] = rand::rng().random();
    let key = base64::engine::general_purpose::STANDARD.encode(key_bytes);

    let mut req = String::with_capacity(256);
    req.push_str("GET ");
    req.push_str(path);
    req.push_str(" HTTP/1.1\r\n");
    req.push_str("Host: ");
    req.push_str(host);
    req.push_str("\r\n");
    req.push_str("Upgrade: websocket\r\n");
    req.push_str("Connection: Upgrade\r\n");
    req.push_str("Sec-WebSocket-Key: ");
    req.push_str(&key);
    req.push_str("\r\n");
    req.push_str("Sec-WebSocket-Version: 13\r\n");
    if let Some(headers) = headers {
        for (name, value) in headers {
            if is_reserved_header(name) {
                continue;
            }
            if name.contains(['\r', '\n']) || value.contains(['\r', '\n']) {
                tracing::warn!("WS header {:?} contains CR/LF, skipped", name);
                continue;
            }
            req.push_str(name);
            req.push_str(": ");
            req.push_str(value);
            req.push_str("\r\n");
        }
    }
    req.push_str("\r\n");
    (req.into_bytes(), key)
}

/// Validate the server's 101 response head (everything up to the blank
/// line, CRLF-separated, without the trailing empty line).
fn validate_handshake_response(head: &str, key: &str) -> Result<()> {
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap_or_default();
    let code = status.split_whitespace().nth(1).unwrap_or_default();
    ensure!(
        status.starts_with("HTTP/") && code == "101",
        "WebSocket upgrade rejected: {}",
        status
    );
    let mut accept = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("sec-websocket-accept")
        {
            accept = Some(value.trim().to_string());
        }
    }
    let accept = accept.context("WebSocket response missing Sec-WebSocket-Accept")?;
    let expected = compute_accept_key(key);
    ensure!(
        accept == expected,
        "WebSocket Sec-WebSocket-Accept mismatch (got {}, expected {})",
        accept,
        expected
    );
    Ok(())
}

/// Find the end of the HTTP header block; returns the index of the first
/// byte of the "\r\n\r\n" terminator.
fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Encode a single frame. `mask_key: Some(..)` masks the payload (mandatory
/// client→server per RFC 6455), `None` sends it plain (server→client).
///
/// Test-only helper: the production write path is [`encode_client_frame_into`],
/// which frames without allocating.
#[cfg(test)]
fn encode_frame_with_key(opcode: u8, payload: &[u8], mask_key: Option<[u8; 4]>) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 14);
    out.push(0x80 | opcode); // FIN always set: one write = one message
    let mask_bit = if mask_key.is_some() { 0x80 } else { 0x00 };
    let len = payload.len();
    if len < 126 {
        out.push(mask_bit | len as u8);
    } else if len <= 0xFFFF {
        out.push(mask_bit | 126);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(mask_bit | 127);
        out.extend_from_slice(&(len as u64).to_be_bytes());
    }
    match mask_key {
        Some(key) => {
            out.extend_from_slice(&key);
            out.extend(payload.iter().enumerate().map(|(i, b)| b ^ key[i % 4]));
        }
        None => out.extend_from_slice(payload),
    }
    out
}

/// Encode a client frame with a fresh random mask key.
#[cfg(test)]
fn encode_client_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    encode_frame_with_key(opcode, payload, Some(rand::rng().random()))
}

/// Read exactly `buf.len()` bytes, draining the handshake leftover buffer
/// first, then the reader.
#[cfg(test)]
async fn read_exact_prefixed<R: AsyncRead + Unpin>(
    reader: &mut R,
    pending: &mut Vec<u8>,
    buf: &mut [u8],
) -> io::Result<()> {
    let mut filled = 0;
    if !pending.is_empty() {
        let n = pending.len().min(buf.len());
        buf[..n].copy_from_slice(&pending[..n]);
        pending.drain(..n);
        filled = n;
    }
    reader.read_exact(&mut buf[filled..]).await?;
    Ok(())
}

/// Read and decode one WS frame into `payload`, returning the opcode.
/// `fin` is parsed but not surfaced: for byte-stream semantics each
/// fragment's payload is forwarded in arrival order, which transparently
/// reassembles fragmented messages. Masked frames are unmasked
/// transparently (RFC forbids server→client masking, but tolerating it is
/// harmless).
///
/// `payload` is cleared and refilled each call; its allocation is reused
/// across frames by the caller.
///
/// Test-only helper (the fake servers below speak WS back to the client);
/// the production read path is [`WsStream`]'s poll state machine.
#[cfg(test)]
async fn read_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
    pending: &mut Vec<u8>,
    payload: &mut Vec<u8>,
) -> Result<u8> {
    let mut hdr = [0u8; 2];
    read_exact_prefixed(reader, pending, &mut hdr).await?;
    let opcode = hdr[0] & 0x0f;
    let masked = hdr[1] & 0x80 != 0;
    let mut len = (hdr[1] & 0x7f) as u64;
    if len == 126 {
        let mut b = [0u8; 2];
        read_exact_prefixed(reader, pending, &mut b).await?;
        len = u16::from_be_bytes(b) as u64;
    } else if len == 127 {
        let mut b = [0u8; 8];
        read_exact_prefixed(reader, pending, &mut b).await?;
        len = u64::from_be_bytes(b);
    }
    ensure!(
        len <= MAX_FRAME_PAYLOAD,
        "WS frame payload too large: {} bytes",
        len
    );
    let mask_key = if masked {
        let mut key = [0u8; 4];
        read_exact_prefixed(reader, pending, &mut key).await?;
        Some(key)
    } else {
        None
    };
    let len = len as usize;
    payload.clear();
    payload.reserve(len);
    // Handshake leftover bytes first, then the reader. The Vec allocation is
    // reused across frames and filled through uninitialized spare capacity
    // (same pattern as the VMess pump tasks): the old `vec![0u8; len]` both
    // reallocated and zero-filled the whole frame before overwriting it.
    if !pending.is_empty() {
        let n = pending.len().min(len);
        payload.extend_from_slice(&pending[..n]);
        pending.drain(..n);
    }
    while payload.len() < len {
        let n = {
            let filled = payload.len();
            let chunk = &mut payload.spare_capacity_mut()[..len - filled];
            let mut rb = ReadBuf::uninit(chunk);
            std::future::poll_fn(|cx| std::pin::Pin::new(&mut *reader).poll_read(cx, &mut rb))
                .await?;
            rb.filled().len()
        };
        ensure!(n > 0, "connection closed inside a WS frame payload");
        // SAFETY: the ReadBuf above just initialized these `n` bytes of
        // spare capacity.
        unsafe { payload.set_len(payload.len() + n) };
    }
    if let Some(key) = mask_key {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= key[i % 4];
        }
    }
    Ok(opcode)
}

/// Mask `payload` with the 4-byte periodic key, appending to `out` and
/// reusing its allocation. u32-wide XOR over the 4-byte-aligned body,
/// per-byte for the tail; byte-identical to `payload[i] ^ key[i % 4]` at
/// every index (`from_ne_bytes`/`to_ne_bytes` keep in-memory byte order, so
/// this is endian-neutral).
fn mask_payload_into(out: &mut Vec<u8>, payload: &[u8], key: [u8; 4]) {
    let key32 = u32::from_ne_bytes(key);
    let (chunks, remainder) = payload.as_chunks::<4>();
    for c in chunks {
        let masked = u32::from_ne_bytes(*c) ^ key32;
        out.extend_from_slice(&masked.to_ne_bytes());
    }
    // The tail starts at a multiple of 4, so its key phase restarts at 0.
    for (i, b) in remainder.iter().enumerate() {
        out.push(b ^ key[i]);
    }
}

/// Append one masked client→server frame (header + masked payload) to
/// `out`. FIN is always set: one write = one message. The mask key is fresh
/// random per frame (RFC 6455 mandates client→server masking).
fn encode_client_frame_into(out: &mut Vec<u8>, opcode: u8, payload: &[u8]) {
    let key: [u8; 4] = rand::rng().random();
    // 2-byte head + up to 8-byte extended length + 4-byte mask key.
    out.reserve(14 + payload.len());
    out.push(0x80 | opcode);
    let len = payload.len();
    if len < 126 {
        out.push(0x80 | len as u8);
    } else if len <= 0xFFFF {
        out.push(0x80 | 126);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(0x80 | 127);
        out.extend_from_slice(&(len as u64).to_be_bytes());
    }
    out.extend_from_slice(&key);
    mask_payload_into(out, payload, key);
}

/// WebSocket stream with byte-stream semantics: a direct poll-based frame
/// codec over the inner stream (same shape as `ShadowTlsIo`) — no pump
/// tasks, no duplex bridge, so payload bytes cross the codec with a single
/// copy per direction (frame buffer ↔ caller buffer) instead of two copies
/// plus a cross-task wakeup per chunk.
///
/// Read side state machine, per frame: assemble the header (`hdr`/`hdr_pos`;
/// `hdr_len` grows from 2 once the fixed part reveals the extended-length
/// and mask-key sizes) → assemble the payload into `payload` through spare
/// capacity → dispatch on opcode (data: deliver through `payload_pos`,
/// retaining leftovers larger than the caller's buffer across polls; ping:
/// queue a pong; close: queue an echo, then EOF). Write side: every frame —
/// caller data or a queued control frame — is appended to `out` and drained
/// in order, so wire ordering across data/pong/close survives a poll that
/// pends mid-frame.
pub struct WsStream {
    inner: BoxProxyStream,

    // -- read side --
    /// Bytes that arrived glued to the handshake response (a fast server's
    /// first frame can share the segment); drained before the inner stream.
    read_pending: Vec<u8>,
    read_pending_pos: usize,
    /// Frame header assembly.
    hdr: [u8; 14],
    hdr_pos: usize,
    hdr_len: usize,
    /// Payload of the frame being assembled / delivered. Assembly progress
    /// is `payload.len() < frame_len`; delivery progress is `payload_pos`.
    payload: Vec<u8>,
    payload_pos: usize,
    /// Payload length of the frame currently being assembled/delivered
    /// (0 between frames).
    frame_len: usize,
    /// The peer sent a close frame: once the echo is flushed, reads are EOF.
    closing: bool,
    /// Reads are done (close handshake finished or inner EOF).
    read_eof: bool,

    // -- write side --
    /// Encoded frames (header + masked payload) not yet flushed to the
    /// inner stream; data and control frames append here in order.
    out: Vec<u8>,
    out_pos: usize,
    /// Caller bytes represented by the data frame currently draining in
    /// `out`, so a `poll_write` that pended mid-frame knows what to report
    /// when it resumes.
    write_in_flight: Option<usize>,
    /// A control frame (pong / close echo) was queued from the read path
    /// and must be flushed before more frames are read.
    ctrl_flush: bool,
    /// A close frame was already sent (peer close echoed or local shutdown).
    close_sent: bool,
    /// The inner stream was shut down; further writes fail.
    write_closed: bool,
}

impl WsStream {
    fn new(inner: BoxProxyStream, handshake_leftover: Vec<u8>) -> Self {
        Self {
            inner,
            read_pending: handshake_leftover,
            read_pending_pos: 0,
            hdr: [0u8; 14],
            hdr_pos: 0,
            hdr_len: 2,
            payload: Vec::with_capacity(16384),
            payload_pos: 0,
            frame_len: 0,
            closing: false,
            read_eof: false,
            out: Vec::with_capacity(WRITE_BUF_SIZE + 14),
            out_pos: 0,
            write_in_flight: None,
            ctrl_flush: false,
            close_sent: false,
            write_closed: false,
        }
    }

    /// Reset the per-frame read state after a frame was fully delivered or
    /// handled; the `payload` allocation is kept for reuse.
    fn reset_frame(&mut self) {
        self.payload.clear();
        self.payload_pos = 0;
        self.frame_len = 0;
        self.hdr_pos = 0;
        self.hdr_len = 2;
    }

    /// Parse the assembled header into (opcode, payload length, mask key).
    fn parse_header(&self) -> io::Result<(u8, usize, Option<[u8; 4]>)> {
        let opcode = self.hdr[0] & 0x0f;
        let masked = self.hdr[1] & 0x80 != 0;
        let mut len = (self.hdr[1] & 0x7f) as u64;
        let mut off = 2;
        if len == 126 {
            len = u16::from_be_bytes([self.hdr[2], self.hdr[3]]) as u64;
            off = 4;
        } else if len == 127 {
            len = u64::from_be_bytes(self.hdr[2..10].try_into().unwrap());
            off = 10;
        }
        if len > MAX_FRAME_PAYLOAD {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("WS frame payload too large: {len} bytes"),
            ));
        }
        let mask = if masked {
            Some(self.hdr[off..off + 4].try_into().unwrap())
        } else {
            None
        };
        Ok((opcode, len as usize, mask))
    }

    /// Drain `out` (encoded frames, in order) into the inner stream, then
    /// flush it. The per-frame flush matches the old write task: nothing
    /// else flushes the underlying (possibly buffered/TLS) stream, and WS
    /// carries latency-sensitive handshake bytes.
    fn poll_flush_out(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.out_pos < self.out.len() {
            match Pin::new(&mut self.inner).poll_write(cx, &self.out[self.out_pos..]) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(e)) => {
                    self.out.clear();
                    self.out_pos = 0;
                    return Poll::Ready(Err(e));
                }
                Poll::Ready(Ok(0)) => {
                    self.out.clear();
                    self.out_pos = 0;
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "failed to write WS frame",
                    )));
                }
                Poll::Ready(Ok(n)) => self.out_pos += n,
            }
        }
        self.out.clear();
        self.out_pos = 0;
        Pin::new(&mut self.inner).poll_flush(cx)
    }
}

impl AsyncRead for WsStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            let this = &mut *self;

            // Deliver the assembled data-frame payload; a leftover larger
            // than the caller's buffer stays for the next poll.
            if this.payload_pos < this.payload.len() && this.payload.len() == this.frame_len {
                let n = buf.remaining().min(this.payload.len() - this.payload_pos);
                buf.put_slice(&this.payload[this.payload_pos..this.payload_pos + n]);
                this.payload_pos += n;
                if this.payload_pos == this.payload.len() {
                    this.reset_frame();
                }
                return Poll::Ready(Ok(()));
            }
            if buf.remaining() == 0 || this.read_eof {
                return Poll::Ready(Ok(()));
            }

            // A pong / close echo queued from the read path goes out before
            // more frames are read; once the close echo is flushed the
            // stream is at EOF.
            if this.ctrl_flush {
                match this.poll_flush_out(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Err(e)) => {
                        this.ctrl_flush = false;
                        if this.closing {
                            // The peer is gone either way; EOF is the signal.
                            this.read_eof = true;
                            return Poll::Ready(Ok(()));
                        }
                        return Poll::Ready(Err(e));
                    }
                    Poll::Ready(Ok(())) => this.ctrl_flush = false,
                }
            }
            if this.closing {
                this.read_eof = true;
                return Poll::Ready(Ok(()));
            }

            // Assemble the frame header; it may arrive byte by byte.
            while this.hdr_pos < this.hdr_len {
                if this.read_pending_pos < this.read_pending.len() {
                    let n = (this.hdr_len - this.hdr_pos)
                        .min(this.read_pending.len() - this.read_pending_pos);
                    this.hdr[this.hdr_pos..this.hdr_pos + n].copy_from_slice(
                        &this.read_pending[this.read_pending_pos..this.read_pending_pos + n],
                    );
                    this.hdr_pos += n;
                    this.read_pending_pos += n;
                    if this.read_pending_pos == this.read_pending.len() {
                        this.read_pending.clear();
                        this.read_pending_pos = 0;
                    }
                    continue;
                }
                let mut rb = ReadBuf::new(&mut this.hdr[this.hdr_pos..this.hdr_len]);
                match Pin::new(&mut this.inner).poll_read(cx, &mut rb) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                    Poll::Ready(Ok(())) => {
                        let n = rb.filled().len();
                        if n == 0 {
                            if this.hdr_pos == 0 {
                                // Clean EOF at a frame boundary.
                                this.read_eof = true;
                                return Poll::Ready(Ok(()));
                            }
                            return Poll::Ready(Err(io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                "connection closed inside a WS frame header",
                            )));
                        }
                        this.hdr_pos += n;
                    }
                }
            }
            if this.hdr_len == 2 {
                // Fixed part complete: the full header size is now known
                // (extended length + mask key). Minimal headers fall through
                // to the parse below.
                let ext = match this.hdr[1] & 0x7f {
                    126 => 2,
                    127 => 8,
                    _ => 0,
                };
                let mask = if this.hdr[1] & 0x80 != 0 { 4 } else { 0 };
                let full = 2 + ext + mask;
                if full > 2 {
                    this.hdr_len = full;
                    continue;
                }
            }
            let (opcode, len, mask_key) = match this.parse_header() {
                Ok(v) => v,
                Err(e) => return Poll::Ready(Err(e)),
            };

            // Assemble the payload through uninitialized spare capacity
            // (same pattern as the old read task): zero-filling the frame
            // first would be a wasted memset.
            this.payload.reserve(len - this.payload.len());
            while this.payload.len() < len {
                if this.read_pending_pos < this.read_pending.len() {
                    let n = (len - this.payload.len())
                        .min(this.read_pending.len() - this.read_pending_pos);
                    this.payload.extend_from_slice(
                        &this.read_pending[this.read_pending_pos..this.read_pending_pos + n],
                    );
                    this.read_pending_pos += n;
                    if this.read_pending_pos == this.read_pending.len() {
                        this.read_pending.clear();
                        this.read_pending_pos = 0;
                    }
                    continue;
                }
                let n = {
                    let filled = this.payload.len();
                    let chunk = &mut this.payload.spare_capacity_mut()[..len - filled];
                    let mut rb = ReadBuf::uninit(chunk);
                    match Pin::new(&mut this.inner).poll_read(cx, &mut rb) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                        Poll::Ready(Ok(())) => rb.filled().len(),
                    }
                };
                if n == 0 {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "connection closed inside a WS frame payload",
                    )));
                }
                // SAFETY: the ReadBuf above just initialized these `n` bytes
                // of spare capacity.
                unsafe { this.payload.set_len(this.payload.len() + n) };
            }
            this.frame_len = len;

            // Unmask in place (RFC forbids server→client masking, but
            // tolerating it is harmless).
            if let Some(key) = mask_key {
                for (i, b) in this.payload.iter_mut().enumerate() {
                    *b ^= key[i % 4];
                }
            }

            match opcode {
                OPCODE_BINARY | OPCODE_CONT | OPCODE_TEXT => {
                    if this.payload.is_empty() {
                        // Nothing to deliver, and a 0-byte read would signal
                        // EOF; skip to the next frame. `fin` is not surfaced:
                        // forwarding each fragment's payload in arrival order
                        // reassembles fragmented messages transparently.
                        this.reset_frame();
                    }
                }
                OPCODE_PING => {
                    encode_client_frame_into(&mut this.out, OPCODE_PONG, &this.payload);
                    this.ctrl_flush = true;
                    this.reset_frame();
                }
                OPCODE_CLOSE => {
                    if !this.close_sent {
                        encode_client_frame_into(&mut this.out, OPCODE_CLOSE, &[]);
                        this.close_sent = true;
                    }
                    this.ctrl_flush = true;
                    this.closing = true;
                    this.reset_frame();
                }
                // Pongs and unknown opcodes carry no stream bytes.
                _ => this.reset_frame(),
            }
        }
    }
}

impl AsyncWrite for WsStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.write_closed {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "WS stream is closed",
            )));
        }
        // A previous frame (caller data or a queued control frame) may still
        // be draining; finish it before taking new bytes.
        if self.write_in_flight.is_some() || self.out_pos < self.out.len() {
            match self.poll_flush_out(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(e)) => {
                    self.write_in_flight = None;
                    return Poll::Ready(Err(e));
                }
                Poll::Ready(Ok(())) => {}
            }
            if let Some(n) = self.write_in_flight.take() {
                return Poll::Ready(Ok(n));
            }
        }
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        // One poll_write = one masked binary frame, capped at WRITE_BUF_SIZE.
        let n = buf.len().min(WRITE_BUF_SIZE);
        encode_client_frame_into(&mut self.out, OPCODE_BINARY, &buf[..n]);
        self.write_in_flight = Some(n);
        match self.poll_flush_out(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(e)) => {
                self.write_in_flight = None;
                Poll::Ready(Err(e))
            }
            Poll::Ready(Ok(())) => {
                self.write_in_flight = None;
                Poll::Ready(Ok(n))
            }
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Deliberately does not touch `write_in_flight`: the pended
        // `poll_write` still owes its caller the completion.
        self.poll_flush_out(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Same close behaviour as the old write task: a close frame goes out
        // before the stream is torn down (queued after any in-flight data).
        if !self.close_sent {
            encode_client_frame_into(&mut self.out, OPCODE_CLOSE, &[]);
            self.close_sent = true;
        }
        match self.poll_flush_out(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Ready(Ok(())) => {}
        }
        match Pin::new(&mut self.inner).poll_shutdown(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Ready(Ok(())) => {
                self.write_closed = true;
                Poll::Ready(Ok(()))
            }
        }
    }
}

/// Perform the WS client handshake over an already-connected stream
/// (plain TCP, TLS, or Reality TLS), then return a byte-stream view of
/// the WS connection.
///
/// `default_host` (the server address) is used for the Host header when the
/// config doesn't override it.
pub async fn connect_ws(
    mut stream: BoxProxyStream,
    default_host: &str,
    config: &WsConfig,
) -> Result<BoxProxyStream> {
    let host = config.host.as_deref().unwrap_or(default_host);
    let path = normalize_path(config.path.as_deref());

    let (request, key) = build_handshake_request(host, &path, config.headers.as_ref());
    stream.write_all(&request).await?;
    stream.flush().await?;

    // Read the response head, preserving any bytes past the header block —
    // a fast server may already have sent the first WS frame in the same
    // segment.
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 512];
    let leftover = loop {
        if let Some(pos) = find_header_end(&buf) {
            let head = String::from_utf8_lossy(&buf[..pos]).into_owned();
            validate_handshake_response(&head, &key)?;
            break buf.split_off(pos + 4);
        }
        ensure!(
            buf.len() <= MAX_HANDSHAKE_RESPONSE,
            "WebSocket handshake response too large (>{})",
            MAX_HANDSHAKE_RESPONSE
        );
        let n = stream.read(&mut chunk).await?;
        ensure!(n > 0, "connection closed during WebSocket handshake");
        buf.extend_from_slice(&chunk[..n]);
    };

    Ok(Box::new(WsStream::new(stream, leftover)))
}

/// Convenience wrapper with a handshake timeout, used by the connection
/// factory so a stuck upgrade can't hang a connection attempt forever.
pub async fn connect_ws_timeout(
    stream: BoxProxyStream,
    default_host: &str,
    config: &WsConfig,
    timeout: Duration,
) -> Result<BoxProxyStream> {
    tokio::time::timeout(timeout, connect_ws(stream, default_host, config))
        .await
        .map_err(|_| {
            anyhow::anyhow!("WebSocket handshake timed out after {}s", timeout.as_secs())
        })?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::WsConfig;

    fn ws_config(path: Option<&str>, host: Option<&str>) -> WsConfig {
        WsConfig {
            path: path.map(|s| s.to_string()),
            host: host.map(|s| s.to_string()),
            headers: None,
        }
    }

    /// Run a WS handshake over a duplex link against a fake server that
    /// validates the request and replies 101. Returns (client stream,
    /// server end, the HTTP request head the client sent).
    async fn handshake_pair(config: WsConfig) -> (BoxProxyStream, tokio::io::DuplexStream, String) {
        let (client, mut server) = tokio::io::duplex(64 * 1024);
        let server_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            let mut chunk = [0u8; 256];
            loop {
                let n = server.read(&mut chunk).await?;
                buf.extend_from_slice(&chunk[..n]);
                if let Some(pos) = find_header_end(&buf) {
                    let head = String::from_utf8_lossy(&buf[..pos]).into_owned();
                    let key = head
                        .split("\r\n")
                        .find_map(|l| {
                            l.split_once(':').and_then(|(k, v)| {
                                k.trim()
                                    .eq_ignore_ascii_case("sec-websocket-key")
                                    .then(|| v.trim().to_string())
                            })
                        })
                        .context("no Sec-WebSocket-Key in request")?;
                    let resp = format!(
                        "HTTP/1.1 101 Switching Protocols\r\n\
                         Upgrade: websocket\r\n\
                         Connection: Upgrade\r\n\
                         Sec-WebSocket-Accept: {}\r\n\r\n",
                        compute_accept_key(&key)
                    );
                    server.write_all(resp.as_bytes()).await?;
                    return Ok((head, server));
                }
                anyhow::ensure!(buf.len() <= MAX_HANDSHAKE_RESPONSE, "request too large");
            }
        });
        let stream = connect_ws(Box::new(client) as BoxProxyStream, "example.com", &config)
            .await
            .expect("handshake must succeed");
        let (head, server) = server_task.await.unwrap().unwrap();
        (stream, server, head)
    }

    // -- Handshake ---------------------------------------------------------

    #[test]
    fn accept_key_matches_rfc6455_example() {
        // RFC 6455 §1.3 worked example.
        assert_eq!(
            compute_accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn build_request_has_required_headers() {
        let (req, key) = build_handshake_request("cdn.example.com", "/ray", None);
        let text = String::from_utf8(req).unwrap();
        assert!(text.starts_with("GET /ray HTTP/1.1\r\n"), "got: {}", text);
        assert!(text.contains("Host: cdn.example.com\r\n"));
        assert!(text.contains("Upgrade: websocket\r\n"));
        assert!(text.contains("Connection: Upgrade\r\n"));
        assert!(text.contains("Sec-WebSocket-Version: 13\r\n"));
        assert!(text.ends_with("\r\n\r\n"));
        // Key is base64 of 16 random bytes.
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&key)
            .unwrap();
        assert_eq!(decoded.len(), 16);
        assert!(text.contains(&format!("Sec-WebSocket-Key: {}\r\n", key)));
    }

    #[test]
    fn build_request_includes_custom_headers_but_skips_reserved() {
        let mut headers = std::collections::HashMap::new();
        headers.insert("Host".to_string(), "evil.example.com".to_string());
        headers.insert("X-Custom".to_string(), "abc".to_string());
        headers.insert("Upgrade".to_string(), "h2c".to_string());
        let (req, _) = build_handshake_request("cdn.example.com", "/", Some(&headers));
        let text = String::from_utf8(req).unwrap();
        assert!(text.contains("X-Custom: abc\r\n"));
        assert_eq!(text.matches("Host:").count(), 1, "got: {}", text);
        assert!(text.contains("Host: cdn.example.com\r\n"));
        assert_eq!(text.matches("Upgrade:").count(), 1);
    }

    #[test]
    fn validate_response_rejects_bad_status_and_bad_accept() {
        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        let good = format!(
            "HTTP/1.1 101 Switching Protocols\r\nSec-WebSocket-Accept: {}\r\n",
            compute_accept_key(key)
        );
        validate_handshake_response(&good, key).unwrap();

        let wrong_accept =
            "HTTP/1.1 101 Switching Protocols\r\nSec-WebSocket-Accept: AAAA\r\n".to_string();
        assert!(validate_handshake_response(&wrong_accept, key).is_err());

        assert!(validate_handshake_response("HTTP/1.1 200 OK\r\n", key).is_err());
        assert!(
            validate_handshake_response("HTTP/1.1 101 Switching Protocols\r\n", key).is_err(),
            "missing accept header must fail"
        );
    }

    // -- Frame codec -------------------------------------------------------

    #[tokio::test]
    async fn frame_roundtrip_masked_small() {
        let payload = b"hello websocket";
        let encoded = encode_frame_with_key(OPCODE_BINARY, payload, Some([0xDE, 0xAD, 0xBE, 0xEF]));
        // Header: FIN+binary, mask bit set, 7-bit length.
        assert_eq!(encoded[0], 0x82);
        assert_eq!(encoded[1], 0x80 | payload.len() as u8);
        assert_eq!(&encoded[2..6], &[0xDE, 0xAD, 0xBE, 0xEF]);

        let mut reader: &[u8] = &encoded;
        let mut pending = Vec::new();
        let mut out = Vec::new();
        let opcode = read_frame(&mut reader, &mut pending, &mut out)
            .await
            .unwrap();
        assert_eq!(opcode, OPCODE_BINARY);
        assert_eq!(out, payload);
    }

    #[tokio::test]
    async fn frame_roundtrip_16bit_length() {
        // 126..=65535 forces the 126 + u16 extended length form.
        let payload = vec![0xABu8; 40_000];
        let encoded = encode_client_frame(OPCODE_BINARY, &payload);
        assert_eq!(encoded[1] & 0x7f, 126);
        assert_eq!(u16::from_be_bytes([encoded[2], encoded[3]]), 40_000);

        let mut reader: &[u8] = &encoded;
        let mut pending = Vec::new();
        let mut out = Vec::new();
        read_frame(&mut reader, &mut pending, &mut out)
            .await
            .unwrap();
        assert_eq!(out, payload);
    }

    #[tokio::test]
    async fn frame_roundtrip_64bit_length_header() {
        // 65536 bytes still fits memory but already uses the 127 + u64 form.
        let payload = vec![0xCDu8; 65_536];
        let encoded = encode_client_frame(OPCODE_BINARY, &payload);
        assert_eq!(encoded[1] & 0x7f, 127);
        assert_eq!(
            u64::from_be_bytes(encoded[2..10].try_into().unwrap()),
            65_536
        );

        let mut reader: &[u8] = &encoded;
        let mut pending = Vec::new();
        let mut out = Vec::new();
        read_frame(&mut reader, &mut pending, &mut out)
            .await
            .unwrap();
        assert_eq!(out, payload);
    }

    #[tokio::test]
    async fn read_frame_tolerates_bytewise_delivery() {
        // Frame bytes dribbling in one at a time must still decode.
        let payload = b"segmented";
        let encoded = encode_client_frame(OPCODE_BINARY, payload);
        let (mut w, mut r) = tokio::io::duplex(8);
        let writer = tokio::spawn(async move {
            for b in &encoded {
                w.write_all(&[*b]).await.unwrap();
            }
        });
        let mut pending = Vec::new();
        let mut out = Vec::new();
        read_frame(&mut r, &mut pending, &mut out).await.unwrap();
        assert_eq!(out, payload);
        writer.await.unwrap();
    }

    // -- Full-duplex stream behaviour --------------------------------------

    #[tokio::test]
    async fn handshake_sends_expected_request() {
        let mut headers = std::collections::HashMap::new();
        headers.insert("X-Token".to_string(), "t0k3n".to_string());
        let config = WsConfig {
            path: Some("/ray".to_string()),
            host: Some("cdn.example.com".to_string()),
            headers: Some(headers),
        };
        let (_client, _server, request) = handshake_pair(config).await;
        assert!(request.starts_with("GET /ray HTTP/1.1\r\n"));
        assert!(request.contains("Host: cdn.example.com\r\n"));
        assert!(request.contains("X-Token: t0k3n"), "got: {:?}", request);
    }

    #[tokio::test]
    async fn handshake_failure_on_wrong_accept() {
        let (client, mut server) = tokio::io::duplex(4096);
        let server_task = tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            let _ = server.read(&mut buf).await;
            server
                .write_all(
                    b"HTTP/1.1 101 Switching Protocols\r\nSec-WebSocket-Accept: bogus\r\n\r\n",
                )
                .await
                .unwrap();
        });
        let result = connect_ws(
            Box::new(client) as BoxProxyStream,
            "example.com",
            &ws_config(None, None),
        )
        .await;
        assert!(result.is_err());
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn byte_stream_roundtrip_over_frames() {
        let (client, mut server, _request) = handshake_pair(ws_config(None, None)).await;

        let server_task = tokio::spawn(async move {
            // Echo server: unmask each client frame, reply with the payload
            // in an unmasked binary frame, until the client hangs up.
            let mut pending = Vec::new();
            let mut payload = Vec::new();
            loop {
                let Ok(opcode) = read_frame(&mut server, &mut pending, &mut payload).await else {
                    break;
                };
                if opcode != OPCODE_BINARY {
                    continue;
                }
                if server
                    .write_all(&encode_frame_with_key(OPCODE_BINARY, &payload, None))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        // Split so writes and reads proceed concurrently: a full-duplex echo
        // of more data than the internal buffers would deadlock a
        // write-all-then-read sequence (true for any buffered stream).
        let (mut rd, mut wr) = tokio::io::split(client);
        let data = vec![0x5Au8; 100_000]; // crosses the 64KB duplex buffer
        let sent = data.clone();
        let write_task = tokio::spawn(async move { wr.write_all(&sent).await.unwrap() });
        let mut got = vec![0u8; data.len()];
        rd.read_exact(&mut got).await.unwrap();
        assert_eq!(got, data);
        write_task.await.unwrap();
        server_task.abort();
    }

    #[tokio::test]
    async fn fragmented_message_is_reassembled() {
        let (mut client, mut server, _request) = handshake_pair(ws_config(None, None)).await;

        let server_task = tokio::spawn(async move {
            // FIN=0 binary + FIN=1 continuation = one fragmented message.
            let mut frag1 = vec![0x02]; // binary, FIN clear
            frag1.extend_from_slice(&encode_frame_with_key(OPCODE_BINARY, b"hello ", None)[1..]);
            // rebuild: encode_frame_with_key always sets FIN; strip and resend
            let cont = encode_frame_with_key(OPCODE_CONT, b"world", None);
            server.write_all(&frag1).await.unwrap();
            server.write_all(&cont).await.unwrap();
        });

        let mut got = [0u8; 11];
        client.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"hello world");
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn ping_is_answered_with_pong() {
        let (mut client, mut server, _request) = handshake_pair(ws_config(None, None)).await;

        let server_task = tokio::spawn(async move {
            server
                .write_all(&encode_frame_with_key(OPCODE_PING, b"pingbody", None))
                .await
                .unwrap();
            let mut pending = Vec::new();
            let mut payload = Vec::new();
            let opcode = read_frame(&mut server, &mut pending, &mut payload)
                .await
                .unwrap();
            assert_eq!(opcode, OPCODE_PONG);
            assert_eq!(payload, b"pingbody");
        });

        // The codec is lazy (no pump tasks): the ping is seen — and the pong
        // sent — while the caller polls reads. This read pends once the pong
        // has gone out, waiting for the next frame.
        let client_task = tokio::spawn(async move {
            let mut buf = [0u8; 64];
            let _ = client.read(&mut buf).await;
        });

        tokio::time::timeout(Duration::from_secs(5), server_task)
            .await
            .expect("pong never arrived")
            .unwrap();
        client_task.abort();
    }

    #[tokio::test]
    async fn close_frame_ends_the_stream() {
        let (mut client, mut server, _request) = handshake_pair(ws_config(None, None)).await;

        let server_task = tokio::spawn(async move {
            server
                .write_all(&encode_frame_with_key(OPCODE_CLOSE, &[], None))
                .await
                .unwrap();
            // Client must echo a close frame.
            let mut pending = Vec::new();
            let mut payload = Vec::new();
            let opcode = read_frame(&mut server, &mut pending, &mut payload)
                .await
                .unwrap();
            assert_eq!(opcode, OPCODE_CLOSE);
        });

        let mut buf = [0u8; 16];
        let n = client.read(&mut buf).await.unwrap();
        assert_eq!(n, 0, "close must surface as EOF");
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_sends_close_frame() {
        let (mut client, mut server, _request) = handshake_pair(ws_config(None, None)).await;

        client.shutdown().await.unwrap();
        let mut pending = Vec::new();
        let mut payload = Vec::new();
        let opcode = read_frame(&mut server, &mut pending, &mut payload)
            .await
            .unwrap();
        assert_eq!(opcode, OPCODE_CLOSE);
    }

    #[tokio::test]
    async fn stream_read_tolerates_bytewise_frame_delivery() {
        // Frame bytes dribbling in one at a time must still decode through
        // the poll state machine (partial header, partial payload).
        let (mut client, mut server, _request) = handshake_pair(ws_config(None, None)).await;
        let server_task = tokio::spawn(async move {
            for b in encode_frame_with_key(OPCODE_BINARY, b"trickle", None) {
                server.write_all(&[b]).await.unwrap();
            }
        });
        let mut got = [0u8; 7];
        client.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"trickle");
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn oversized_frame_is_delivered_across_reads() {
        // One 70KB frame (> the 64KB link buffer, so it arrives in pieces)
        // read through a small caller buffer: the payload leftover must be
        // retained across polls.
        let (mut client, mut server, _request) = handshake_pair(ws_config(None, None)).await;
        let server_task = tokio::spawn(async move {
            server
                .write_all(&encode_frame_with_key(
                    OPCODE_BINARY,
                    &vec![0x77u8; 70_000],
                    None,
                ))
                .await
                .unwrap();
        });
        let mut got = vec![0u8; 70_000];
        for chunk in got.chunks_mut(1000) {
            client.read_exact(chunk).await.unwrap();
        }
        assert!(got.iter().all(|&b| b == 0x77));
        server_task.await.unwrap();
    }

    /// Bytes that arrive glued to the end of the 101 response (same TCP
    /// segment) must not be lost.
    #[tokio::test]
    async fn frame_bytes_attached_to_handshake_response_survive() {
        let (client, mut server) = tokio::io::duplex(64 * 1024);
        let server_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            let mut chunk = [0u8; 256];
            loop {
                let n = server.read(&mut chunk).await.unwrap();
                buf.extend_from_slice(&chunk[..n]);
                if find_header_end(&buf).is_some() {
                    break;
                }
            }
            let key = String::from_utf8_lossy(&buf)
                .split("\r\n")
                .find_map(|l| {
                    l.split_once(':').and_then(|(k, v)| {
                        k.trim()
                            .eq_ignore_ascii_case("sec-websocket-key")
                            .then(|| v.trim().to_string())
                    })
                })
                .unwrap();
            // 101 response and the first data frame in ONE write.
            let mut resp = format!(
                "HTTP/1.1 101 Switching Protocols\r\nSec-WebSocket-Accept: {}\r\n\r\n",
                compute_accept_key(&key)
            )
            .into_bytes();
            resp.extend_from_slice(&encode_frame_with_key(OPCODE_BINARY, b"early", None));
            server.write_all(&resp).await.unwrap();
        });

        let mut stream = connect_ws(
            Box::new(client) as BoxProxyStream,
            "example.com",
            &ws_config(None, None),
        )
        .await
        .unwrap();
        let mut got = [0u8; 5];
        stream.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"early");
        server_task.await.unwrap();
    }
}

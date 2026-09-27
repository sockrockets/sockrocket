//! Hysteria2 protocol client implementation.
//!
//! Hysteria2 is a QUIC-based proxy protocol that uses HTTP/3 for authentication
//! and raw QUIC streams for TCP proxying. It tunnels TCP connections over QUIC
//! with HTTP/3-style masquerading.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use anyhow::{Result, bail};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

use crate::config::model::TlsConfig;

use super::connector::{BoxProxyStream, Outbound};
use super::quic_conn::{
    QuicConnectionState, build_quic_client_config, ensure_quic_crypto_provider,
    resolve_server_addrs,
};

const HY2_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(7);
const HY2_CONNECT_ATTEMPTS_PER_ADDR: usize = 2;
const HY2_CONNECT_RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_millis(250);
/// Stagger between concurrent per-address QUIC connect attempts
/// (happy-eyeballs). CDN-fronted nodes resolve to a mix of live and dead IPs;
/// a sequential scan pays the full QUIC handshake timeout per dead entry, which
/// blows the caller's overall deadline when several ALPN candidates are tried.
const HY2_ADDR_STAGGER: std::time::Duration = std::time::Duration::from_millis(500);
/// Stagger between concurrent ALPN candidates for the *same* address. Official
/// Hysteria2 servers only accept "hysteria" and panels only accept "h3", so
/// when the first candidate is going to fail by timeout (dead network path)
/// the second would otherwise pay another full handshake timeout sequentially.
const HY2_ALPN_STAGGER: std::time::Duration = std::time::Duration::from_millis(500);

/// Address-attempt context for spawned happy-eyeballs tasks. Avoids borrowing
/// `&self` across task boundaries.
struct Hysteria2Attempt {
    server: String,
    port: u16,
    sni: String,
}

impl Hysteria2Attempt {
    async fn try_addr(
        &self,
        addr: std::net::SocketAddr,
        client_config: &quinn::ClientConfig,
        alpn: &str,
    ) -> Result<(quinn::Endpoint, quinn::Connection)> {
        let bind_addr: std::net::SocketAddr = if addr.is_ipv4() {
            "0.0.0.0:0".parse()?
        } else {
            "[::]:0".parse()?
        };
        let mut endpoint = quinn::Endpoint::client(bind_addr)?;
        endpoint.set_default_client_config(client_config.clone());

        let connecting = endpoint
            .connect(addr, &self.sni)
            .map_err(|e| anyhow::anyhow!("QUIC connect setup to {} failed: {}", addr, e))?;
        let connection = tokio::time::timeout(HY2_CONNECT_TIMEOUT, connecting)
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "Hysteria2 QUIC to {}:{} via {} alpn='{}' timed out after {}s",
                    self.server,
                    self.port,
                    addr,
                    alpn,
                    HY2_CONNECT_TIMEOUT.as_secs()
                )
            })?
            .map_err(|e| {
                anyhow::anyhow!(
                    "Hysteria2 QUIC to {}:{} via {} alpn='{}' failed: {}",
                    self.server,
                    self.port,
                    addr,
                    alpn,
                    e
                )
            })?;
        tracing::info!(
            "Hysteria2 QUIC connection established to {}:{} ({}) alpn='{}'",
            self.server,
            self.port,
            addr,
            alpn
        );
        Ok((endpoint, connection))
    }
}

/// Hysteria2 outbound connector.
///
/// Uses QUIC (quinn) for transport with HTTP/3 authentication and
/// raw bidirectional streams for TCP proxy requests.
pub struct Hysteria2Outbound {
    server: String,
    port: u16,
    password: String,
    sni: String,
    skip_cert_verify: bool,
    /// ALPN candidates tried in order. The official Hysteria2 server requires
    /// "hysteria", but some widely deployed panels only accept "h3" — once a
    /// candidate succeeds it is remembered to skip the search on reconnects.
    alpn_candidates: Vec<String>,
    working_alpn: std::sync::Mutex<Option<String>>,
    conn_state: QuicConnectionState,
}

impl Hysteria2Outbound {
    pub fn new(
        server: &str,
        port: u16,
        password: &str,
        tls_config: Option<&TlsConfig>,
    ) -> Result<Self> {
        let sni = tls_config
            .and_then(|t| t.sni.as_deref())
            .unwrap_or(server)
            .to_string();
        let skip_cert_verify = tls_config.map(|t| t.skip_cert_verify).unwrap_or(false);
        let alpn_candidates = tls_config
            .and_then(|t| t.alpn.as_ref())
            .cloned()
            .unwrap_or_else(|| vec!["hysteria".to_string(), "h3".to_string()]);

        ensure_quic_crypto_provider();

        Ok(Self {
            server: server.to_string(),
            port,
            password: password.to_string(),
            sni,
            skip_cert_verify,
            alpn_candidates,
            working_alpn: std::sync::Mutex::new(None),
            conn_state: QuicConnectionState::new(),
        })
    }

    /// Build a QUIC client config (TLS + transport) for a given ALPN.
    fn build_client_config(&self, alpn: &str) -> Result<quinn::ClientConfig> {
        let mut client_config =
            build_quic_client_config(self.skip_cert_verify, &[alpn.to_string()])?;

        // Transport config: BBR congestion control for Hysteria2
        let mut transport = quinn::TransportConfig::default();
        transport.congestion_controller_factory(Arc::new(quinn::congestion::BbrConfig::default()));
        transport.keep_alive_interval(Some(std::time::Duration::from_secs(10)));
        transport.max_idle_timeout(Some(
            quinn::IdleTimeout::try_from(std::time::Duration::from_secs(30)).unwrap(),
        ));
        transport.receive_window(quinn::VarInt::from_u32(16 * 1024 * 1024));
        transport.send_window(16 * 1024 * 1024);
        client_config.transport_config(Arc::new(transport));
        Ok(client_config)
    }

    /// Get or create QUIC connection with authentication.
    ///
    /// Concurrent callers share a single QUIC session: the expensive
    /// connect+authenticate path is serialized inside `QuicConnectionState`.
    async fn get_connection(&self) -> Result<quinn::Connection> {
        self.conn_state
            .get_or_connect(|| async {
                let (endpoint, conn) = self.create_connection().await?;
                self.authenticate_h3(&conn).await?;
                Ok((endpoint, conn))
            })
            .await
    }

    async fn create_connection(&self) -> Result<(quinn::Endpoint, quinn::Connection)> {
        let addrs = resolve_server_addrs(&self.server, self.port).await?;
        tracing::debug!(
            "Hysteria2 resolved {}:{} -> {:?}",
            self.server,
            self.port,
            addrs
        );

        // Try the remembered working ALPN first, then remaining candidates.
        let mut alpns: Vec<String> = Vec::new();
        if let Some(working) = self.working_alpn.lock().unwrap().clone() {
            alpns.push(working);
        }
        for candidate in &self.alpn_candidates {
            if !alpns.contains(candidate) {
                alpns.push(candidate.clone());
            }
        }

        // Happy-eyeballs over resolved addresses × ALPN candidates: the first
        // successful handshake wins. When a working ALPN is already remembered
        // only that one is raced across addresses; otherwise all candidates
        // race with a short stagger so a dead network path doesn't serialize
        // per-ALPN timeouts.
        match self.connect_staggered(&addrs, &alpns).await {
            Ok((pair, used_alpn)) => {
                *self.working_alpn.lock().unwrap() = Some(used_alpn);
                Ok(pair)
            }
            Err(e) => Err(e),
        }
    }

    /// Try one address: endpoint setup + QUIC handshake with timeout.
    async fn try_addr(
        &self,
        addr: std::net::SocketAddr,
        client_config: &quinn::ClientConfig,
        alpn: &str,
    ) -> Result<(quinn::Endpoint, quinn::Connection)> {
        let attempt = Hysteria2Attempt {
            server: self.server.clone(),
            port: self.port,
            sni: self.sni.clone(),
        };
        attempt.try_addr(addr, client_config, alpn).await
    }

    /// Happy-eyeballs over all resolved addresses and ALPN candidates:
    /// attempts run concurrently with staggers, the first success wins and
    /// aborts the rest. This avoids paying a full handshake timeout per dead
    /// IP or per rejected ALPN when the network path is dead (common with
    /// CDN-fronted nodes resolving to mixed live/dead addresses).
    async fn connect_staggered(
        &self,
        addrs: &[std::net::SocketAddr],
        alpns: &[String],
    ) -> Result<((quinn::Endpoint, quinn::Connection), String)> {
        // Already know a working ALPN? Don't race others — just probe addresses.
        let remembered = self.working_alpn.lock().unwrap().clone();
        if let Some(alpn) = remembered {
            let client_config = self.build_client_config(&alpn)?;
            return self
                .race_addresses(addrs, &client_config, &alpn)
                .await
                .map(|pair| (pair, alpn));
        }
        // First connection: race every (addr, alpn) pair concurrently.
        let mut set: tokio::task::JoinSet<Result<((quinn::Endpoint, quinn::Connection), String)>> =
            tokio::task::JoinSet::new();
        let mut last_err = None;
        for (i, addr) in addrs.iter().copied().enumerate() {
            if i > 0 {
                tokio::select! {
                    _ = tokio::time::sleep(HY2_ADDR_STAGGER) => {}
                    Some(res) = set.join_next() => {
                        if let Ok(Ok(pair)) = res {
                            set.abort_all();
                            return Ok(pair);
                        }
                    }
                }
            }
            for (j, alpn) in alpns.iter().enumerate() {
                if j > 0 {
                    tokio::select! {
                        _ = tokio::time::sleep(HY2_ALPN_STAGGER) => {}
                        Some(res) = set.join_next() => {
                            if let Ok(Ok(pair)) = res {
                                set.abort_all();
                                return Ok(pair);
                            }
                        }
                    }
                }
                let client_config = match self.build_client_config(alpn) {
                    Ok(c) => c,
                    Err(e) => {
                        last_err = Some(e);
                        continue;
                    }
                };
                let this = Hysteria2Attempt {
                    server: self.server.clone(),
                    port: self.port,
                    sni: self.sni.clone(),
                };
                let alpn = alpn.clone();
                set.spawn(async move {
                    this.try_addr(addr, &client_config, &alpn)
                        .await
                        .map(|pair| (pair, alpn))
                });
            }
        }

        while let Some(res) = set.join_next().await {
            match res {
                Ok(Ok(pair)) => {
                    set.abort_all();
                    return Ok(pair);
                }
                Ok(Err(e)) => last_err = Some(e),
                Err(join_err) => {
                    last_err = Some(anyhow::anyhow!("QUIC attempt task failed: {}", join_err));
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no addresses to connect")))
    }

    /// Race only the addresses for a single ALPN (used when the working ALPN
    /// is already known).
    async fn race_addresses(
        &self,
        addrs: &[std::net::SocketAddr],
        client_config: &quinn::ClientConfig,
        alpn: &str,
    ) -> Result<(quinn::Endpoint, quinn::Connection)> {
        if addrs.len() == 1 {
            // Single address: plain sequential retries, no racing overhead.
            let mut last_err = None;
            for attempt in 1..=HY2_CONNECT_ATTEMPTS_PER_ADDR {
                match self.try_addr(addrs[0], client_config, alpn).await {
                    Ok(pair) => return Ok(pair),
                    Err(e) => last_err = Some(e),
                }
                if attempt < HY2_CONNECT_ATTEMPTS_PER_ADDR {
                    tokio::time::sleep(HY2_CONNECT_RETRY_BACKOFF).await;
                }
            }
            return Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no attempts made")));
        }

        let mut set: tokio::task::JoinSet<Result<(quinn::Endpoint, quinn::Connection)>> =
            tokio::task::JoinSet::new();
        for (i, addr) in addrs.iter().copied().enumerate() {
            if i > 0 {
                // Stagger, but bail the wait early if a previous attempt
                // already succeeded or failed (fast failover).
                tokio::select! {
                    _ = tokio::time::sleep(HY2_ADDR_STAGGER) => {}
                    Some(res) = set.join_next() => {
                        if let Ok(Ok(pair)) = res {
                            set.abort_all();
                            return Ok(pair);
                        }
                    }
                }
            }
            let this = Hysteria2Attempt {
                server: self.server.clone(),
                port: self.port,
                sni: self.sni.clone(),
            };
            let cfg = client_config.clone();
            let alpn = alpn.to_string();
            set.spawn(async move { this.try_addr(addr, &cfg, &alpn).await });
        }

        let mut last_err = None;
        while let Some(res) = set.join_next().await {
            match res {
                Ok(Ok(pair)) => {
                    set.abort_all();
                    return Ok(pair);
                }
                Ok(Err(e)) => last_err = Some(e),
                Err(join_err) => {
                    last_err = Some(anyhow::anyhow!("QUIC attempt task failed: {}", join_err));
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no addresses to connect")))
    }

    /// Authenticate via HTTP/3 POST /auth request.
    ///
    /// Per the Hysteria2 spec, the client sends an HTTP/3 request:
    ///   POST /auth with Hysteria-Auth header containing the password.
    /// The server responds with status 233 if authentication succeeds.
    async fn authenticate_h3(&self, conn: &quinn::Connection) -> Result<()> {
        let h3_conn = h3_quinn::Connection::new(conn.clone());
        let (_conn, mut sender) = h3::client::new(h3_conn).await?;

        // Build the auth request
        let req = http::Request::builder()
            .method("POST")
            .uri("https://hysteria/auth")
            .header("Hysteria-Auth", &self.password)
            .header("Hysteria-CC-RX", "0")
            .body(())
            .map_err(|e| anyhow::anyhow!("Failed to build auth request: {}", e))?;

        let mut stream = sender.send_request(req).await?;
        stream.finish().await?;

        let resp = stream.recv_response().await?;

        if resp.status() != 233 {
            bail!(
                "Hysteria2 auth failed: server returned status {} (expected 233)",
                resp.status()
            );
        }

        tracing::info!("Hysteria2 authentication successful (HTTP/3 status 233)");

        // Drop the h3 client — we now use raw QUIC streams for proxy requests
        drop(sender);

        Ok(())
    }
}

impl Outbound for Hysteria2Outbound {
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
        let host = host.to_string();
        Box::pin(async move {
            let addr_str = super::transport::format_host_port(&host, port);

            // Two attempts: handles the race where the cached QUIC connection
            // dies between get_connection() and open_bi() (stale connection).
            let mut last_err = anyhow::anyhow!("Hysteria2: no connection attempt made");
            for attempt in 1u8..=2 {
                let conn = self.get_connection().await?;
                let (mut send, mut recv) = match conn.open_bi().await {
                    Ok(pair) => pair,
                    Err(e) => {
                        last_err = anyhow::anyhow!(
                            "Hysteria2 open_bi failed (attempt {}): {}",
                            attempt,
                            e
                        );
                        tracing::debug!("{last_err} — will retry with fresh connection");
                        continue;
                    }
                };

                // Hysteria2 TCP request (per protocol spec):
                //   varint 0x401 (TCPRequest ID)
                //   varint address_length
                //   bytes  address string ("host:port")
                //   varint padding_length (0)
                //   bytes  padding (empty)
                let addr_bytes = addr_str.as_bytes();
                write_varint(&mut send, 0x0401).await?;
                write_varint(&mut send, addr_bytes.len() as u64).await?;
                send.write_all(addr_bytes).await?;
                write_varint(&mut send, 0).await?; // no padding
                send.flush().await?;

                // Read the server's TCPResponse (present in full on both
                // success and failure — verified against live servers).
                let (status, msg) = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    read_tcp_response(&mut recv),
                )
                .await
                .map_err(|_| anyhow::anyhow!("Hysteria2 TCPResponse timeout"))??;

                if status == 0x01 {
                    bail!(
                        "Hysteria2 TCP connect to {} rejected by server: {}",
                        addr_str,
                        msg
                    );
                }

                tracing::debug!("Hysteria2 TCP connect to {} ({})", addr_str, msg);

                let stream = Hy2BidiStream { send, recv };
                return Ok(Box::new(stream) as BoxProxyStream);
            }
            Err(last_err)
        })
    }

    fn name(&self) -> &str {
        "hysteria2"
    }
}

/// Combined bidirectional QUIC stream for Hysteria2.
struct Hy2BidiStream {
    send: quinn::SendStream,
    recv: quinn::RecvStream,
}

impl AsyncRead for Hy2BidiStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match Pin::new(&mut self.get_mut().recv).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
            Poll::Ready(Err(e)) => Poll::Ready(Err(io::Error::other(e))),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncWrite for Hy2BidiStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match Pin::new(&mut self.get_mut().send).poll_write(cx, buf) {
            Poll::Ready(Ok(n)) => Poll::Ready(Ok(n)),
            Poll::Ready(Err(e)) => Poll::Ready(Err(io::Error::other(e))),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match Pin::new(&mut self.get_mut().send).poll_flush(cx) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
            Poll::Ready(Err(e)) => Poll::Ready(Err(io::Error::other(e))),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match Pin::new(&mut self.get_mut().send).poll_shutdown(cx) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
            Poll::Ready(Err(e)) => Poll::Ready(Err(io::Error::other(e))),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Unpin for Hy2BidiStream {}

// --- Protocol helpers ---

/// Upper bound for the TCPResponse message string; anything larger is a
/// protocol violation, not a message to buffer.
const HY2_MAX_TCP_RESPONSE_MSG_LEN: u64 = 64 * 1024;

/// Read a Hysteria2 TCPResponse:
///   uint8  status (0x00 = OK, 0x01 = Error)
///   varint message_length + bytes message string ("Connected" on success)
///   varint padding_length + bytes padding
///
/// The message is read in full (up to [`HY2_MAX_TCP_RESPONSE_MSG_LEN`]);
/// truncating it would leave bytes in the stream and permanently corrupt
/// subsequent reads.
async fn read_tcp_response<R: AsyncRead + Unpin>(recv: &mut R) -> Result<(u8, String)> {
    let status = recv.read_u8().await?;

    let msg_len = read_varint(recv).await?;
    anyhow::ensure!(
        msg_len <= HY2_MAX_TCP_RESPONSE_MSG_LEN,
        "Hysteria2 TCPResponse message too large: {msg_len}"
    );
    let mut msg = vec![0u8; msg_len as usize];
    recv.read_exact(&mut msg).await?;
    let msg = String::from_utf8_lossy(&msg).to_string();

    let pad_len = read_varint(recv).await?;
    anyhow::ensure!(
        pad_len <= 1 << 20,
        "Hysteria2 TCPResponse padding too large: {pad_len}"
    );
    skip_exact(recv, pad_len as usize).await?;

    Ok((status, msg))
}

/// Write a QUIC variable-length integer.
async fn write_varint<W: AsyncWrite + Unpin>(writer: &mut W, value: u64) -> Result<()> {
    if value < 64 {
        writer.write_all(&[value as u8]).await?;
    } else if value < 16384 {
        let bytes = ((value as u16) | 0x4000).to_be_bytes();
        writer.write_all(&bytes).await?;
    } else if value < 1_073_741_824 {
        let bytes = ((value as u32) | 0x80000000).to_be_bytes();
        writer.write_all(&bytes).await?;
    } else {
        let bytes = (value | 0xC000000000000000).to_be_bytes();
        writer.write_all(&bytes).await?;
    }
    Ok(())
}

/// Read a QUIC variable-length integer (2-bit length prefix: 1/2/4/8 bytes).
async fn read_varint<R: AsyncRead + Unpin>(recv: &mut R) -> Result<u64> {
    let mut first = [0u8; 1];
    recv.read_exact(&mut first).await?;
    let total_len = 1usize << (first[0] >> 6);
    let mut value = (first[0] & 0x3f) as u64;
    for _ in 1..total_len {
        let mut byte = [0u8; 1];
        recv.read_exact(&mut byte).await?;
        value = (value << 8) | byte[0] as u64;
    }
    Ok(value)
}

/// Read and discard exactly `len` bytes.
async fn skip_exact<R: AsyncRead + Unpin>(recv: &mut R, len: usize) -> Result<()> {
    let mut remaining = len;
    let mut chunk = [0u8; 8192];
    while remaining > 0 {
        let n = remaining.min(chunk.len());
        recv.read_exact(&mut chunk[..n]).await?;
        remaining -= n;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hysteria2_outbound_new() {
        let outbound = Hysteria2Outbound::new("server.com", 443, "my_password", None).unwrap();
        assert_eq!(outbound.name(), "hysteria2");
        assert_eq!(outbound.server, "server.com");
        assert_eq!(outbound.port, 443);
        assert_eq!(outbound.sni, "server.com");
    }

    #[test]
    fn test_hysteria2_with_tls_config() {
        let tls = TlsConfig {
            sni: Some("custom.sni.com".to_string()),
            skip_cert_verify: true,
            alpn: Some(vec!["h3".to_string()]),
            fingerprint: None,
        };
        let outbound = Hysteria2Outbound::new("server.com", 443, "pass", Some(&tls)).unwrap();
        assert_eq!(outbound.sni, "custom.sni.com");
    }

    /// Regression: a TCPResponse message longer than 4096 bytes must be read
    /// in full; truncating it left bytes in the stream and corrupted the
    /// connection permanently.
    #[tokio::test]
    async fn test_read_tcp_response_reads_full_message() {
        let (mut client, mut server) = tokio::io::duplex(64 * 1024);
        let payload = vec![b'x'; 6000]; // larger than the old 4096 cap

        let server_task = tokio::spawn({
            let payload = payload.clone();
            async move {
                server.write_u8(0x00).await.unwrap(); // status OK
                write_varint(&mut server, payload.len() as u64)
                    .await
                    .unwrap();
                server.write_all(&payload).await.unwrap();
                write_varint(&mut server, 7).await.unwrap(); // padding length
                server.write_all(&[0u8; 7]).await.unwrap(); // padding
            }
        });

        let (status, msg) = read_tcp_response(&mut client).await.unwrap();
        server_task.await.unwrap();

        assert_eq!(status, 0x00);
        assert_eq!(msg.len(), 6000);
        assert!(msg.as_bytes().iter().all(|&b| b == b'x'));
    }

    #[tokio::test]
    async fn test_read_tcp_response_rejects_oversized_message() {
        let (mut client, mut server) = tokio::io::duplex(1024);
        let server_task = tokio::spawn(async move {
            server.write_u8(0x00).await.unwrap();
            // Claim a message larger than the 64 KiB cap.
            write_varint(&mut server, HY2_MAX_TCP_RESPONSE_MSG_LEN + 1)
                .await
                .unwrap();
        });

        let err = read_tcp_response(&mut client).await.unwrap_err();
        server_task.await.unwrap();
        assert!(
            format!("{err:#}").contains("message too large"),
            "unexpected error: {err:#}"
        );
    }

    #[tokio::test]
    async fn test_read_tcp_response_error_status_with_message() {
        let (mut client, mut server) = tokio::io::duplex(1024);
        let server_task = tokio::spawn(async move {
            server.write_u8(0x01).await.unwrap(); // status Error
            write_varint(&mut server, 8).await.unwrap();
            server.write_all(b"rejected").await.unwrap();
            write_varint(&mut server, 0).await.unwrap(); // no padding
        });

        let (status, msg) = read_tcp_response(&mut client).await.unwrap();
        server_task.await.unwrap();
        assert_eq!(status, 0x01);
        assert_eq!(msg, "rejected");
    }
}

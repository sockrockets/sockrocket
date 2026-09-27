//! TUIC v5 protocol client implementation.
//!
//! TUIC v5 runs over QUIC (quinn), providing multiplexed streams with
//! a single persistent QUIC connection per outbound.

use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use anyhow::{Result, bail};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};

use crate::config::model::TlsConfig;

use super::connector::{BoxProxyStream, Outbound};
use super::quic_conn::{
    QuicConnectionState, build_quic_client_config, ensure_quic_crypto_provider,
    resolve_server_addrs,
};

// TUIC v5 protocol constants
const TUIC_VERSION: u8 = 0x05;
const CMD_AUTHENTICATE: u8 = 0x00;
const CMD_CONNECT: u8 = 0x01;
#[allow(dead_code)]
const CMD_PACKET: u8 = 0x02;
#[allow(dead_code)]
const CMD_DISSOCIATE: u8 = 0x03;
#[allow(dead_code)]
const CMD_HEARTBEAT: u8 = 0x04;
const TUIC_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(7);
const TUIC_CONNECT_ATTEMPTS_PER_ADDR: usize = 2;
const TUIC_CONNECT_RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_millis(250);
/// Stagger between concurrent (addr, alpn) QUIC connect attempts
/// (happy-eyeballs). Same rationale as Hysteria2: dead IPs or rejected ALPNs
/// must not serialize full handshake timeouts.
const TUIC_ADDR_STAGGER: std::time::Duration = std::time::Duration::from_millis(500);
const TUIC_ALPN_STAGGER: std::time::Duration = std::time::Duration::from_millis(500);

// Address types
const ADDR_TYPE_DOMAIN: u8 = 0x00;
const ADDR_TYPE_IPV4: u8 = 0x01;
const ADDR_TYPE_IPV6: u8 = 0x02;

/// TUIC v5 outbound connector.
pub struct TuicOutbound {
    server: String,
    port: u16,
    uuid: [u8; 16],
    password: String,
    sni: String,
    skip_cert_verify: bool,
    congestion: CongestionControl,
    /// ALPN candidates tried in order. TUIC v5 specifies "tuic", but many
    /// servers are deployed with "h3" — a working candidate is remembered to
    /// skip the search on reconnects.
    alpn_candidates: Vec<String>,
    working_alpn: std::sync::Mutex<Option<String>>,
    /// Pre-built QUIC client config (avoids rebuilding root certs per reconnect)
    quic_client_config: quinn::ClientConfig,
    conn_state: QuicConnectionState,
}

/// Address-attempt context for spawned happy-eyeballs tasks. Avoids borrowing
/// `&self` across task boundaries.
struct TuicAttempt {
    server: String,
    port: u16,
    sni: String,
}

impl TuicAttempt {
    async fn try_addr(
        &self,
        addr: SocketAddr,
        client_config: &quinn::ClientConfig,
        alpn: &str,
    ) -> Result<(quinn::Endpoint, quinn::Connection)> {
        let bind_addr: SocketAddr = if addr.is_ipv4() {
            "0.0.0.0:0".parse()?
        } else {
            "[::]:0".parse()?
        };
        let mut endpoint = quinn::Endpoint::client(bind_addr)?;
        endpoint.set_default_client_config(client_config.clone());

        let connecting = endpoint
            .connect(addr, &self.sni)
            .map_err(|e| anyhow::anyhow!("QUIC connect setup to {} failed: {}", addr, e))?;
        let connection = tokio::time::timeout(TUIC_CONNECT_TIMEOUT, connecting)
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "TUIC QUIC connection to {}:{} via {} alpn='{}' timed out after {}s (UDP may be blocked or unstable)",
                    self.server,
                    self.port,
                    addr,
                    alpn,
                    TUIC_CONNECT_TIMEOUT.as_secs()
                )
            })?
            .map_err(|e| {
                anyhow::anyhow!(
                    "TUIC QUIC connection to {}:{} via {} alpn='{}' failed: {}",
                    self.server,
                    self.port,
                    addr,
                    alpn,
                    e
                )
            })?;
        tracing::info!(
            "TUIC QUIC connection established to {}:{} ({}) alpn='{}'",
            self.server,
            self.port,
            addr,
            alpn
        );
        Ok((endpoint, connection))
    }
}

#[derive(Clone, Copy, Debug)]
enum CongestionControl {
    Cubic,
    NewReno,
    Bbr,
}

impl CongestionControl {
    fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "cubic" => Self::Cubic,
            "new_reno" | "newreno" => Self::NewReno,
            "bbr" => Self::Bbr,
            _ => Self::Cubic,
        }
    }
}

impl TuicOutbound {
    pub fn new(
        server: &str,
        port: u16,
        uuid_str: &str,
        password: &str,
        congestion: &str,
        tls_config: Option<&TlsConfig>,
    ) -> Result<Self> {
        let uuid = parse_uuid_bytes(uuid_str)?;
        let sni = tls_config
            .and_then(|t| t.sni.as_deref())
            .unwrap_or(server)
            .to_string();
        let skip_cert_verify = tls_config.map(|t| t.skip_cert_verify).unwrap_or(false);
        let alpn_candidates = tls_config
            .and_then(|t| t.alpn.as_ref())
            .cloned()
            .unwrap_or_else(|| vec!["tuic".to_string(), "h3".to_string()]);
        let congestion = CongestionControl::from_str(congestion);

        // Build QUIC TLS config once
        ensure_quic_crypto_provider();
        let quic_client_config =
            build_quic_client_config(skip_cert_verify, &[alpn_candidates[0].clone()])?;

        Ok(Self {
            server: server.to_string(),
            port,
            uuid,
            password: password.to_string(),
            sni,
            skip_cert_verify,
            congestion,
            alpn_candidates,
            working_alpn: std::sync::Mutex::new(None),
            quic_client_config,
            conn_state: QuicConnectionState::new(),
        })
    }

    /// QUIC client config for a given ALPN candidate. The config built in
    /// `new()` already carries the first candidate's ALPN.
    fn config_for_alpn(&self, alpn: &str) -> Result<quinn::ClientConfig> {
        if self.alpn_candidates.first().is_some_and(|a| a == alpn) {
            return Ok(self.quic_client_config.clone());
        }
        build_quic_client_config(self.skip_cert_verify, &[alpn.to_string()])
    }

    /// Get or create QUIC connection, authenticating on first connect.
    ///
    /// Concurrent callers share a single QUIC session: the expensive
    /// connect+authenticate path is serialized inside `QuicConnectionState`.
    async fn get_connection(&self) -> Result<quinn::Connection> {
        self.conn_state
            .get_or_connect(|| async {
                let (endpoint, conn) = self.create_connection().await?;
                self.authenticate(&conn).await?;
                Ok((endpoint, conn))
            })
            .await
    }

    async fn create_connection(&self) -> Result<(quinn::Endpoint, quinn::Connection)> {
        let addrs = resolve_server_addrs(&self.server, self.port).await?;
        tracing::debug!("TUIC resolved {}:{} -> {:?}", self.server, self.port, addrs);

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
        addr: SocketAddr,
        client_config: &quinn::ClientConfig,
        alpn: &str,
    ) -> Result<(quinn::Endpoint, quinn::Connection)> {
        let attempt = TuicAttempt {
            server: self.server.clone(),
            port: self.port,
            sni: self.sni.clone(),
        };
        attempt.try_addr(addr, client_config, alpn).await
    }

    /// Happy-eyeballs over all resolved addresses and ALPN candidates.
    async fn connect_staggered(
        &self,
        addrs: &[SocketAddr],
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
                    _ = tokio::time::sleep(TUIC_ADDR_STAGGER) => {}
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
                        _ = tokio::time::sleep(TUIC_ALPN_STAGGER) => {}
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
                let this = TuicAttempt {
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
        addrs: &[SocketAddr],
        client_config: &quinn::ClientConfig,
        alpn: &str,
    ) -> Result<(quinn::Endpoint, quinn::Connection)> {
        if addrs.len() == 1 {
            // Single address: plain sequential retries, no racing overhead.
            let mut last_err = None;
            for attempt in 1..=TUIC_CONNECT_ATTEMPTS_PER_ADDR {
                match self.try_addr(addrs[0], client_config, alpn).await {
                    Ok(pair) => return Ok(pair),
                    Err(e) => last_err = Some(e),
                }
                if attempt < TUIC_CONNECT_ATTEMPTS_PER_ADDR {
                    tokio::time::sleep(TUIC_CONNECT_RETRY_BACKOFF).await;
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
                    _ = tokio::time::sleep(TUIC_ADDR_STAGGER) => {}
                    Some(res) = set.join_next() => {
                        if let Ok(Ok(pair)) = res {
                            set.abort_all();
                            return Ok(pair);
                        }
                    }
                }
            }
            let this = TuicAttempt {
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

    /// QUIC client config (TLS + transport) for a given ALPN candidate.
    fn build_client_config(&self, alpn: &str) -> Result<quinn::ClientConfig> {
        let mut client_config = self.config_for_alpn(alpn)?;

        // Set transport config with congestion control
        let mut transport = quinn::TransportConfig::default();
        match self.congestion {
            CongestionControl::Bbr => {
                transport.congestion_controller_factory(Arc::new(
                    quinn::congestion::BbrConfig::default(),
                ));
            }
            CongestionControl::NewReno => {
                transport.congestion_controller_factory(Arc::new(
                    quinn::congestion::NewRenoConfig::default(),
                ));
            }
            CongestionControl::Cubic => {
                transport.congestion_controller_factory(Arc::new(
                    quinn::congestion::CubicConfig::default(),
                ));
            }
        }
        // Keep-alive for long-lived connections
        transport.keep_alive_interval(Some(std::time::Duration::from_secs(15)));
        // Max idle timeout to detect dead connections
        transport.max_idle_timeout(Some(
            quinn::IdleTimeout::try_from(std::time::Duration::from_secs(30)).unwrap(),
        ));
        client_config.transport_config(Arc::new(transport));
        Ok(client_config)
    }

    /// Send authentication on a unidirectional stream.
    async fn authenticate(&self, conn: &quinn::Connection) -> Result<()> {
        let mut send = conn.open_uni().await?;

        let token = export_auth_token(conn, &self.uuid, &self.password)?;
        send.write_u8(TUIC_VERSION).await?;
        send.write_u8(CMD_AUTHENTICATE).await?;
        send.write_all(&self.uuid).await?;
        send.write_all(&token).await?;
        send.flush().await?;

        send.finish()?;
        tracing::info!("TUIC authentication sent");

        Ok(())
    }
}

impl Outbound for TuicOutbound {
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
        let host = host.to_string();
        Box::pin(async move {
            // Two attempts: if open_bi() fails the connection died between
            // get_connection() returning it and us using it (race with server
            // closing the QUIC connection). get_connection() will detect the
            // dead connection via close_reason() on the next call and reconnect.
            let mut last_err = anyhow::anyhow!("TUIC: no connection attempt made");
            for attempt in 1u8..=2 {
                let conn = self.get_connection().await?;
                let (mut send, recv) = match conn.open_bi().await {
                    Ok(pair) => pair,
                    Err(e) => {
                        last_err =
                            anyhow::anyhow!("TUIC open_bi failed (attempt {}): {}", attempt, e);
                        tracing::debug!("{last_err} — will retry with fresh connection");
                        continue;
                    }
                };

                // Send Connect command — must flush so server receives it before relay data
                send.write_u8(TUIC_VERSION).await?;
                send.write_u8(CMD_CONNECT).await?;
                write_address(&mut send, &host, port).await?;
                send.flush().await?;

                tracing::debug!("TUIC connect to {}:{}", host, port);

                let stream = QuicBidiStream { send, recv };
                return Ok(Box::new(stream) as BoxProxyStream);
            }
            Err(last_err)
        })
    }

    fn name(&self) -> &str {
        "tuic"
    }
}

/// Combined bidirectional QUIC stream.
struct QuicBidiStream {
    send: quinn::SendStream,
    recv: quinn::RecvStream,
}

impl AsyncRead for QuicBidiStream {
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

impl AsyncWrite for QuicBidiStream {
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

impl Unpin for QuicBidiStream {}

// --- Protocol helpers ---

/// Write TUIC address (type + addr + port).
async fn write_address(writer: &mut quinn::SendStream, host: &str, port: u16) -> Result<()> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        match ip {
            IpAddr::V4(v4) => {
                writer.write_u8(ADDR_TYPE_IPV4).await?;
                writer.write_all(&v4.octets()).await?;
            }
            IpAddr::V6(v6) => {
                writer.write_u8(ADDR_TYPE_IPV6).await?;
                writer.write_all(&v6.octets()).await?;
            }
        }
    } else {
        let domain = host.as_bytes();
        if domain.len() > 255 {
            bail!("Domain name too long: {}", host);
        }
        writer.write_u8(ADDR_TYPE_DOMAIN).await?;
        writer.write_u8(domain.len() as u8).await?;
        writer.write_all(domain).await?;
    }
    writer.write_all(&port.to_be_bytes()).await?;
    Ok(())
}

/// Parse UUID string to 16 bytes.
fn parse_uuid_bytes(uuid_str: &str) -> Result<[u8; 16]> {
    let uuid = uuid::Uuid::parse_str(uuid_str)?;
    Ok(*uuid.as_bytes())
}

fn export_auth_token(
    conn: &quinn::Connection,
    uuid: &[u8; 16],
    password: &str,
) -> Result<[u8; 32]> {
    let mut token = [0u8; 32];
    conn.export_keying_material(&mut token, uuid, password.as_bytes())
        .map_err(|err| anyhow::anyhow!("failed to export TUIC auth token: {:?}", err))?;
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_uuid() {
        let uuid = parse_uuid_bytes("550e8400-e29b-41d4-a716-446655440000").unwrap();
        assert_eq!(uuid[0], 0x55);
        assert_eq!(uuid[15], 0x00);
    }

    #[test]
    fn test_congestion_control() {
        assert!(matches!(
            CongestionControl::from_str("bbr"),
            CongestionControl::Bbr
        ));
        assert!(matches!(
            CongestionControl::from_str("cubic"),
            CongestionControl::Cubic
        ));
        assert!(matches!(
            CongestionControl::from_str("new_reno"),
            CongestionControl::NewReno
        ));
        assert!(matches!(
            CongestionControl::from_str("unknown"),
            CongestionControl::Cubic
        ));
    }

    #[test]
    fn test_tuic_outbound_new() {
        let outbound = TuicOutbound::new(
            "server.com",
            443,
            "550e8400-e29b-41d4-a716-446655440000",
            "my_password",
            "bbr",
            None,
        )
        .unwrap();
        assert_eq!(outbound.name(), "tuic");
        assert_eq!(outbound.server, "server.com");
        assert_eq!(outbound.port, 443);
    }

    #[test]
    fn test_protocol_constants() {
        assert_eq!(TUIC_VERSION, 0x05);
        assert_eq!(CMD_AUTHENTICATE, 0x00);
        assert_eq!(CMD_CONNECT, 0x01);
        assert_eq!(CMD_PACKET, 0x02);
        assert_eq!(CMD_DISSOCIATE, 0x03);
        assert_eq!(CMD_HEARTBEAT, 0x04);
        assert_eq!(ADDR_TYPE_DOMAIN, 0x00);
        assert_eq!(ADDR_TYPE_IPV4, 0x01);
        assert_eq!(ADDR_TYPE_IPV6, 0x02);
    }
}

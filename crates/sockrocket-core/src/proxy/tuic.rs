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

use crate::config::model::{TlsConfig, TuicRelay};

use super::connector::{BoxProxyStream, Outbound};
use super::quic_conn::{
    QuicConnectionState, build_quic_client_config_with_early_data, ensure_quic_crypto_provider,
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
    certificate_pin: Option<[u8; 32]>,
    congestion: CongestionControl,
    pub(super) relay: TuicRelay,
    pub(super) heartbeat: std::time::Duration,
    heartbeat_task: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// ALPN candidates tried in order. TUIC v5 specifies "tuic", but many
    /// servers are deployed with "h3" — a working candidate is remembered to
    /// skip the search on reconnects.
    alpn_candidates: Vec<String>,
    working_alpn: std::sync::Mutex<Option<String>>,
    /// Pre-built QUIC client config (avoids rebuilding root certs per reconnect)
    configs: std::sync::Mutex<std::collections::HashMap<String, quinn::ClientConfig>>,
    zero_rtt: bool,
    conn_state: QuicConnectionState,
    udp_hub: tokio::sync::Mutex<Option<Arc<super::quic_datagram::Hub>>>,
}

/// A resumed connection can carry streams before its authentication exporter is ready.
struct Dialed {
    endpoint: quinn::Endpoint,
    conn: quinn::Connection,
    handshake: Option<quinn::ZeroRttAccepted>,
}

/// Owned context for concurrent happy-eyeballs attempts.
struct TuicAttempt {
    zero_rtt: bool,
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
    ) -> Result<Dialed> {
        let bind_addr: SocketAddr = if addr.is_ipv4() {
            "0.0.0.0:0".parse()?
        } else {
            "[::]:0".parse()?
        };
        let mut endpoint = super::quic_conn::client_endpoint(bind_addr)?;
        endpoint.set_default_client_config(client_config.clone());

        let connecting = endpoint
            .connect(addr, &self.sni)
            .map_err(|e| anyhow::anyhow!("QUIC connect setup to {} failed: {}", addr, e))?;
        let connecting = if self.zero_rtt {
            match connecting.into_0rtt() {
                Ok((conn, handshake)) => {
                    return Ok(Dialed {
                        endpoint,
                        conn,
                        handshake: Some(handshake),
                    });
                }
                Err(normal) => normal,
            }
        } else {
            connecting
        };
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
        tracing::info!("TUIC QUIC connection established, ALPN={}", alpn);
        Ok(Dialed {
            endpoint,
            conn: connection,
            handshake: None,
        })
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
        Self::new_pinned(
            server, port, uuid_str, password, congestion, tls_config, None, false,
        )
    }

    pub(crate) fn new_pinned(
        server: &str,
        port: u16,
        uuid_str: &str,
        password: &str,
        congestion: &str,
        tls_config: Option<&TlsConfig>,
        certificate_pin: Option<[u8; 32]>,
        zero_rtt: bool,
    ) -> Result<Self> {
        let uuid = parse_uuid_bytes(uuid_str)?;
        let sni = tls_config
            .and_then(|t| t.sni.as_deref())
            .unwrap_or(server)
            .to_string();
        let skip_cert_verify = tls_config.map(|t| t.skip_cert_verify).unwrap_or(false);
        let mut alpn_candidates = tls_config
            .and_then(|t| t.alpn.as_ref())
            .cloned()
            .unwrap_or_else(|| vec!["tuic".to_string(), "h3".to_string()]);
        alpn_candidates.retain(|value| !value.is_empty());
        if alpn_candidates.is_empty() {
            alpn_candidates = vec!["tuic".into(), "h3".into()];
        }
        anyhow::ensure!(
            alpn_candidates.iter().all(|a| a.len() <= 255),
            "Invalid TUIC ALPN length"
        );
        let congestion = CongestionControl::from_str(congestion);

        // Build QUIC TLS config once
        ensure_quic_crypto_provider();
        let quic_client_config = build_quic_client_config_with_early_data(
            skip_cert_verify,
            &[alpn_candidates[0].clone()],
            certificate_pin,
            zero_rtt,
        )?;

        Ok(Self {
            server: server.to_string(),
            port,
            uuid,
            password: password.to_string(),
            sni,
            skip_cert_verify,
            certificate_pin,
            congestion,
            relay: TuicRelay::Native,
            heartbeat: std::time::Duration::from_secs(10),
            heartbeat_task: Default::default(),
            configs: std::sync::Mutex::new(std::collections::HashMap::from([(
                alpn_candidates[0].clone(),
                quic_client_config,
            )])),
            alpn_candidates,
            working_alpn: std::sync::Mutex::new(None),
            zero_rtt,
            conn_state: QuicConnectionState::new(),
            udp_hub: tokio::sync::Mutex::new(None),
        })
    }

    /// QUIC client config for a given ALPN candidate. The config built in
    /// `new()` already carries the first candidate's ALPN.
    fn config_for_alpn(&self, alpn: &str) -> Result<quinn::ClientConfig> {
        let mut configs = self.configs.lock().unwrap();
        if let Some(config) = configs.get(alpn) {
            return Ok(config.clone());
        }
        let config = build_quic_client_config_with_early_data(
            self.skip_cert_verify,
            &[alpn.to_owned()],
            self.certificate_pin,
            self.zero_rtt,
        )?;
        configs.insert(alpn.to_owned(), config.clone());
        Ok(config)
    }

    /// Get or create QUIC connection, authenticating on first connect.
    ///
    /// Concurrent callers share a single QUIC session: the expensive
    /// connect+authenticate path is serialized inside `QuicConnectionState`.
    async fn get_connection(&self) -> Result<quinn::Connection> {
        self.conn_state
            .get_or_connect(|| async {
                let Dialed {
                    endpoint,
                    conn,
                    handshake,
                } = self.create_connection().await?;
                if let Some(handshake) = handshake {
                    // The TLS exporter becomes available at handshake completion.
                    // Peer rejection invalidates early streams; never replay caller bytes.
                    let conn = conn.clone();
                    let uuid = self.uuid;
                    let password = self.password.clone();
                    tokio::spawn(async move {
                        let auth = async {
                            handshake.await;
                            Self::authenticate(&conn, &uuid, &password).await
                        };
                        if !matches!(
                            tokio::time::timeout(TUIC_CONNECT_TIMEOUT, auth).await,
                            Ok(Ok(()))
                        ) {
                            conn.close(1u32.into(), b"TUIC authentication failed");
                        }
                    });
                } else {
                    Self::authenticate(&conn, &self.uuid, &self.password).await?;
                }
                self.start_heartbeat(&conn);
                Ok((endpoint, conn))
            })
            .await
    }

    fn start_heartbeat(&self, conn: &quinn::Connection) {
        let mut task = self.heartbeat_task.lock().unwrap();
        if let Some(previous) = task.take() {
            previous.abort();
        }
        if conn.max_datagram_size().is_none() {
            return;
        }
        let conn = conn.clone();
        let interval = self.heartbeat;
        *task = Some(tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = conn.closed() => break,
                    _ = tokio::time::sleep(interval) => {
                        if conn.send_datagram(bytes::Bytes::from_static(&[TUIC_VERSION, CMD_HEARTBEAT])).is_err() { break; }
                    }
                }
            }
        }));
    }

    async fn create_connection(&self) -> Result<Dialed> {
        let addrs = resolve_server_addrs(&self.server, self.port).await?;
        tracing::debug!("TUIC server resolution completed");

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
    ) -> Result<Dialed> {
        let attempt = TuicAttempt {
            zero_rtt: self.zero_rtt,
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
    ) -> Result<(Dialed, String)> {
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
        let mut set: tokio::task::JoinSet<Result<(Dialed, String)>> = tokio::task::JoinSet::new();
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
                    zero_rtt: self.zero_rtt,
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
    ) -> Result<Dialed> {
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

        let mut set: tokio::task::JoinSet<Result<Dialed>> = tokio::task::JoinSet::new();
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
                zero_rtt: self.zero_rtt,
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
        // Quinn's default aggregate receive window is VarInt::MAX. A slow
        // reader must backpressure the peer before per-stream buffers add up
        // beyond the TUN payload budget (which cannot account for QUIC RAM).
        transport.receive_window(quinn::VarInt::from_u32(16 * 1024 * 1024));
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
        transport.keep_alive_interval(Some(self.heartbeat));
        // Max idle timeout to detect dead connections
        transport.max_idle_timeout(Some(
            quinn::IdleTimeout::try_from(std::time::Duration::from_secs(30)).unwrap(),
        ));
        client_config.transport_config(Arc::new(transport));
        Ok(client_config)
    }

    /// Send authentication on a unidirectional stream.
    async fn authenticate(conn: &quinn::Connection, uuid: &[u8; 16], password: &str) -> Result<()> {
        let mut send = conn.open_uni().await?;

        let token = export_auth_token(conn, uuid, password)?;
        send.write_u8(TUIC_VERSION).await?;
        send.write_u8(CMD_AUTHENTICATE).await?;
        send.write_all(uuid).await?;
        send.write_all(&token).await?;
        send.flush().await?;

        send.finish()?;
        tracing::info!("TUIC authentication sent");

        Ok(())
    }
}

impl Drop for TuicOutbound {
    fn drop(&mut self) {
        if let Some(task) = self.heartbeat_task.get_mut().unwrap().take() {
            task.abort();
        }
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
                        conn.close(quinn::VarInt::from_u32(0), b"open_bi failed");
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

    fn connect_datagram(
        &self,
        host: &str,
        port: u16,
    ) -> super::datagram::DatagramFuture<'_, super::datagram::BoxDatagram> {
        let host = host.to_owned();
        Box::pin(async move {
            if matches!(self.relay, TuicRelay::UotV1 | TuicRelay::UotV2) {
                use super::stream_datagram::{Framing, StreamDatagram};
                let mut address = super::anytls::socks_address(&host, port)?;
                if self.relay == TuicRelay::UotV1 {
                    address[0] = match address[0] {
                        1 => 0,
                        4 => 1,
                        3 => 2,
                        _ => unreachable!(),
                    };
                    let stream = self.connect("sp.udp-over-tcp.arpa", 0).await?;
                    return Ok(StreamDatagram::boxed(stream, Framing::UotLegacy(address)));
                }
                let mut stream = self.connect("sp.v2.udp-over-tcp.arpa", 0).await?;
                stream.write_u8(1).await?;
                stream.write_all(&address).await?;
                stream.flush().await?;
                return Ok(StreamDatagram::boxed(stream, Framing::Length16));
            }
            let connection = self.get_connection().await?;
            let mut hub = self.udp_hub.lock().await;
            if hub.as_ref().is_none_or(|hub| !hub.matches(&connection)) {
                *hub = Some(super::quic_datagram::Hub::with_relay(
                    connection,
                    super::quic_datagram::Protocol::Tuic,
                    self.relay == TuicRelay::Quic,
                ));
            }
            hub.as_ref().unwrap().associate(&host, port)
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

    fn local_server() -> quinn::Endpoint {
        use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
        super::super::quic_conn::ensure_quic_crypto_provider();
        let cert = CertificateDer::from(include_bytes!("testdata/localhost-cert.der").to_vec());
        let key = PrivatePkcs8KeyDer::from(include_bytes!("testdata/localhost-key.der").to_vec());
        let mut tls = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert], key.into())
            .unwrap();
        tls.max_early_data_size = u32::MAX;
        tls.alpn_protocols = vec![b"tuic".to_vec()];
        let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls).unwrap();
        let mut config = quinn::ServerConfig::with_crypto(Arc::new(crypto));
        Arc::get_mut(&mut config.transport)
            .unwrap()
            .send_window(64 * 1024 * 1024);
        quinn::Endpoint::server(config, "127.0.0.1:0".parse().unwrap()).unwrap()
    }

    #[tokio::test]
    async fn opted_in_tuic_resumes_before_server_accepts_and_authenticates() {
        let server = local_server();
        let tls = TlsConfig {
            sni: Some("localhost".into()),
            skip_cert_verify: true,
            alpn: Some(vec!["tuic".into()]),
            fingerprint: None,
        };
        let out = TuicOutbound::new_pinned(
            "127.0.0.1",
            server.local_addr().unwrap().port(),
            "00000000-0000-0000-0000-000000000001",
            "fixture",
            "cubic",
            Some(&tls),
            None,
            true,
        )
        .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(6), async {
            let (client, peer) = tokio::join!(out.get_connection(), async {
                server.accept().await.unwrap().await.unwrap()
            });
            let first = client.unwrap();
            let auth = peer
                .accept_uni()
                .await
                .unwrap()
                .read_to_end(64)
                .await
                .unwrap();
            assert_eq!(auth.len(), 50);
            // Ensure the post-handshake ticket reaches the client before disconnect.
            peer.send_datagram(bytes::Bytes::from_static(b"ready"))
                .unwrap();
            first.read_datagram().await.unwrap();
            first.close(0u32.into(), b"fixture reconnect");
            peer.closed().await;
            // No server accept is polled until application bytes are queued.
            let resumed = out.get_connection().await.unwrap();
            let (mut send, mut read) = resumed.open_bi().await.unwrap();
            send.write_all(b"early").await.unwrap();
            send.finish().unwrap();
            assert!(read.is_0rtt());
            let peer = server.accept().await.unwrap().await.unwrap();
            let (mut reply, mut input) = peer.accept_bi().await.unwrap();
            let bytes = input.read_to_end(16).await.unwrap();
            assert_eq!(&bytes, b"early");
            let auth = peer
                .accept_uni()
                .await
                .unwrap()
                .read_to_end(64)
                .await
                .unwrap();
            let token = export_auth_token(&peer, &out.uuid, &out.password).unwrap();
            assert_eq!(auth[18..], token);
            reply.write_all(b"ok").await.unwrap();
            reply.finish().unwrap();
            assert_eq!(&read.read_to_end(16).await.unwrap(), b"ok");
        })
        .await
        .expect("TUIC early stream deadline");
    }

    #[tokio::test]
    async fn configured_heartbeat_reaches_authenticated_connection() {
        let server = local_server();
        let tls = TlsConfig {
            sni: Some("localhost".into()),
            skip_cert_verify: true,
            alpn: Some(vec!["tuic".into()]),
            fingerprint: None,
        };
        let mut outbound = TuicOutbound::new(
            "127.0.0.1",
            server.local_addr().unwrap().port(),
            "00000000-0000-0000-0000-000000000001",
            "test",
            "cubic",
            Some(&tls),
        )
        .unwrap();
        outbound.heartbeat = std::time::Duration::from_millis(25);
        let (client, peer) = tokio::join!(outbound.get_connection(), async {
            server.accept().await.unwrap().await.unwrap()
        });
        let _connection = client.unwrap();
        let mut auth = peer.accept_uni().await.unwrap();
        let bytes = auth.read_to_end(64).await.unwrap();
        assert_eq!(&bytes[..2], &[TUIC_VERSION, CMD_AUTHENTICATE]);
        for _ in 0..2 {
            let packet =
                tokio::time::timeout(std::time::Duration::from_secs(1), peer.read_datagram())
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(&packet[..], &[TUIC_VERSION, CMD_HEARTBEAT]);
        }
    }
    #[tokio::test]
    async fn stalled_readers_share_a_connection_receive_budget() {
        use std::time::Duration;
        let server = local_server();
        let tls = TlsConfig {
            sni: Some("localhost".into()),
            skip_cert_verify: true,
            alpn: None,
            fingerprint: None,
        };
        let outbound = TuicOutbound::new(
            "localhost",
            443,
            "00000000-0000-0000-0000-000000000001",
            "test",
            "bbr",
            Some(&tls),
        )
        .unwrap();
        let mut client = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        client.set_default_client_config(outbound.build_client_config("tuic").unwrap());
        let connecting = client
            .connect(server.local_addr().unwrap(), "localhost")
            .unwrap();
        let (client_conn, server_conn) = tokio::join!(connecting, async {
            server.accept().await.unwrap().await.unwrap()
        });
        let client_conn = client_conn.unwrap();
        let sender = tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            for _ in 0..24 {
                let mut stream = server_conn.open_uni().await.unwrap();
                tasks.spawn(async move {
                    stream.write_all(&vec![0x5a; 1024 * 1024]).await.unwrap();
                    stream.finish().unwrap();
                    assert_eq!(stream.stopped().await.unwrap(), None);
                });
            }
            while let Some(result) = tasks.join_next().await {
                result.unwrap();
            }
            server_conn
        });
        let first = tokio::time::timeout(Duration::from_secs(5), client_conn.accept_uni())
            .await
            .unwrap()
            .unwrap();
        let mut sender = sender;
        // Every stream is smaller than Quinn's individual stream window.
        // Only an aggregate window prevents all 24 MiB being ACKed into RAM.
        assert!(
            tokio::time::timeout(Duration::from_secs(3), &mut sender)
                .await
                .is_err(),
            "all payload was buffered despite stalled application readers"
        );
        tokio::time::timeout(Duration::from_secs(10), async {
            let mut readers = tokio::task::JoinSet::new();
            let mut first = Some(first);
            for _ in 0..24 {
                let mut receiver = match first.take() {
                    Some(stream) => stream,
                    None => client_conn.accept_uni().await.unwrap(),
                };
                readers.spawn(async move {
                    let bytes = receiver.read_to_end(1024 * 1024).await.unwrap();
                    assert_eq!(bytes.len(), 1024 * 1024);
                    assert!(bytes.iter().all(|b| *b == 0x5a));
                });
            }
            while let Some(result) = readers.join_next().await {
                result.unwrap();
            }
        })
        .await
        .unwrap();
        let connection = sender.await.unwrap();
        connection.close(0u32.into(), b"done");
    }

    #[test]
    fn test_parse_uuid() {
        let uuid = parse_uuid_bytes("550e8400-e29b-41d4-a716-446655440000").unwrap();
        assert_eq!(uuid[0], 0x55);
        assert_eq!(uuid[15], 0x00);
    }

    #[test]
    fn empty_alpn_uses_safe_defaults() {
        let tls = TlsConfig {
            sni: Some("tls.example.invalid".into()),
            skip_cert_verify: false,
            alpn: Some(vec![]),
            fingerprint: None,
        };
        let outbound = TuicOutbound::new(
            "vpn.example.invalid",
            443,
            "00000000-0000-0000-0000-000000000001",
            "synthetic-secret",
            "bbr",
            Some(&tls),
        )
        .unwrap();
        assert_eq!(outbound.alpn_candidates, ["tuic", "h3"]);
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

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use craft_tls::TlsConnector;
use craft_tls::client::TlsStream;
use craft_tls::rustls::client::danger::{
    HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
};
use craft_tls::rustls::{ClientConfig, DigitallySignedStruct, Error as TlsError, SignatureScheme};
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
use socket2::{SockRef, TcpKeepalive};
use tokio::net::TcpStream;

use crate::config::model::{RealityConfig, TlsConfig, TransportConfig, TransportType, WsConfig};

use super::connector::BoxProxyStream;
use super::pool::ConnFactory;

/// Default timeout for TCP connect + TLS handshake.
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Socket send/receive buffer size (256KB — improves throughput on high-BDP links).
const SOCKET_BUF_SIZE: usize = 256 * 1024;
/// TCP keepalive interval (keeps connections alive through NATs/firewalls).
const TCP_KEEPALIVE_SECS: u64 = 30;

/// Globally cached root certificate store (built once, reused everywhere).
static ROOT_CERT_STORE: OnceLock<craft_tls::rustls::RootCertStore> = OnceLock::new();

pub(crate) fn get_root_cert_store() -> &'static craft_tls::rustls::RootCertStore {
    ROOT_CERT_STORE.get_or_init(|| {
        let mut store = craft_tls::rustls::RootCertStore::empty();
        store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        store
    })
}

/// Map a fingerprint name from config to a craftls FingerprintBuilder.
fn fingerprint_builder(name: Option<&str>) -> craft_tls::rustls::craft::FingerprintBuilder {
    use craft_tls::rustls::craft::*;
    match name.map(|s| s.to_lowercase()).as_deref() {
        Some("chrome") | None => CHROME_112.builder(),
        Some("safari") => {
            // SAFARI_17_1 advertises TLS 1.0/1.1 in supported_versions, which
            // this rustls base cannot emit; strict mode would panic per
            // connection, so relax it.
            SAFARI_17_1.builder().dangerous_disable_strict_mode()
        }
        Some("firefox") => FIREFOX_105.builder(),
        Some("chrome108") => CHROME_108.builder(),
        // Default to Chrome 112 for any unrecognized fingerprint
        _ => CHROME_112.builder(),
    }
}

/// Apply performance-critical socket options to a connected TCP stream:
/// TCP_NODELAY, enlarged send/receive buffers, and TCP keepalive.
pub(crate) fn tune_socket(stream: &TcpStream) {
    stream.set_nodelay(true).ok();
    let sock = SockRef::from(stream);
    sock.set_send_buffer_size(SOCKET_BUF_SIZE).ok();
    sock.set_recv_buffer_size(SOCKET_BUF_SIZE).ok();
    let keepalive = TcpKeepalive::new().with_time(Duration::from_secs(TCP_KEEPALIVE_SECS));
    sock.set_tcp_keepalive(&keepalive).ok();
}

/// Format a `host:port` connect string, bracketing IPv6 literals.
///
/// Node configs reach the outbounds in both forms — `url`-crate parsing keeps
/// the brackets (`[2001:db8::1]`), VMess JSON `add` fields carry them bare
/// (`2001:db8::1`) — so a naive `format!("{}:{}", host, port)` breaks on the
/// bare form (`2001:db8::1:443` is unparsable). Domains pass through unchanged.
pub(crate) fn format_host_port(host: &str, port: u16) -> String {
    let bare = host.trim_matches(['[', ']']);
    if bare.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{}]:{}", bare, port)
    } else {
        format!("{}:{}", host, port)
    }
}

/// Resolve address string and connect TCP with a timeout, applying socket tuning.
///
/// When the host part is a domain (the common case for proxy server addresses),
/// resolution goes through a process-wide cache ([`resolve_server_ips`]) to
/// avoid a fresh DNS lookup on every connection. Any cache/lookup failure
/// falls back to the original direct `TcpStream::connect` behaviour.
pub(crate) async fn connect_tcp(addr: &str, timeout: Duration) -> Result<TcpStream> {
    let tcp = tokio::time::timeout(timeout, connect_tcp_inner(addr))
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "TCP connection to {} timed out after {}s",
                addr,
                timeout.as_secs()
            )
        })?
        .with_context(|| format!("TCP connection to {} failed", addr))?;
    tune_socket(&tcp);
    Ok(tcp)
}

async fn connect_tcp_inner(addr: &str) -> Result<TcpStream> {
    if let Some((host, port_str)) = addr.rsplit_once(':')
        && let Ok(port) = port_str.parse::<u16>()
    {
        let bare = host.trim_matches(['[', ']']);
        // IP literal (the common case for TUN sessions): connect through an
        // explicitly built socket so the TUN-mode outbound-interface binding
        // is applied — a plain `TcpStream::connect` would follow the TUN
        // default route and loop the session back into ourselves.
        if let Ok(ip) = bare.parse::<IpAddr>() {
            return connect_bound(SocketAddr::new(ip, port))
                .await
                .with_context(|| format!("TCP connection to {} failed", addr));
        }
        if let Some(ips) = resolve_server_ips(host).await {
            let sockaddrs: Vec<SocketAddr> =
                ips.iter().map(|ip| SocketAddr::new(*ip, port)).collect();
            return connect_staggered(&sockaddrs)
                .await
                .with_context(|| format!("TCP connection to {} failed", addr));
        }
    }
    // Last resort (domain the server-IP cache couldn't resolve): resolve via
    // the system resolver, still with the outbound binding applied.
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host(addr)
        .await
        .with_context(|| format!("DNS resolution for {} failed", addr))?
        .collect();
    connect_staggered(&addrs)
        .await
        .with_context(|| format!("TCP connection to {} failed", addr))
}

/// Connect to a single address via an explicitly built socket so the
/// TUN-mode outbound-interface binding (IP_UNICAST_IF / SO_BINDTODEVICE /
/// IP_BOUND_IF) can be applied before connecting — without it, our own
/// direct-routed connections would loop back into the TUN device.
async fn connect_bound(sa: SocketAddr) -> std::io::Result<TcpStream> {
    let socket = if sa.is_ipv4() {
        tokio::net::TcpSocket::new_v4()?
    } else {
        tokio::net::TcpSocket::new_v6()?
    };
    super::outbound_bind::apply_to_tcp_socket(&socket, sa.is_ipv6())?;
    socket.connect(sa).await
}

/// Happy-eyeballs-style connect over multiple resolved addresses.
///
/// CDN-fronted proxy nodes often resolve to a mix of live and dead IPs; a
/// sequential scan pays a full SYN timeout (≈20s on Windows) per dead entry.
/// Instead, attempts start staggered 300ms apart and the first success wins.
async fn connect_staggered(addrs: &[SocketAddr]) -> std::io::Result<TcpStream> {
    use tokio::task::JoinSet;
    const STAGGER: Duration = Duration::from_millis(300);

    let mut set: JoinSet<std::io::Result<TcpStream>> = JoinSet::new();
    let mut last_err: Option<std::io::Error> = None;
    for (i, sa) in addrs.iter().copied().enumerate() {
        if i > 0 {
            // Wait out the stagger delay, but bail early if a previous
            // attempt already succeeded, and skip the rest of the delay if
            // one already failed (fast failover).
            tokio::select! {
                _ = tokio::time::sleep(STAGGER) => {}
                Some(res) = set.join_next() => {
                    match res {
                        Ok(Ok(stream)) => {
                            set.abort_all();
                            return Ok(stream);
                        }
                        Ok(Err(e)) => last_err = Some(e),
                        Err(join_err) => {
                            last_err = Some(std::io::Error::other(join_err));
                        }
                    }
                }
            }
        }
        set.spawn(async move {
            // Build the socket explicitly so the TUN-mode outbound-interface
            // binding can be applied before connecting.
            connect_bound(sa).await
        });
    }
    while let Some(res) = set.join_next().await {
        match res {
            Ok(Ok(stream)) => {
                set.abort_all();
                return Ok(stream);
            }
            Ok(Err(e)) => last_err = Some(e),
            Err(join_err) => {
                last_err = Some(std::io::Error::other(join_err));
            }
        }
    }
    Err(last_err.unwrap_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "no addresses to connect")
    }))
}

// --- Server-address DNS cache ---

/// TTL for cached proxy-server DNS answers.
const SERVER_ADDR_CACHE_TTL: Duration = Duration::from_secs(60);

/// Minimal DNS cache for proxy *server* hostnames.
///
/// The full [`crate::dns::DnsResolver`] carries china-split routing semantics
/// meant for intercepted client queries; proxy server addresses only need a
/// short-TTL memoization of the system resolver to avoid a lookup per
/// connection.
struct ServerAddrCache {
    entries: Mutex<HashMap<String, (Vec<IpAddr>, Instant)>>,
}

impl ServerAddrCache {
    fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Return the cached addresses if the entry is still fresh at `now`.
    fn get(&self, host: &str, now: Instant) -> Option<Vec<IpAddr>> {
        let entries = self.entries.lock().ok()?;
        match entries.get(host) {
            Some((ips, ts)) if now.duration_since(*ts) < SERVER_ADDR_CACHE_TTL => Some(ips.clone()),
            _ => None,
        }
    }

    fn insert(&self, host: &str, ips: Vec<IpAddr>, now: Instant) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(host.to_string(), (ips, now));
        }
    }
}

static SERVER_ADDR_CACHE: OnceLock<ServerAddrCache> = OnceLock::new();

fn server_addr_cache() -> &'static ServerAddrCache {
    SERVER_ADDR_CACHE.get_or_init(ServerAddrCache::new)
}

/// Servers whose REALITY endpoint rejects the hybrid key share and needs a
/// classic-X25519-only ClientHello.
///
/// Without this, `create()` re-runs the *failing* handshake on every single
/// connection and only then retries — measured in production as dozens of
/// fallbacks, i.e. every connection paid for two handshakes. Remembering the
/// server lets subsequent connections go straight to the working variant.
static REALITY_CLASSIC_SERVERS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn reality_classic_cache_path() -> std::path::PathBuf {
    // /tmp on Merlin is tmpfs (survives daemon restart, cleared on reboot).
    // Desktop: still fine; worst case the file is ignored if unwritable.
    std::env::temp_dir().join("sockrocket_reality_classic.cache")
}

fn load_reality_classic_cache() -> HashSet<String> {
    let path = reality_classic_cache_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return HashSet::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && l.contains(':'))
        .map(str::to_string)
        .collect()
}

fn persist_reality_classic_cache(set: &HashSet<String>) {
    let path = reality_classic_cache_path();
    let mut lines: Vec<&str> = set.iter().map(String::as_str).collect();
    lines.sort_unstable();
    let body = lines.join("\n");
    let _ = std::fs::write(path, body);
}

fn reality_classic_servers() -> &'static Mutex<HashSet<String>> {
    REALITY_CLASSIC_SERVERS.get_or_init(|| Mutex::new(load_reality_classic_cache()))
}

/// In-flight REALITY fallback probes, keyed by `server:port`.
///
/// The classic-servers cache above only helps *after* a probe has completed.
/// A burst of concurrent connections to an unseen server (e.g. a latency
/// check over the whole group) would otherwise each run the failing hybrid
/// handshake before any of them got far enough to write the cache — the log
/// showed the same node falling back 3–5 times in a row. Sharing a per-server
/// `Notify` makes the first connection the only one that probes; the rest
/// wait for its outcome and then go straight to the winning variant.
static REALITY_PROBE_INFLIGHT: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Notify>>>> =
    OnceLock::new();

fn reality_probe_inflight() -> &'static Mutex<HashMap<String, Arc<tokio::sync::Notify>>> {
    REALITY_PROBE_INFLIGHT.get_or_init(|| Mutex::new(HashMap::new()))
}

fn server_needs_classic_reality(server: &str, port: u16) -> bool {
    reality_classic_servers()
        .lock()
        .map(|set| set.contains(&format!("{}:{}", server, port)))
        .unwrap_or(false)
}

fn mark_server_needs_classic_reality(server: &str, port: u16) {
    let key = format!("{}:{}", server, port);
    if let Ok(mut set) = reality_classic_servers().lock()
        && set.insert(key)
    {
        // Persist so the next daemon start (and Merlin API probe process)
        // skips hybrid→classic discovery — fewer double handshakes and
        // fewer false timeouts under concurrent Test-all.
        persist_reality_classic_cache(&set);
    }
}

/// Outcome of [`begin_reality_probe`]: whether this caller should run the
/// hybrid handshake itself or wait for another connection's probe.
enum RealityProbe {
    /// No probe is running for this server: proceed with the hybrid handshake.
    /// The returned `Arc<Notify>` must be passed to [`finish_reality_probe`]
    /// once the outcome (classic needed or not) is known.
    Leader(Arc<tokio::sync::Notify>),
    /// Another connection is already probing this server.
    Follower(Arc<tokio::sync::Notify>),
}

/// Register interest in probing `server:port`. The first caller becomes the
/// leader; concurrent callers become followers that must wait on the notify.
fn begin_reality_probe(server: &str, port: u16) -> RealityProbe {
    let key = format!("{}:{}", server, port);
    let mut map = match reality_probe_inflight().lock() {
        Ok(m) => m,
        Err(_) => {
            // Lock poisoned: fall back to every-connection-probes behaviour
            // rather than panicking in the hot path.
            return RealityProbe::Leader(Arc::new(tokio::sync::Notify::new()));
        }
    };
    match map.entry(key) {
        std::collections::hash_map::Entry::Occupied(e) => RealityProbe::Follower(e.get().clone()),
        std::collections::hash_map::Entry::Vacant(e) => {
            let notify = Arc::new(tokio::sync::Notify::new());
            e.insert(notify.clone());
            RealityProbe::Leader(notify)
        }
    }
}

/// Mark the probe for `server:port` complete, waking every follower. When
/// `classic` is true the server is also recorded in the classic cache.
fn finish_reality_probe(server: &str, port: u16, notify: Arc<tokio::sync::Notify>, classic: bool) {
    if classic {
        mark_server_needs_classic_reality(server, port);
    }
    let key = format!("{}:{}", server, port);
    if let Ok(mut map) = reality_probe_inflight().lock() {
        // Only remove if it is still *this* probe's entry (a follower that
        // timed out and became a new leader may have replaced it).
        if map.get(&key).is_some_and(|n| Arc::ptr_eq(n, &notify)) {
            map.remove(&key);
        }
    }
    notify.notify_waiters();
}

/// Resolve a proxy server hostname to IP addresses, using a process-wide
/// 60s cache. Returns `None` on any failure (callers fall back to the
/// uncached path).
pub(crate) async fn resolve_server_ips(host: &str) -> Option<Vec<IpAddr>> {
    // Node configs may carry bracketed IPv6 literals (`[2001:db8::1]` from
    // url-crate parsing); normalize before the literal check and lookup.
    let host = host.trim_matches(['[', ']']);
    if host.parse::<IpAddr>().is_ok() {
        return None; // literal IPs don't need caching
    }
    let cache = server_addr_cache();
    let now = Instant::now();
    if let Some(ips) = cache.get(host, now) {
        return Some(ips);
    }
    // Bootstrap through domestic resolvers FIRST (see
    // `resolve_via_bootstrap_dns`): on the router the system resolver is
    // sockrocket's own pipeline, so asking it for the tunnel's own server hostname
    // is circular. In desktop TUN mode (`outbound_bind` active) direct UDP
    // would loop into the TUN device, so keep the system resolver there.
    let resolved = if super::outbound_bind::outbound_bind().is_some() {
        resolve_via_system(host).await
    } else {
        match resolve_via_bootstrap_dns(host).await {
            Some(ips) => Some(ips),
            None => resolve_via_system(host).await,
        }
    };
    match resolved {
        Some(ips) => {
            cache.insert(host, ips.clone(), Instant::now());
            Some(ips)
        }
        None => None,
    }
}

/// Resolve via the OS resolver (`/etc/resolv.conf`). On the router that file
/// points at the local dnsmasq → sockrocket's own DNS listener, so this is only
/// safe as a bootstrap fallback (or when no proxy pipeline is involved).
async fn resolve_via_system(host: &str) -> Option<Vec<IpAddr>> {
    match tokio::net::lookup_host((host, 0u16)).await {
        Ok(addrs) => {
            let ips: Vec<IpAddr> = addrs.map(|a| a.ip()).collect();
            if ips.is_empty() { None } else { Some(ips) }
        }
        Err(e) => {
            tracing::debug!("server DNS lookup for {} failed: {}", host, e);
            None
        }
    }
}

/// Bootstrap DNS resolver for proxy-server hostnames: domestic UDP resolvers
/// queried directly, bypassing the OS resolver entirely.
///
/// Why this exists: on the router (merlin) the system resolver IS sockrocket's own
/// pipeline — /etc/resolv.conf → dnsmasq → sockrocket:5300 — and the international
/// group there resolves through the proxy node (DNS-over-TCP). Asking that
/// pipeline for the proxy server's *own* hostname is circular: the lookup
/// needs the tunnel, the tunnel needs the lookup, and a cold start spins
/// until every layer times out (the node can never connect, so no DNS path
/// ever recovers). Node servers are by definition directly reachable, so
/// bootstrap them via domestic resolvers — direct, fast, and closer to the
/// CDN edges the nodes sit behind. Any failure falls back to the system
/// resolver (previous behaviour), which keeps non-CN desktops working.
fn bootstrap_dns_resolver() -> &'static crate::dns::DnsResolver {
    static RESOLVER: OnceLock<crate::dns::DnsResolver> = OnceLock::new();
    RESOLVER.get_or_init(|| {
        crate::dns::DnsResolver::with_config(crate::dns::DnsConfig {
            servers: vec![
                crate::dns::DnsServer::Udp("223.5.5.5:53".parse().unwrap()),
                crate::dns::DnsServer::Udp("119.29.29.29:53".parse().unwrap()),
            ],
            fallback: Vec::new(),
            domain_rules: Default::default(),
            cache_ttl: 0,
            // ServerAddrCache (60s) already memoizes; a second cache layer
            // here would only lengthen the stale window.
            cache_enabled: false,
        })
    })
}

/// Resolve `host` via the bootstrap resolver (A and AAAA concurrently).
/// Returns None when both families fail or come back empty.
async fn resolve_via_bootstrap_dns(host: &str) -> Option<Vec<IpAddr>> {
    let resolver = bootstrap_dns_resolver();
    let (a, aaaa) = tokio::join!(
        resolver.resolve_with_qtype(host, crate::dns::QTYPE_A),
        resolver.resolve_with_qtype(host, crate::dns::QTYPE_AAAA),
    );
    let mut ips: Vec<IpAddr> = Vec::new();
    ips.extend(a.into_iter().flatten());
    ips.extend(aaaa.into_iter().flatten());
    if ips.is_empty() {
        tracing::debug!("bootstrap DNS lookup for {} failed or empty", host);
        None
    } else {
        Some(ips)
    }
}

/// Build a reusable TLS connector from config. Call once per outbound
/// and store the result — avoids rebuilding root cert store and
/// ClientConfig on every connection.
pub fn build_tls_connector(config: Option<&TlsConfig>) -> TlsConnector {
    let skip_verify = config.map(|c| c.skip_cert_verify).unwrap_or(false);
    let alpn = config.and_then(|c| c.alpn.as_ref());
    let fp_name = config.and_then(|c| c.fingerprint.as_deref());

    let base_config = if skip_verify {
        ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(InsecureVerifier))
            .with_no_client_auth()
    } else {
        ClientConfig::builder()
            .with_root_certificates(get_root_cert_store().clone())
            .with_no_client_auth()
    };

    let fp = if alpn.is_some() {
        fingerprint_builder(fp_name).do_not_override_alpn()
    } else {
        fingerprint_builder(fp_name)
    };
    let mut tls_config = base_config.with_fingerprint(fp);

    if let Some(alpn) = alpn {
        tls_config.alpn_protocols = alpn.iter().map(|s| s.as_bytes().to_vec()).collect();
    }

    TlsConnector::from(Arc::new(tls_config))
}

/// Build a TLS connector for the REALITY protocol.
///
/// Uses a special X25519 key exchange group that retains the ephemeral
/// private key (required to derive the REALITY auth key), disables session
/// resumption, and lets craftls perform the REALITY session-id camouflage
/// and server certificate HMAC verification.
pub fn build_reality_tls_connector(
    reality: &RealityConfig,
    tls: Option<&TlsConfig>,
    force_classic: bool,
) -> Result<TlsConnector> {
    use base64::Engine;
    use craft_tls::rustls::reality::{
        RealityConfig as CraftRealityConfig, RealityX25519KxGroup, RealityX25519MlKem768Group,
    };

    // The public key is URL-safe base64 without padding (Xray format).
    // A malformed node config must surface as an error, not panic the process.
    let public_key: [u8; 32] = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(reality.public_key.as_bytes())
        .or_else(|_| {
            base64::engine::general_purpose::STANDARD.decode(reality.public_key.as_bytes())
        })
        .ok()
        .and_then(|v| <[u8; 32]>::try_from(v.as_slice()).ok())
        .context("invalid REALITY public key (expected base64 of 32 bytes)")?;
    let short_id: Vec<u8> = hex::decode(reality.short_id.as_bytes())
        .context("invalid REALITY short id (expected hex)")?;

    let fp_name = tls.and_then(|c| c.fingerprint.as_deref());

    // REALITY key shares: hybrid X25519MLKEM768 first (new servers demand it
    // before any classic share), then classic X25519 (old servers only
    // understand this one; xray's auth prefers the classic share when both
    // are present). Both groups retain their private keys for auth-key
    // derivation; craftls emits both in the ClientHello (see hs.rs).
    // `force_classic` emits a classic-only CH for pre-MLKEM servers whose
    // uTLS cannot parse the hybrid group at all (used as a one-shot retry
    // after a detected fallback).
    let mut provider = craft_tls::rustls::crypto::ring::default_provider();
    provider.kx_groups = if force_classic {
        vec![&RealityX25519KxGroup]
    } else {
        vec![&RealityX25519MlKem768Group, &RealityX25519KxGroup]
    };

    let base_config = ClientConfig::builder_with_provider(provider.into())
        .with_safe_default_protocol_versions()
        .expect("protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(InsecureVerifier))
        .with_no_client_auth();

    // The built-in fingerprints only know classic key shares; let rustls
    // emit the native key share (our hybrid group) instead.
    let mut tls_config = base_config
        .with_fingerprint(fingerprint_builder(fp_name).dangerous_disable_override_keyshare());
    tls_config.reality = Some(Arc::new(CraftRealityConfig {
        public_key,
        short_id,
    }));
    // REALITY cannot resume sessions: a resumption PSK binder is computed
    // over the pre-patch ClientHello and the session id must stay unique.
    tls_config.resumption.store = Arc::new(craft_tls::rustls::client::NoClientSessionStorage);

    Ok(TlsConnector::from(Arc::new(tls_config)))
}

/// Connect to a remote server with TLS using a pre-built connector.
pub async fn connect_tls_with(
    tcp: TcpStream,
    sni: &str,
    connector: &TlsConnector,
) -> Result<TlsStream<TcpStream>> {
    let server_name = ServerName::try_from(sni.to_string())?;
    let stream = connector
        .connect(server_name, tcp)
        .await
        .with_context(|| format!("TLS handshake failed with SNI '{}'", sni))?;
    Ok(stream)
}

/// Connect to a remote server with TLS using browser fingerprint emulation.
pub async fn connect_tls(
    tcp: TcpStream,
    sni: &str,
    config: Option<&TlsConfig>,
) -> Result<TlsStream<TcpStream>> {
    let connector = build_tls_connector(config);
    connect_tls_with(tcp, sni, &connector).await
}

/// Connect to a remote server over TCP, optionally wrapping with TLS.
/// Returns a boxed ProxyStream.
///
/// Applies a connection timeout (default 15s) to both the TCP connect
/// and the TLS handshake to prevent indefinite hangs.
pub async fn connect_with_tls(
    server: &str,
    port: u16,
    tls_config: Option<&TlsConfig>,
    use_tls: bool,
) -> Result<Box<dyn super::connector::ProxyStream>> {
    connect_with_tls_timeout(server, port, tls_config, use_tls, DEFAULT_CONNECT_TIMEOUT).await
}

/// Like [`connect_with_tls`] but with a pre-built TLS connector for
/// connection reuse (avoids rebuilding root certs + fingerprints per call).
pub async fn connect_with_connector(
    server: &str,
    port: u16,
    connector: &TlsConnector,
    sni: &str,
    use_tls: bool,
    timeout: Duration,
) -> Result<Box<dyn super::connector::ProxyStream>> {
    let addr = format_host_port(server, port);
    let tcp = connect_tcp(&addr, timeout).await?;

    if use_tls {
        let tls = tokio::time::timeout(timeout, connect_tls_with(tcp, sni, connector))
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "TLS handshake with {} (SNI: {}) timed out after {}s",
                    addr,
                    sni,
                    timeout.as_secs()
                )
            })??;
        Ok(Box::new(tls))
    } else {
        Ok(Box::new(tcp))
    }
}

/// Like [`connect_with_tls`] but with an explicit timeout.
pub async fn connect_with_tls_timeout(
    server: &str,
    port: u16,
    tls_config: Option<&TlsConfig>,
    use_tls: bool,
    timeout: Duration,
) -> Result<Box<dyn super::connector::ProxyStream>> {
    let addr = format_host_port(server, port);
    let tcp = connect_tcp(&addr, timeout).await?;

    if use_tls {
        let sni = tls_config.and_then(|c| c.sni.as_deref()).unwrap_or(server);
        let tls = tokio::time::timeout(timeout, connect_tls(tcp, sni, tls_config))
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "TLS handshake with {} (SNI: {}) timed out after {}s",
                    addr,
                    sni,
                    timeout.as_secs()
                )
            })??;
        Ok(Box::new(tls))
    } else {
        Ok(Box::new(tcp))
    }
}

/// Certificate verifier that accepts all certificates (for skip_cert_verify).
#[derive(Debug)]
pub(crate) struct InsecureVerifier;

impl ServerCertVerifier for InsecureVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ECDSA_NISTP521_SHA512,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ED25519,
            SignatureScheme::ED448,
        ]
    }
}

// --- Shared helpers for TCP+TLS outbounds ---

/// Resolve TLS config and TLS-enable flag from a transport config.
///
/// Handles the Reality special case: when `transport_type == Reality` and
/// no explicit `tls` block is present, synthesises a TlsConfig from the
/// Reality settings (SNI + skip_cert_verify).
pub fn resolve_transport_tls(
    transport: Option<&TransportConfig>,
    server: &str,
) -> (Option<TlsConfig>, bool) {
    let Some(t) = transport else {
        return (None, false);
    };
    // TLS is enabled either by the declared transport type (Tls/Reality) or
    // by the presence of an explicit TLS block — e.g. "ws over tls" nodes
    // carry type=websocket plus a tls config, and must not be silently
    // downgraded to plaintext.
    let use_tls = matches!(
        t.transport_type,
        TransportType::Tls | TransportType::Reality
    ) || t.tls.is_some();
    let tls = if t.transport_type == TransportType::Reality && t.tls.is_none() {
        if let Some(ref r) = t.reality {
            tracing::debug!(
                "Reality transport: SNI '{}', public_key {}...",
                r.sni.as_deref().unwrap_or(server),
                &r.public_key[..r.public_key.len().min(12)]
            );
            Some(TlsConfig {
                sni: r.sni.clone(),
                skip_cert_verify: true,
                alpn: None,
                fingerprint: None,
            })
        } else {
            t.tls.clone()
        }
    } else {
        t.tls.clone()
    };
    (tls, use_tls)
}

/// Shared connection factory for TLS-based outbounds (VLESS, VMess, Trojan).
///
/// Replaces the per-protocol `XxxConnFactory` boilerplate. Build once in
/// `Outbound::new()` and store inside a `ConnPool`.
pub struct TlsConnFactory {
    server: String,
    port: u16,
    sni: String,
    use_tls: bool,
    connector: TlsConnector,
    /// Separate connector performing the REALITY handshake, if applicable.
    reality_connector: Option<TlsConnector>,
    /// Classic-X25519-only REALITY connector, used as a one-shot retry when
    /// the dual-share handshake landed on the camouflage target (pre-MLKEM
    /// servers cannot parse the hybrid group and silently forward us).
    reality_classic_connector: Option<TlsConnector>,
    /// Set when building the REALITY connector failed (bad public_key /
    /// short_id in the node config). Surfaced on every `create()` call
    /// instead of panicking at construction time.
    reality_error: Option<String>,
    /// WebSocket layer config; set when the transport is ws. The WS
    /// handshake runs on top of TCP/TLS/Reality inside `create()`.
    ws: Option<WsConfig>,
}

impl TlsConnFactory {
    /// Create from an optional TLS config.  SNI falls back to `server` if not set.
    /// When `reality` is given, builds a dedicated REALITY connector.
    pub fn new(
        server: &str,
        port: u16,
        tls_config: Option<&TlsConfig>,
        use_tls: bool,
        reality: Option<&RealityConfig>,
    ) -> Self {
        Self::build(server, port, tls_config, use_tls, reality, None)
    }

    /// Build from a full transport config: resolves TLS (including the
    /// ws+tls combination and Reality) and attaches the WebSocket layer when
    /// `transport_type == WebSocket`. `force_tls` is for protocols that are
    /// always TLS-wrapped (Trojan).
    pub fn from_transport(
        server: &str,
        port: u16,
        transport: Option<&TransportConfig>,
        force_tls: bool,
    ) -> Self {
        let (tls_config, resolved_tls) = resolve_transport_tls(transport, server);
        let ws = match transport {
            Some(t) if t.transport_type == TransportType::WebSocket => {
                // Nodes parsed before ws settings were captured carry
                // `ws: null` — fall back to the defaults (path "/", Host=server).
                Some(t.ws.clone().unwrap_or(WsConfig {
                    path: None,
                    host: None,
                    headers: None,
                }))
            }
            _ => None,
        };
        Self::build(
            server,
            port,
            tls_config.as_ref(),
            force_tls || resolved_tls,
            transport.and_then(|t| t.reality.as_ref()),
            ws,
        )
    }

    fn build(
        server: &str,
        port: u16,
        tls_config: Option<&TlsConfig>,
        use_tls: bool,
        reality: Option<&RealityConfig>,
        ws: Option<WsConfig>,
    ) -> Self {
        let sni = tls_config
            .and_then(|c| c.sni.as_deref())
            .unwrap_or(server)
            .to_string();
        // WebSocket runs over HTTP/1.1: unless the user pinned an ALPN list,
        // stop the browser fingerprint from offering h2 — an h2-negotiating
        // edge (Gcore/nginx) would answer our HTTP/1.1 upgrade with HTTP/2
        // frames and the handshake dies.
        let ws_tls;
        let tls_config = match (ws.is_some(), tls_config) {
            (true, Some(c)) if c.alpn.is_none() => {
                let mut cloned = c.clone();
                cloned.alpn = Some(vec!["http/1.1".to_string()]);
                ws_tls = Some(cloned);
                ws_tls.as_ref()
            }
            _ => tls_config,
        };
        let (reality_connector, reality_classic_connector, reality_error) = match reality {
            Some(r) => match build_reality_tls_connector(r, tls_config, false) {
                Ok(c) => {
                    let classic = build_reality_tls_connector(r, tls_config, true).ok();
                    (Some(c), classic, None)
                }
                Err(e) => (None, None, Some(format!("{e:#}"))),
            },
            None => (None, None, None),
        };
        Self {
            server: server.to_string(),
            port,
            sni,
            use_tls,
            connector: build_tls_connector(tls_config),
            reality_connector,
            reality_classic_connector,
            reality_error,
            ws,
        }
    }
}

impl ConnFactory for TlsConnFactory {
    fn create(&self) -> Pin<Box<dyn Future<Output = anyhow::Result<BoxProxyStream>> + Send + '_>> {
        Box::pin(async {
            if let Some(err) = &self.reality_error {
                return Err(anyhow::anyhow!("REALITY configuration error: {}", err));
            }
            let can_retry_classic = self.reality_classic_connector.is_some();

            // Wait briefly for an in-flight probe on this server so a burst of
            // concurrent latency checks share one hybrid→classic discovery.
            // Cap the wait low: callers (GUI/Merlin http_latency_test) often wrap
            // connect() in a 5–10s timeout — a 20s wait here made every follower
            // look like a hard timeout ("most nodes unreachable") during Test-all.
            if can_retry_classic
                && !server_needs_classic_reality(&self.server, self.port)
                && let RealityProbe::Follower(notify) = begin_reality_probe(&self.server, self.port)
            {
                let _ = tokio::time::timeout(Duration::from_secs(2), notify.notified()).await;
            }

            // If this server already proved it needs the classic key share, skip
            // the handshake that is known to fail and go straight to it.
            let prefer_classic =
                can_retry_classic && server_needs_classic_reality(&self.server, self.port);
            let connector = if prefer_classic {
                self.reality_classic_connector
                    .as_ref()
                    .expect("checked above")
            } else {
                self.reality_connector.as_ref().unwrap_or(&self.connector)
            };
            // Register as the leader when we are about to run the probe
            // ourselves (hybrid, server not yet classified). `probe_notify`
            // stays `Some` until the outcome is recorded, so every exit path
            // out of this scope wakes the followers exactly once.
            let mut probe_notify = if can_retry_classic && !prefer_classic {
                match begin_reality_probe(&self.server, self.port) {
                    RealityProbe::Leader(n) => Some(n),
                    // Lost the race with a just-finished probe's wake-up:
                    // proceed without registering; the cache check above has
                    // already seen the latest outcome.
                    RealityProbe::Follower(_) => None,
                }
            } else {
                None
            };
            let stream = connect_with_connector(
                &self.server,
                self.port,
                connector,
                &self.sni,
                self.use_tls,
                Duration::from_secs(15),
            )
            .await;
            let stream = match stream {
                Ok(s) => s,
                Err(e)
                    if can_retry_classic && format!("{e:#}").contains("Ed25519 SPKI not found") =>
                {
                    // The server forwarded us to the camouflage target: it
                    // cannot parse the hybrid key share (pre-MLKEM xray).
                    // Retry once with a classic-X25519-only ClientHello, and
                    // remember it so the next connection to this server skips
                    // the doomed first attempt entirely.
                    tracing::info!(
                        "REALITY fallback detected for {}:{}, retrying with classic X25519 share",
                        self.server,
                        self.port
                    );
                    if let Some(n) = probe_notify.take() {
                        finish_reality_probe(&self.server, self.port, n, true);
                    } else {
                        mark_server_needs_classic_reality(&self.server, self.port);
                    }
                    let retried = connect_with_connector(
                        &self.server,
                        self.port,
                        self.reality_classic_connector.as_ref().expect("checked"),
                        &self.sni,
                        self.use_tls,
                        Duration::from_secs(15),
                    )
                    .await;
                    // A classic retry that fails leaves the server classified
                    // as classic (a transient error must not flip it back).
                    // The wake-up already happened through finish_reality_probe,
                    // so followers re-check the cache and go straight to classic.
                    match retried {
                        Ok(s) => s,
                        Err(e) => return Err(e),
                    }
                }
                Err(e) => {
                    // The hybrid attempt failed for a non-fallback reason
                    // (timeout, refused, …). Wake followers with no cache
                    // entry so they run their own probe instead of hanging.
                    if let Some(n) = probe_notify.take() {
                        finish_reality_probe(&self.server, self.port, n, false);
                    }
                    return Err(e);
                }
            };
            // The hybrid handshake got past the REALITY check: this server is
            // fine with the default share, so no classic entry is recorded.
            if let Some(n) = probe_notify.take() {
                finish_reality_probe(&self.server, self.port, n, false);
            }
            if let Some(ws) = &self.ws {
                super::ws::connect_ws_timeout(stream, &self.server, ws, Duration::from_secs(15))
                    .await
            } else {
                Ok(stream)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reality_bad_public_key_returns_error_not_panic() {
        let reality = RealityConfig {
            public_key: "!!!not-base64!!!".to_string(),
            short_id: "abcd".to_string(),
            sni: None,
            client_fingerprint: None,
            skip_cert_verify: false,
        };
        let err = match build_reality_tls_connector(&reality, None, false) {
            Err(e) => e,
            Ok(_) => panic!("bad public key must be an error, not a connector"),
        };
        assert!(
            format!("{err:#}").contains("REALITY public key"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn reality_bad_short_id_returns_error_not_panic() {
        use base64::Engine;
        let reality = RealityConfig {
            public_key: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([7u8; 32]),
            short_id: "zz-not-hex".to_string(),
            sni: None,
            client_fingerprint: None,
            skip_cert_verify: false,
        };
        let err = match build_reality_tls_connector(&reality, None, false) {
            Err(e) => e,
            Ok(_) => panic!("bad short id must be an error, not a connector"),
        };
        assert!(
            format!("{err:#}").contains("REALITY short id"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn server_addr_cache_hit_and_expiry() {
        let cache = ServerAddrCache::new();
        let ips = vec![IpAddr::V4(std::net::Ipv4Addr::new(93, 184, 216, 34))];
        let t0 = Instant::now();

        cache.insert("node.example.com", ips.clone(), t0);

        // Fresh entry is returned.
        assert_eq!(cache.get("node.example.com", t0), Some(ips.clone()));
        assert_eq!(
            cache.get("node.example.com", t0 + Duration::from_secs(59)),
            Some(ips.clone())
        );

        // Past the TTL the entry is treated as expired.
        assert_eq!(
            cache.get("node.example.com", t0 + Duration::from_secs(61)),
            None
        );

        // Unknown host misses.
        assert_eq!(cache.get("other.example.com", t0), None);
    }

    #[test]
    fn server_addr_cache_stores_ipv6() {
        let cache = ServerAddrCache::new();
        let ips = vec![
            IpAddr::V4(std::net::Ipv4Addr::new(93, 184, 216, 34)),
            IpAddr::V6("2001:db8::1".parse().unwrap()),
        ];
        let t0 = Instant::now();

        cache.insert("dual.example.com", ips.clone(), t0);
        let got = cache.get("dual.example.com", t0).unwrap();
        assert_eq!(got, ips);
        assert!(got.iter().any(|ip| ip.is_ipv6()));
    }

    #[tokio::test]
    async fn resolve_server_ips_passthrough_for_ip_literal() {
        // Literal IPs are not cached; the caller connects directly.
        assert_eq!(resolve_server_ips("127.0.0.1").await, None);
        // Bracketed and bare IPv6 literals take the same passthrough path.
        assert_eq!(resolve_server_ips("::1").await, None);
        assert_eq!(resolve_server_ips("[::1]").await, None);
        assert_eq!(resolve_server_ips("[2001:db8::1]").await, None);
    }

    #[test]
    fn format_host_port_brackets_ipv6() {
        // Bare IPv6 literal (VMess JSON `add` field form).
        assert_eq!(format_host_port("2001:db8::1", 443), "[2001:db8::1]:443");
        // Bracketed IPv6 literal (url-crate `host_str` form) is normalized.
        assert_eq!(format_host_port("[2001:db8::1]", 443), "[2001:db8::1]:443");
        assert_eq!(format_host_port("::1", 1080), "[::1]:1080");
        // IPv4 and domains pass through unchanged.
        assert_eq!(format_host_port("127.0.0.1", 443), "127.0.0.1:443");
        assert_eq!(
            format_host_port("node.example.com", 443),
            "node.example.com:443"
        );
    }

    #[test]
    fn resolve_transport_tls_ws_with_tls_block_enables_tls() {
        // ws+tls node: type=websocket plus an explicit tls config must NOT
        // be silently downgraded to plaintext TCP.
        let t = TransportConfig {
            transport_type: TransportType::WebSocket,
            tls: Some(TlsConfig {
                sni: Some("cdn.example.com".to_string()),
                skip_cert_verify: true,
                alpn: None,
                fingerprint: None,
            }),
            ws: None,
            reality: None,
        };
        let (tls, use_tls) = resolve_transport_tls(Some(&t), "server.com");
        assert!(use_tls, "ws+tls must enable TLS");
        assert_eq!(tls.unwrap().sni.as_deref(), Some("cdn.example.com"));
    }

    #[test]
    fn resolve_transport_tls_ws_without_tls_stays_plaintext() {
        let t = TransportConfig {
            transport_type: TransportType::WebSocket,
            tls: None,
            ws: None,
            reality: None,
        };
        let (tls, use_tls) = resolve_transport_tls(Some(&t), "server.com");
        assert!(!use_tls);
        assert!(tls.is_none());
    }

    /// End-to-end wiring check: TlsConnFactory::from_transport with a ws
    /// transport must run the WS handshake inside create(), and the
    /// resulting stream must carry framed bytes both ways.
    #[tokio::test]
    async fn factory_ws_transport_handshakes_and_carries_bytes() {
        use base64::Engine as _;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let server_task = tokio::spawn(async move {
            let (mut conn, _) = listener.accept().await.unwrap();
            // Read the HTTP Upgrade request.
            let mut buf = Vec::new();
            let mut chunk = [0u8; 256];
            let head = loop {
                let n = conn.read(&mut chunk).await.unwrap();
                buf.extend_from_slice(&chunk[..n]);
                if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break String::from_utf8_lossy(&buf[..pos]).into_owned();
                }
            };
            assert!(head.starts_with("GET /ws-path HTTP/1.1\r\n"), "got: {head}");
            assert!(head.contains("Host: cdn.example.com\r\n"), "got: {head}");
            let key = head
                .split("\r\n")
                .find_map(|l| {
                    l.split_once(':').and_then(|(k, v)| {
                        k.trim()
                            .eq_ignore_ascii_case("sec-websocket-key")
                            .then(|| v.trim().to_string())
                    })
                })
                .unwrap();
            let mut hasher = sha1::Sha1::new();
            use sha1::Digest as _;
            hasher.update(key.as_bytes());
            hasher.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
            let accept = base64::engine::general_purpose::STANDARD.encode(hasher.finalize());
            conn.write_all(
                format!(
                    "HTTP/1.1 101 Switching Protocols\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();

            // Echo: read one masked frame, reply with the payload unmasked.
            let mut hdr = [0u8; 2];
            conn.read_exact(&mut hdr).await.unwrap();
            assert_eq!(hdr[0], 0x82, "expected FIN+binary frame");
            assert!(hdr[1] & 0x80 != 0, "client frames must be masked");
            let len = (hdr[1] & 0x7f) as usize;
            assert!(len < 126);
            let mut mask = [0u8; 4];
            conn.read_exact(&mut mask).await.unwrap();
            let mut payload = vec![0u8; len];
            conn.read_exact(&mut payload).await.unwrap();
            for (i, b) in payload.iter_mut().enumerate() {
                *b ^= mask[i % 4];
            }
            let mut frame = vec![0x82u8, len as u8];
            frame.extend_from_slice(&payload);
            conn.write_all(&frame).await.unwrap();
        });

        let transport = TransportConfig {
            transport_type: TransportType::WebSocket,
            tls: None,
            ws: Some(WsConfig {
                path: Some("/ws-path".to_string()),
                host: Some("cdn.example.com".to_string()),
                headers: None,
            }),
            reality: None,
        };
        let factory = TlsConnFactory::from_transport("127.0.0.1", port, Some(&transport), false);
        let mut stream = factory.create().await.expect("ws connect must succeed");

        stream.write_all(b"vmess-bytes-here").await.unwrap();
        let mut got = [0u8; 16];
        stream.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"vmess-bytes-here");

        server_task.await.unwrap();
    }
}

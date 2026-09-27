//! Shared QUIC/TLS infrastructure for TUIC and Hysteria2.
//!
//! Centralises the common QUIC connection management, TLS configuration,
//! and DNS resolution logic shared across QUIC-based proxy protocols.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::{Arc, Once};

use anyhow::Result;

/// Hard budget for a single connection-setup attempt inside the single-flight
/// critical section (QUIC handshake + protocol auth + bootstrap DNS).
const CONNECT_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

/// Single-flight cache for an expensive-to-create, cloneable value (typically
/// a QUIC connection tuple).
///
/// Concurrent `get_or_connect` calls race on the fast path read; the first to
/// miss takes an exclusive lock, re-checks under the lock, and only then runs
/// the (potentially slow) `connect` closure. Every loser either observes the
/// winner's value or — while still holding the lock — runs `connect` itself,
/// so at most one extra redundant connection is ever created, never N.
pub(crate) struct SingleFlight<V> {
    state: tokio::sync::RwLock<Option<V>>,
    lock: tokio::sync::Mutex<()>,
}

impl<V: Clone> SingleFlight<V> {
    fn new() -> Self {
        Self {
            state: tokio::sync::RwLock::new(None),
            lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Fast path: return the cached value if it is live.
    async fn get_existing(&self, is_live: impl Fn(&V) -> bool) -> Option<V> {
        let guard = self.state.read().await;
        guard.as_ref().filter(|v| is_live(v)).cloned()
    }

    /// Get the cached value, or create it exactly once even under concurrency.
    ///
    /// `is_live` decides whether a cached value may still be used; `connect`
    /// performs the (slow) creation and must return the new value on success.
    async fn get_or_connect<L, C, Fut>(&self, is_live: L, connect: C) -> Result<V>
    where
        L: Fn(&V) -> bool,
        C: FnOnce() -> Fut,
        Fut: Future<Output = Result<V>>,
    {
        if let Some(v) = self.get_existing(&is_live).await {
            return Ok(v);
        }
        // Serialize slow-path creation: concurrent callers that all missed the
        // fast path must not each open their own connection.
        let _guard = self.lock.lock().await;
        if let Some(v) = self.get_existing(&is_live).await {
            return Ok(v);
        }
        // Hard bound on connection setup while the single-flight lock is held:
        // a hung handshake (dead network path, unresponsive peer) must not
        // wedge every future caller of this node behind the mutex. On timeout
        // nothing is cached, so the next caller retries from scratch.
        let new_value = tokio::time::timeout(CONNECT_BUDGET, connect())
            .await
            .map_err(|_| {
                anyhow::anyhow!("connection setup hung beyond {CONNECT_BUDGET:?} budget")
            })??;
        let mut guard = self.state.write().await;
        if let Some(existing) = guard.as_ref().filter(|v| is_live(v)) {
            return Ok(existing.clone());
        }
        *guard = Some(new_value.clone());
        Ok(new_value)
    }
}

/// Cached QUIC connection state (endpoint + connection pair).
pub struct QuicConnectionState {
    inner: SingleFlight<(quinn::Endpoint, quinn::Connection)>,
}

impl QuicConnectionState {
    pub fn new() -> Self {
        Self {
            inner: SingleFlight::new(),
        }
    }

    fn conn_is_live((_, conn): &(quinn::Endpoint, quinn::Connection)) -> bool {
        conn.close_reason().is_none()
    }

    /// Get a live cached connection, or establish one (serialized so that
    /// concurrent callers share a single QUIC session).
    ///
    /// `connect` must perform connection setup *and* any protocol-level
    /// authentication; it is only invoked when no live connection is cached.
    pub async fn get_or_connect<C, Fut>(&self, connect: C) -> Result<quinn::Connection>
    where
        C: FnOnce() -> Fut,
        Fut: Future<Output = Result<(quinn::Endpoint, quinn::Connection)>>,
    {
        // The endpoint stays alive inside the cache; the local binding is
        // just the destructure of the cached tuple.
        let (_endpoint, conn) = self
            .inner
            .get_or_connect(Self::conn_is_live, connect)
            .await?;
        Ok(conn)
    }
}

impl Default for QuicConnectionState {
    fn default() -> Self {
        Self::new()
    }
}

/// Ensure the rustls ring crypto provider is installed (idempotent).
pub fn ensure_quic_crypto_provider() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Globally cached root cert store for QUIC TLS (built once).
static QUIC_ROOT_CERTS: std::sync::OnceLock<rustls::RootCertStore> = std::sync::OnceLock::new();

pub fn quic_root_certs() -> &'static rustls::RootCertStore {
    QUIC_ROOT_CERTS.get_or_init(|| {
        let mut store = rustls::RootCertStore::empty();
        store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        store
    })
}

/// Build a `quinn::ClientConfig` with TLS settings (reusable across reconnections).
pub fn build_quic_client_config(
    skip_cert_verify: bool,
    alpn: &[String],
) -> Result<quinn::ClientConfig> {
    let mut rustls_config = if skip_cert_verify {
        rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(InsecureQuicVerifier))
            .with_no_client_auth()
    } else {
        rustls::ClientConfig::builder()
            .with_root_certificates(quic_root_certs().clone())
            .with_no_client_auth()
    };

    if !alpn.is_empty() {
        rustls_config.alpn_protocols = alpn.iter().map(|s| s.as_bytes().to_vec()).collect();
    }

    let quic_config = quinn::crypto::rustls::QuicClientConfig::try_from(rustls_config)
        .map_err(|e| anyhow::anyhow!("QUIC TLS config error: {}", e))?;

    Ok(quinn::ClientConfig::new(Arc::new(quic_config)))
}

pub(crate) fn prefer_socket_addrs(mut addrs: Vec<SocketAddr>) -> Vec<SocketAddr> {
    addrs.sort_by_key(|addr| if addr.is_ipv4() { 0 } else { 1 });
    addrs.dedup();
    addrs
}

/// Resolve server hostname to `SocketAddr` candidates, preferring IPv4 first.
///
/// Uses the process-wide server-address cache (`transport::resolve_server_ips`)
/// to avoid a fresh DNS lookup on every (re)connection; falls back to a direct
/// `lookup_host` on any cache/lookup failure.
pub async fn resolve_server_addrs(server: &str, port: u16) -> Result<Vec<SocketAddr>> {
    if let Some(ips) = super::transport::resolve_server_ips(server).await {
        return Ok(prefer_socket_addrs(
            ips.into_iter()
                .map(|ip| SocketAddr::new(ip, port))
                .collect(),
        ));
    }
    use tokio::net::lookup_host;
    let addr = super::transport::format_host_port(server, port);
    let addrs: Vec<_> = lookup_host(addr).await?.collect();
    let addrs = prefer_socket_addrs(addrs);
    if addrs.is_empty() {
        anyhow::bail!("Failed to resolve {}", server);
    }
    Ok(addrs)
}

/// Insecure certificate verifier for the `skip_cert_verify` option.
#[derive(Debug)]
pub struct InsecureQuicVerifier;

impl rustls::client::danger::ServerCertVerifier for InsecureQuicVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::ED448,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn test_prefer_socket_addrs_prefers_ipv4_and_dedups() {
        let ordered = prefer_socket_addrs(vec![
            "[2001:db8::1]:443".parse().unwrap(),
            "198.51.100.10:443".parse().unwrap(),
            "198.51.100.10:443".parse().unwrap(),
            "[2001:db8::2]:443".parse().unwrap(),
        ]);

        assert_eq!(ordered.len(), 3);
        assert!(ordered[0].is_ipv4());
        assert!(ordered[1].is_ipv6());
        assert!(ordered[2].is_ipv6());
    }

    #[tokio::test]
    async fn test_singleflight_concurrent_connects_run_once() {
        let cache = Arc::new(SingleFlight::<Arc<()>>::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let mut set = tokio::task::JoinSet::new();
        for _ in 0..8 {
            let cache = Arc::clone(&cache);
            let calls = Arc::clone(&calls);
            set.spawn(async move {
                cache
                    .get_or_connect(
                        |_| true,
                        || async {
                            calls.fetch_add(1, Ordering::SeqCst);
                            // Give the other tasks time to pile up on the lock.
                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                            Ok::<_, anyhow::Error>(Arc::new(()))
                        },
                    )
                    .await
            });
        }
        let results: Vec<_> = set.join_all().await;
        let first = results[0].as_ref().unwrap().clone();
        for r in &results {
            // Every caller must observe the exact same value.
            assert!(Arc::ptr_eq(&first, r.as_ref().unwrap()));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_singleflight_reconnects_after_death() {
        let cache = SingleFlight::<usize>::new();
        let calls = AtomicUsize::new(0);
        let first = cache
            .get_or_connect(
                |v| *v > 0,
                || async {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, anyhow::Error>(1)
                },
            )
            .await
            .unwrap();
        assert_eq!(first, 1);
        // Poison the cached value: is_live now reports false.
        {
            let mut guard = cache.state.write().await;
            *guard = Some(0);
        }
        let second = cache
            .get_or_connect(
                |v| *v > 0,
                || async {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, anyhow::Error>(2)
                },
            )
            .await
            .unwrap();
        assert_eq!(second, 2);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}

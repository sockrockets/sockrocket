use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::io::{AsyncRead, AsyncWrite};

/// Trait combining AsyncRead + AsyncWrite for boxed proxy streams.
pub trait ProxyStream: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> ProxyStream for T {}

/// Boxed proxy stream for dynamic dispatch.
pub type BoxProxyStream = Box<dyn ProxyStream>;

/// Outbound connector trait. Implementations provide different ways to
/// connect to a remote target (direct, Shadowsocks, VMess, etc.).
pub trait Outbound: Send + Sync + 'static {
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>>;

    fn name(&self) -> &str;
}

/// Overall timeout for establishing an upstream connection through an
/// outbound (node handshake included). Without this a dead node lets a
/// CONNECT hang for the sum of all protocol-level retries (TUIC: 2 addrs ×
/// 2 × 7s ≈ 28s; TCP protocols under the retry wrapper: even longer) before
/// the client sees any response.
pub const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Connect through an outbound, bounded by [`UPSTREAM_CONNECT_TIMEOUT`].
pub(crate) async fn connect_upstream(
    outbound: &SharedOutbound,
    host: &str,
    port: u16,
) -> Result<BoxProxyStream> {
    tokio::time::timeout(UPSTREAM_CONNECT_TIMEOUT, outbound.connect(host, port))
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "upstream connect to {}:{} timed out after {}s",
                host,
                port,
                UPSTREAM_CONNECT_TIMEOUT.as_secs()
            )
        })?
}

/// Default timeout for direct TCP connections.
const DIRECT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Direct TCP connection (no proxy, used for testing and direct mode).
///
/// Goes through [`super::transport::connect_tcp`], so domain hosts are
/// resolved once per 60s via the process-wide server-address cache and
/// connected happy-eyeballs-style; IP literals skip resolution entirely.
pub struct DirectOutbound;

impl Outbound for DirectOutbound {
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
        let addr = super::transport::format_host_port(host, port);
        Box::pin(async move {
            let stream = super::transport::connect_tcp(&addr, DIRECT_CONNECT_TIMEOUT).await?;
            Ok(Box::new(stream) as BoxProxyStream)
        })
    }

    fn name(&self) -> &str {
        "direct"
    }
}

/// Outbound wrapper that retries failed connections with exponential backoff.
///
/// Handles transient failures (brief network blips, momentary server overload)
/// that self-resolve on retry. Default: up to 3 attempts, 200 ms → 2 s delay.
pub struct RetryOutbound {
    inner: Arc<dyn Outbound>,
    max_attempts: u32,
    base_delay_ms: u64,
    max_delay_ms: u64,
}

impl RetryOutbound {
    pub fn new(inner: Arc<dyn Outbound>) -> Self {
        Self {
            inner,
            max_attempts: 3,
            base_delay_ms: 200,
            max_delay_ms: 2000,
        }
    }
}

impl Outbound for RetryOutbound {
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
        let host = host.to_owned();
        Box::pin(async move {
            // Clamp to at least one attempt: max_attempts = 0 would otherwise
            // skip the loop entirely and panic on `last_err.unwrap()`.
            let max_attempts = self.max_attempts.max(1);
            let mut last_err = None;
            let mut delay_ms = self.base_delay_ms;
            for attempt in 0..max_attempts {
                match self.inner.connect(&host, port).await {
                    Ok(stream) => {
                        if attempt > 0 {
                            tracing::debug!(
                                "{}:{} connected on attempt {}/{}",
                                host,
                                port,
                                attempt + 1,
                                max_attempts
                            );
                        }
                        return Ok(stream);
                    }
                    Err(e) => {
                        tracing::debug!(
                            "{}:{} attempt {}/{} failed: {:#}",
                            host,
                            port,
                            attempt + 1,
                            max_attempts,
                            e
                        );
                        last_err = Some(e);
                        if attempt + 1 < max_attempts {
                            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                            delay_ms = (delay_ms * 2).min(self.max_delay_ms);
                        }
                    }
                }
            }
            Err(last_err.unwrap())
        })
    }

    fn name(&self) -> &str {
        self.inner.name()
    }
}

/// Wraps an `Arc<dyn Outbound>` for convenience cloning.
#[derive(Clone)]
pub struct SharedOutbound(pub Arc<dyn Outbound>);

/// An outbound whose inner connector can be swapped at runtime.
///
/// Servers hold a `SharedOutbound` clone of this wrapper, so calling
/// [`SwappableOutbound::set`] redirects all *new* connections to the new
/// outbound without rebinding the local listeners. In-flight connections
/// keep using the outbound they started with.
pub struct SwappableOutbound {
    inner: std::sync::RwLock<SharedOutbound>,
}

impl SwappableOutbound {
    pub fn new(outbound: SharedOutbound) -> Self {
        Self {
            inner: std::sync::RwLock::new(outbound),
        }
    }

    /// Replace the inner outbound (e.g. switch to another node).
    pub fn set(&self, outbound: SharedOutbound) {
        *self.inner.write().unwrap() = outbound;
    }

    /// Clone the current inner outbound.
    pub fn current(&self) -> SharedOutbound {
        self.inner.read().unwrap().clone()
    }
}

impl Outbound for SwappableOutbound {
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
        let inner = self.current();
        let host = host.to_owned();
        Box::pin(async move { inner.connect(&host, port).await })
    }

    fn name(&self) -> &str {
        "swappable"
    }
}

impl SharedOutbound {
    pub fn direct() -> Self {
        Self(Arc::new(DirectOutbound))
    }

    /// Wrap this outbound with automatic retry on transient failures.
    pub fn with_retry(self, max_attempts: u32) -> Self {
        Self(Arc::new(RetryOutbound {
            inner: self.0,
            max_attempts,
            base_delay_ms: 200,
            max_delay_ms: 2000,
        }))
    }

    pub async fn connect(&self, host: &str, port: u16) -> Result<BoxProxyStream> {
        self.0.connect(host, port).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    #[tokio::test]
    async fn test_direct_outbound() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 5];
            stream.read_exact(&mut buf).await.unwrap();
            stream.write_all(&buf).await.unwrap();
        });

        let outbound = DirectOutbound;
        let mut stream = outbound
            .connect(&addr.ip().to_string(), addr.port())
            .await
            .unwrap();

        stream.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 5];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");

        server.await.unwrap();
    }

    /// Outbound that always connects to one fixed local address.
    struct FixedAddrOutbound(std::net::SocketAddr);

    impl Outbound for FixedAddrOutbound {
        fn connect(
            &self,
            _host: &str,
            _port: u16,
        ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
            let addr = self.0;
            Box::pin(async move {
                let stream = TcpStream::connect(addr).await?;
                Ok(Box::new(stream) as BoxProxyStream)
            })
        }

        fn name(&self) -> &str {
            "fixed"
        }
    }

    /// Spawn an echo server; returns its address and a counter of accepted
    /// connections so tests can tell *which* server a stream landed on.
    async fn spawn_echo() -> (std::net::SocketAddr, Arc<std::sync::atomic::AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hits_clone = hits.clone();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                hits_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut buf = [0u8; 16];
                    loop {
                        match s.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                if s.write_all(&buf[..n]).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                });
            }
        });
        (addr, hits)
    }

    #[tokio::test]
    async fn test_swappable_outbound_switches_target() {
        let (addr_a, hits_a) = spawn_echo().await;
        let (addr_b, hits_b) = spawn_echo().await;

        let swappable = Arc::new(SwappableOutbound::new(SharedOutbound(Arc::new(
            FixedAddrOutbound(addr_a),
        ))));
        let shared = SharedOutbound(swappable.clone());

        // Before the swap the wrapper dials A.
        let stream = shared.connect("ignored", 1).await.unwrap();
        drop(stream);

        // Swap to B: new connections go to B without touching `shared`.
        swappable.set(SharedOutbound(Arc::new(FixedAddrOutbound(addr_b))));
        let stream = shared.connect("ignored", 1).await.unwrap();
        drop(stream);

        // Give the accept tasks a moment to register the connections.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(hits_a.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(hits_b.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// Outbound that always fails.
    struct AlwaysFailOutbound;

    impl Outbound for AlwaysFailOutbound {
        fn connect(
            &self,
            _host: &str,
            _port: u16,
        ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
            Box::pin(async { anyhow::bail!("always fails") })
        }

        fn name(&self) -> &str {
            "always-fail"
        }
    }

    /// max_attempts = 0 must not panic (previously `last_err.unwrap()` on an
    /// empty loop); it is clamped to a single attempt and returns the error.
    #[tokio::test]
    async fn test_retry_outbound_zero_attempts_does_not_panic() {
        let retry = RetryOutbound {
            inner: Arc::new(AlwaysFailOutbound),
            max_attempts: 0,
            base_delay_ms: 1,
            max_delay_ms: 1,
        };
        let result = retry.connect("127.0.0.1", 1).await;
        assert!(result.is_err());
    }
}

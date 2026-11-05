use std::collections::VecDeque;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::sync::{Mutex, Notify, OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

use super::connector::BoxProxyStream;

// Idle pre-connects are useful for bursts, not long-term server reservations.
const DEFAULT_MAX_AGE: Duration = Duration::from_secs(30);
const DEMAND_WINDOW: Duration = Duration::from_secs(1);
const FILL_CONCURRENCY: usize = 4;

struct PooledConn<S> {
    stream: S,
    created: Instant,
    _permit: OwnedSemaphorePermit,
}

pub trait ConnFactory<S = BoxProxyStream>: Send + Sync + 'static {
    fn create(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<S>> + Send + '_>>;
}

struct PoolState<S> {
    conns: VecDeque<PooledConn<S>>,
    target: usize,
    requests: usize,
    window_started: Instant,
    last_get: Instant,
}

struct PoolInner<S> {
    state: Mutex<PoolState<S>>,
    factory: Arc<dyn ConnFactory<S>>,
    base: usize,
    capacity: usize,
    budget: Arc<Semaphore>,
    refill: Notify,
}

// Background work owns only PoolInner. Dropping the last public handle aborts
// maintenance and its JoinSet, including any handshakes currently in flight.
struct PoolOwner<S> {
    inner: Arc<PoolInner<S>>,
    task: std::sync::Mutex<Option<tokio::task::AbortHandle>>,
}
impl<S> Drop for PoolOwner<S> {
    fn drop(&mut self) {
        if let Some(task) = self.task.lock().unwrap().take() {
            task.abort();
        }
    }
}

pub struct ConnPool<S = BoxProxyStream> {
    owner: Arc<PoolOwner<S>>,
}

impl<S> Clone for ConnPool<S> {
    fn clone(&self) -> Self {
        Self {
            owner: self.owner.clone(),
        }
    }
}

impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static> ConnPool<S> {
    pub fn new(factory: Arc<dyn ConnFactory<S>>) -> Self {
        let (base, capacity, _) = crate::resources::connection_pool_limits();
        Self::with_limits(factory, base, capacity, shared_budget())
    }

    /// One-shot probes never create background connections.
    pub fn lazy(factory: Arc<dyn ConnFactory<S>>) -> Self {
        Self::with_capacity(factory, 0)
    }

    pub fn with_capacity(factory: Arc<dyn ConnFactory<S>>, capacity: usize) -> Self {
        Self::with_limits(factory, capacity, capacity, shared_budget())
    }

    fn with_limits(
        factory: Arc<dyn ConnFactory<S>>,
        base: usize,
        capacity: usize,
        budget: Arc<Semaphore>,
    ) -> Self {
        let now = Instant::now();
        let pool = Self {
            owner: Arc::new(PoolOwner {
                inner: Arc::new(PoolInner {
                    state: Mutex::new(PoolState {
                        conns: VecDeque::with_capacity(base),
                        target: base,
                        requests: 0,
                        window_started: now,
                        last_get: now,
                    }),
                    factory,
                    base,
                    capacity,
                    budget,
                    refill: Notify::new(),
                }),
                task: std::sync::Mutex::new(None),
            }),
        };
        pool.start_worker();
        pool
    }

    fn start_worker(&self) {
        let inner = &self.owner.inner;
        if inner.capacity == 0 || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let mut task = self.owner.task.lock().unwrap();
        if task.as_ref().is_none_or(|t| t.is_finished()) {
            let inner = inner.clone();
            *task = Some(tokio::spawn(async move { inner.run().await }).abort_handle());
        }
    }

    pub async fn get(&self) -> anyhow::Result<S> {
        self.start_worker();
        let inner = &self.owner.inner;
        {
            let mut state = inner.state.lock().await;
            if state.last_get.elapsed() >= DEFAULT_MAX_AGE {
                state.target = inner.base;
            }
            if state.window_started.elapsed() >= DEMAND_WINDOW {
                state.window_started = Instant::now();
                state.requests = 0;
                state.target = inner.base;
            }
            state.requests = state.requests.saturating_add(1).min(inner.capacity);
            state.target = state
                .target
                .max(state.requests.next_power_of_two())
                .min(inner.capacity);
            state.last_get = Instant::now();
        }
        loop {
            let candidate = inner.state.lock().await.conns.pop_front();
            let Some(mut candidate) = candidate else {
                break;
            };
            if candidate.created.elapsed() < DEFAULT_MAX_AGE
                && idle_stream_is_ready(&mut candidate.stream).await
            {
                inner.refill.notify_one();
                return Ok(candidate.stream);
            }
        }
        inner.refill.notify_one();
        tokio::time::timeout(
            super::connector::UPSTREAM_CONNECT_TIMEOUT,
            inner.factory.create(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("Pool connect timed out"))?
    }

    #[cfg(test)]
    pub(super) async fn len(&self) -> usize {
        self.owner.inner.state.lock().await.conns.len()
    }
}

fn shared_budget() -> Arc<Semaphore> {
    static BUDGET: OnceLock<Arc<Semaphore>> = OnceLock::new();
    BUDGET
        .get_or_init(|| Arc::new(Semaphore::new(crate::resources::connection_pool_limits().2)))
        .clone()
}

/// A single non-blocking read poll detects FIN, TLS close/error, or unexpected
/// server data before handing out an unused connection. Never replay user data.
async fn idle_stream_is_ready<S: tokio::io::AsyncRead + Unpin>(stream: &mut S) -> bool {
    use std::{pin::Pin, task::Poll};
    use tokio::io::ReadBuf;
    let mut byte = [0];
    let mut buf = ReadBuf::new(&mut byte);
    std::future::poll_fn(|cx| {
        Poll::Ready(matches!(
            Pin::new(&mut *stream).poll_read(cx, &mut buf),
            Poll::Pending
        ))
    })
    .await
}

impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static> PoolInner<S> {
    async fn run(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        self.fill().await;
        loop {
            tokio::select! {
                _ = self.refill.notified() => self.fill().await,
                _ = tick.tick() => {
                    let mut state = self.state.lock().await;
                    state.conns.retain(|c| c.created.elapsed() < DEFAULT_MAX_AGE);
                    if state.last_get.elapsed() >= DEFAULT_MAX_AGE {
                        state.target = self.base;
                    }
                    let target = state.target;
                    state.conns.truncate(target);
                    // No demand: do not replace expired idle connections.
                }
            }
        }
    }

    async fn fill(&self) {
        self.state
            .lock()
            .await
            .conns
            .retain(|c| c.created.elapsed() < DEFAULT_MAX_AGE);
        let mut failed_rounds = 0;
        // Bound maintenance even if foreground demand continually drains it.
        let mut attempts = 0;
        while attempts < self.capacity.saturating_mul(3) {
            let deficit = {
                let state = self.state.lock().await;
                state.target.saturating_sub(state.conns.len())
            };
            if deficit == 0 {
                return;
            }
            let mut set = tokio::task::JoinSet::new();
            for _ in 0..deficit.min(FILL_CONCURRENCY) {
                let Ok(permit) = self.budget.clone().try_acquire_owned() else {
                    break;
                };
                let factory = self.factory.clone();
                set.spawn(async move {
                    let stream = tokio::time::timeout(
                        super::connector::UPSTREAM_CONNECT_TIMEOUT,
                        factory.create(),
                    )
                    .await
                    .map_err(|_| anyhow::anyhow!("Pool pre-connect timed out"))??;
                    Ok::<_, anyhow::Error>(PooledConn {
                        stream,
                        created: Instant::now(),
                        _permit: permit,
                    })
                });
            }
            let batch = set.len();
            if batch == 0 {
                return;
            }
            attempts += batch;
            let mut successes = 0;
            while let Some(result) = set.join_next().await {
                match result {
                    Ok(Ok(conn)) => {
                        successes += 1;
                        let mut state = self.state.lock().await;
                        if state.conns.len() < state.target {
                            state.conns.push_back(conn);
                        }
                    }
                    _ => tracing::debug!("Connection pool: pre-connect failed"),
                }
            }
            if successes == 0 {
                failed_rounds += 1;
                if failed_rounds >= 3 {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            } else {
                failed_rounds = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    async fn filled(pool: &ConnPool, expected: usize) {
        for _ in 0..200 {
            if pool.len().await == expected {
                return;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(pool.len().await, expected);
    }

    #[tokio::test(start_paused = true)]
    async fn burst_grows_reserve_and_idle_maintenance_releases_it() {
        let budget = Arc::new(Semaphore::new(16));
        let pool = ConnPool::with_limits(
            MockFactory::new(Duration::ZERO, false),
            2,
            8,
            budget.clone(),
        );
        filled(&pool, 2).await;
        for _ in 0..8 {
            drop(pool.get().await.unwrap());
        }
        filled(&pool, 8).await;
        tokio::time::advance(DEFAULT_MAX_AGE + Duration::from_secs(6)).await;
        filled(&pool, 0).await;
        assert_eq!(pool.owner.inner.state.lock().await.target, 2);
        assert_eq!(budget.available_permits(), 16);
        // A new request after idle returns to the small initial target.
        drop(pool.get().await.unwrap());
        filled(&pool, 2).await;
    }

    #[tokio::test]
    async fn closed_preconnects_are_discarded_before_any_user_data() {
        let factory = MockFactory::new(Duration::ZERO, false);
        let pool = ConnPool::with_limits(factory.clone(), 2, 2, Arc::new(Semaphore::new(4)));
        filled(&pool, 2).await;
        factory.peers.lock().unwrap().clear();
        let mut stream = pool.get().await.unwrap();
        assert!(idle_stream_is_ready(&mut stream).await);
        assert!(factory.attempts.load(AtomicOrdering::SeqCst) >= 3);
    }

    #[tokio::test]
    async fn idle_probe_preserves_tls_session_tickets_and_later_application_bytes() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        struct ReadyFactory(std::sync::Mutex<Option<BoxProxyStream>>);
        impl ConnFactory for ReadyFactory {
            fn create(
                &self,
            ) -> Pin<Box<dyn Future<Output = anyhow::Result<BoxProxyStream>> + Send + '_>>
            {
                Box::pin(async {
                    self.0
                        .lock()
                        .unwrap()
                        .take()
                        .ok_or_else(|| anyhow::anyhow!("fixture exhausted"))
                })
            }
        }
        let cert = include_bytes!("testdata/localhost-cert.der");
        let key = include_bytes!("testdata/localhost-key.der");
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![rustls_pki_types::CertificateDer::from(cert.to_vec())],
            rustls_pki_types::PrivateKeyDer::Pkcs8(rustls_pki_types::PrivatePkcs8KeyDer::from(
                key.to_vec(),
            )),
        )
        .unwrap();
        let (client, server) = tokio::io::duplex(16 * 1024);
        let server = tokio::spawn(async move {
            let mut tls = tokio_rustls::TlsAcceptor::from(Arc::new(config))
                .accept(server)
                .await
                .unwrap();
            let mut request = [0; 4];
            tls.read_exact(&mut request).await.unwrap();
            assert_eq!(&request, b"ping");
            tls.write_all(b"pong").await.unwrap();
            tls.shutdown().await.unwrap();
        });
        use sha2::Digest;
        let pin: [u8; 32] = sha2::Sha256::digest(cert).into();
        let client = super::super::transport::build_tls_connector_pinned(None, Some(pin))
            .connect(
                rustls_pki_types::ServerName::try_from("localhost").unwrap(),
                client,
            )
            .await
            .unwrap();
        let factory = Arc::new(ReadyFactory(std::sync::Mutex::new(Some(Box::new(client)))));
        let pool = ConnPool::with_limits(factory, 1, 1, Arc::new(Semaphore::new(1)));
        filled(&pool, 1).await;
        let mut stream = pool.get().await.unwrap();
        stream.write_all(b"ping").await.unwrap();
        stream.flush().await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        assert_eq!(&response, b"pong");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn pools_share_a_hard_preconnect_budget() {
        let budget = Arc::new(Semaphore::new(3));
        let first = ConnPool::with_limits(
            MockFactory::new(Duration::ZERO, false),
            2,
            8,
            budget.clone(),
        );
        filled(&first, 2).await;
        let second = ConnPool::with_limits(
            MockFactory::new(Duration::ZERO, false),
            2,
            8,
            budget.clone(),
        );
        filled(&second, 1).await;
        assert_eq!(budget.available_permits(), 0);
        // Foreground connections can still proceed when the warm budget is full.
        drop(second.get().await.unwrap());
        drop(second.get().await.unwrap());
    }

    #[tokio::test]
    async fn dropping_last_pool_handle_cancels_inflight_warmup() {
        let factory = MockFactory::new(Duration::from_secs(3600), false);
        let budget = Arc::new(Semaphore::new(4));
        let pool = ConnPool::with_limits(factory.clone(), 4, 4, budget.clone());
        let weak = Arc::downgrade(&pool.owner.inner);
        for _ in 0..200 {
            if factory.attempts.load(AtomicOrdering::SeqCst) == 4 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(budget.available_permits(), 0);
        let last = pool.clone();
        drop(pool);
        assert!(weak.upgrade().is_some());
        drop(last);
        for _ in 0..200 {
            if weak.upgrade().is_none() && budget.available_permits() == 4 {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("warmup retained a dropped pool or its budget");
    }

    /// Mock factory that creates in-memory duplex streams and records
    /// creation statistics (attempts, peak in-flight concurrency).
    struct MockFactory {
        attempts: Arc<AtomicUsize>,
        max_in_flight: Arc<AtomicUsize>,
        in_flight: Arc<AtomicUsize>,
        delay: Duration,
        fail: bool,
        peers: Arc<std::sync::Mutex<Vec<tokio::io::DuplexStream>>>,
    }

    impl MockFactory {
        fn new(delay: Duration, fail: bool) -> Arc<Self> {
            Arc::new(Self {
                attempts: Arc::new(AtomicUsize::new(0)),
                max_in_flight: Arc::new(AtomicUsize::new(0)),
                in_flight: Arc::new(AtomicUsize::new(0)),
                delay,
                fail,
                peers: Arc::new(std::sync::Mutex::new(Vec::new())),
            })
        }
    }

    impl ConnFactory for MockFactory {
        fn create(
            &self,
        ) -> Pin<Box<dyn Future<Output = anyhow::Result<BoxProxyStream>> + Send + '_>> {
            let attempts = self.attempts.clone();
            let max_in_flight = self.max_in_flight.clone();
            let in_flight = self.in_flight.clone();
            let delay = self.delay;
            let fail = self.fail;
            let peers = self.peers.clone();
            Box::pin(async move {
                attempts.fetch_add(1, AtomicOrdering::SeqCst);
                let now = in_flight.fetch_add(1, AtomicOrdering::SeqCst) + 1;
                max_in_flight.fetch_max(now, AtomicOrdering::SeqCst);
                if delay > Duration::ZERO {
                    tokio::time::sleep(delay).await;
                }
                in_flight.fetch_sub(1, AtomicOrdering::SeqCst);
                if fail {
                    anyhow::bail!("mock factory failure");
                }
                let (a, b) = tokio::io::duplex(64);
                peers.lock().unwrap().push(b);
                Ok(Box::new(a) as BoxProxyStream)
            })
        }
    }

    /// Wait until `cond` holds, polling every 10 ms for up to `timeout`.
    async fn wait_for<F: FnMut() -> bool>(timeout: Duration, mut cond: F) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if cond() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        cond()
    }

    #[tokio::test]
    async fn pool_starts_with_a_small_device_tier_reserve() {
        let factory = MockFactory::new(Duration::ZERO, false);
        let pool = ConnPool::new(factory.clone());

        let base = crate::resources::connection_pool_limits().0;
        assert!(
            wait_for(Duration::from_secs(5), || {
                factory.attempts.load(AtomicOrdering::SeqCst) >= base
            })
            .await
        );
        let mut ok = false;
        for _ in 0..100 {
            if pool.len().await == base {
                ok = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(ok, "pool did not fill its initial reserve");
    }

    #[tokio::test]
    async fn pool_fill_respects_concurrency_cap() {
        let factory = MockFactory::new(Duration::from_millis(100), false);
        let max_in_flight = factory.max_in_flight.clone();
        let pool = ConnPool::with_capacity(factory, 8);

        let mut filled = false;
        for _ in 0..200 {
            if pool.len().await == 8 {
                filled = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(filled, "pool did not fill to capacity 8");
        assert!(
            max_in_flight.load(AtomicOrdering::SeqCst) <= FILL_CONCURRENCY,
            "fill exceeded concurrency cap: {}",
            max_in_flight.load(AtomicOrdering::SeqCst)
        );
    }

    #[tokio::test]
    async fn pool_get_returns_warm_connection_and_refills() {
        let factory = MockFactory::new(Duration::ZERO, false);
        let pool = ConnPool::with_capacity(factory, 4);

        let mut filled = false;
        for _ in 0..100 {
            if pool.len().await == 4 {
                filled = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(filled);

        let conn = pool.get().await.unwrap();
        drop(conn);
        // A refill was scheduled; pool returns to capacity.
        let mut refilled = false;
        for _ in 0..100 {
            if pool.len().await == 4 {
                refilled = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(refilled, "pool did not refill after get()");
    }

    #[tokio::test]
    async fn lazy_pool_never_warms_in_background() {
        let factory = MockFactory::new(Duration::ZERO, false);
        let attempts = factory.attempts.clone();
        let pool = ConnPool::lazy(factory);

        // No eager warm-up, and no fill is scheduled by get().
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(attempts.load(AtomicOrdering::SeqCst), 0);
        let conn = pool.get().await.unwrap();
        drop(conn);
        tokio::time::sleep(Duration::from_millis(100)).await;
        // Only the direct get() creation ran; the pool stays empty.
        assert_eq!(attempts.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(pool.len().await, 0);
    }

    #[tokio::test]
    async fn pool_fill_gives_up_after_three_failed_rounds() {
        let factory = MockFactory::new(Duration::ZERO, true);
        let attempts = factory.attempts.clone();
        let pool = ConnPool::with_capacity(factory, 4);

        // 3 failed rounds x batch of 4 = 12 attempts, then fill stops.
        assert!(
            wait_for(Duration::from_secs(5), || {
                attempts.load(AtomicOrdering::SeqCst) >= 12
            })
            .await,
            "fill never ran"
        );
        // Allow one more potential round; attempts must stay at 12.
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert_eq!(attempts.load(AtomicOrdering::SeqCst), 12);
        assert_eq!(pool.len().await, 0);

        // get() with an empty pool surfaces the factory error.
        assert!(pool.get().await.is_err());
    }
}

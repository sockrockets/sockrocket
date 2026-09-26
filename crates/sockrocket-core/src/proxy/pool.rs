use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use super::connector::BoxProxyStream;

/// Max age for pooled connections before they're considered stale.
const DEFAULT_MAX_AGE: Duration = Duration::from_secs(120);
/// Default pool capacity (pre-established connections to keep warm).
const DEFAULT_CAPACITY: usize = 16;
/// Max concurrent connection creations during a single fill round
/// (bounds burst load on the proxy server).
const FILL_CONCURRENCY: usize = 4;

/// A pre-established TCP+TLS connection waiting to be used.
struct PooledConn {
    stream: BoxProxyStream,
    created: Instant,
}

/// Async factory that creates new TCP+TLS connections to the proxy server.
/// Each outbound (VLESS, VMess, Trojan) provides its own implementation.
pub trait ConnFactory: Send + Sync + 'static {
    fn create(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = anyhow::Result<BoxProxyStream>> + Send + '_>,
    >;
}

/// Inner state of the connection pool.
struct PoolInner {
    conns: Mutex<VecDeque<PooledConn>>,
    factory: Arc<dyn ConnFactory>,
    capacity: usize,
    max_age: Duration,
    warmed: std::sync::atomic::AtomicBool,
    filling: std::sync::atomic::AtomicBool,
}

/// Connection pool that maintains pre-established TCP+TLS connections
/// to avoid 2-3 RTTs (TCP handshake + TLS handshake) on each request.
///
/// Usage: call `get()` to obtain a stream. If a warm connection is
/// available it's returned immediately; otherwise a new one is created.
/// After taking a connection, a background refill task is spawned.
#[derive(Clone)]
pub struct ConnPool {
    inner: Arc<PoolInner>,
}

impl ConnPool {
    /// Create a new pool and start the initial warm-up.
    pub fn new(factory: Arc<dyn ConnFactory>) -> Self {
        Self::with_capacity(factory, DEFAULT_CAPACITY)
    }

    /// Create a pool that never warms up in the background: every `get()`
    /// creates a fresh connection on demand. Use this for one-shot
    /// probe/latency outbounds so a single latency test doesn't fire a full
    /// round of pre-connect handshakes at the server.
    pub fn lazy(factory: Arc<dyn ConnFactory>) -> Self {
        Self::with_capacity(factory, 0)
    }

    pub fn with_capacity(factory: Arc<dyn ConnFactory>, capacity: usize) -> Self {
        let pool = Self {
            inner: Arc::new(PoolInner {
                conns: Mutex::new(VecDeque::with_capacity(capacity)),
                factory,
                capacity,
                max_age: DEFAULT_MAX_AGE,
                warmed: std::sync::atomic::AtomicBool::new(false),
                filling: std::sync::atomic::AtomicBool::new(false),
            }),
        };
        // Eager warm-up: start filling immediately when created inside a
        // tokio runtime, instead of waiting for the first `get()`.
        // A zero-capacity (lazy) pool has nothing to fill.
        if capacity > 0 && tokio::runtime::Handle::try_current().is_ok() {
            pool.inner
                .warmed
                .store(true, std::sync::atomic::Ordering::Relaxed);
            pool.schedule_fill();
        }
        pool
    }

    /// Get a connection: returns a warm pooled one if available, else creates new.
    /// On first call, kicks off background pool warmup.
    pub async fn get(&self) -> anyhow::Result<BoxProxyStream> {
        // Lazy warmup: start filling on first use
        if !self
            .inner
            .warmed
            .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            self.schedule_fill();
        }

        // Try to grab a non-stale connection from the pool
        {
            let mut q = self.inner.conns.lock().await;
            while let Some(pc) = q.pop_front() {
                if pc.created.elapsed() < self.inner.max_age {
                    drop(q);
                    // Got a warm connection — schedule refill
                    self.schedule_fill();
                    return Ok(pc.stream);
                }
                // Stale — drop it and try next
            }
        }
        // Pool empty — create directly (no extra latency beyond normal)
        self.inner.factory.create().await
    }

    /// Schedule a background fill only if no fill is already in progress.
    fn schedule_fill(&self) {
        if self.inner.capacity == 0 {
            // Lazy pool: never fill in the background.
            return;
        }
        if !self
            .inner
            .filling
            .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            let p = self.clone();
            tokio::spawn(async move {
                p.fill().await;
                p.inner
                    .filling
                    .store(false, std::sync::atomic::Ordering::Relaxed);
            });
        }
    }

    #[cfg(test)]
    async fn len(&self) -> usize {
        self.inner.conns.lock().await.len()
    }

    /// Fill the pool up to capacity. Only one fill runs at a time.
    ///
    /// Connections are created in concurrent batches of [`FILL_CONCURRENCY`]
    /// so warm-up latency is bounded by one connect RTT per batch instead of
    /// one per connection.
    async fn fill(&self) {
        // Evict stale connections first
        {
            let mut q = self.inner.conns.lock().await;
            q.retain(|pc| pc.created.elapsed() < self.inner.max_age);
        }

        let mut failed_rounds: u32 = 0;
        loop {
            let deficit = {
                let q = self.inner.conns.lock().await;
                self.inner.capacity.saturating_sub(q.len())
            };
            if deficit == 0 {
                break;
            }
            let batch = deficit.min(FILL_CONCURRENCY);

            let mut set = tokio::task::JoinSet::new();
            for _ in 0..batch {
                let factory = self.inner.factory.clone();
                set.spawn(async move { factory.create().await });
            }

            let mut failures: u32 = 0;
            while let Some(res) = set.join_next().await {
                match res {
                    Ok(Ok(stream)) => {
                        let mut q = self.inner.conns.lock().await;
                        if q.len() < self.inner.capacity {
                            q.push_back(PooledConn {
                                stream,
                                created: Instant::now(),
                            });
                        }
                    }
                    Ok(Err(e)) => {
                        failures += 1;
                        tracing::debug!("Connection pool: pre-connect failed: {}", e);
                    }
                    Err(e) => {
                        failures += 1;
                        tracing::debug!("Connection pool: pre-connect task failed: {}", e);
                    }
                }
            }

            if failures == batch as u32 {
                // Whole batch failed — don't give up on the first failure;
                // transient errors (DNS, TCP reset) are common. Try up to 3
                // consecutive failed rounds before abandoning the refill so
                // the pool isn't left permanently underfilled.
                failed_rounds += 1;
                if failed_rounds >= 3 {
                    break;
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

    /// Mock factory that creates in-memory duplex streams and records
    /// creation statistics (attempts, peak in-flight concurrency).
    struct MockFactory {
        attempts: Arc<AtomicUsize>,
        max_in_flight: Arc<AtomicUsize>,
        in_flight: Arc<AtomicUsize>,
        delay: Duration,
        fail: bool,
    }

    impl MockFactory {
        fn new(delay: Duration, fail: bool) -> Arc<Self> {
            Arc::new(Self {
                attempts: Arc::new(AtomicUsize::new(0)),
                max_in_flight: Arc::new(AtomicUsize::new(0)),
                in_flight: Arc::new(AtomicUsize::new(0)),
                delay,
                fail,
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
                let (a, _b) = tokio::io::duplex(64);
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
    async fn pool_warms_up_eagerly_to_default_capacity() {
        let factory = MockFactory::new(Duration::ZERO, false);
        let pool = ConnPool::new(factory.clone());

        // Eager warm-up (spawned by the constructor) fills to 16.
        assert!(
            wait_for(Duration::from_secs(5), || {
                factory.attempts.load(AtomicOrdering::SeqCst) >= 16
            })
            .await
        );
        // Pool should hold 16 warm connections now.
        let mut ok = false;
        for _ in 0..100 {
            if pool.len().await == 16 {
                ok = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(ok, "pool did not fill to capacity 16");
        assert_eq!(DEFAULT_CAPACITY, 16);
        assert_eq!(DEFAULT_MAX_AGE, Duration::from_secs(120));
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

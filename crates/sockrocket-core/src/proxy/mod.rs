pub mod anytls;
pub mod connector;
pub mod datagram;
pub mod factory;
pub mod health;
pub mod http;
mod http_forward;
pub mod hysteria2;
pub mod outbound_bind;
pub mod pool;
pub mod quic_conn;
mod quic_datagram;
pub mod relay;
pub mod routing;
pub mod service;
pub mod shadow_tls;
pub mod socks5;
mod socks_udp;
pub mod speedtest;
pub mod ss;
mod stream_datagram;
pub mod transport;
pub mod trojan;
pub mod tuic;
pub mod tun;
pub mod vless;
pub mod vmess;
pub mod ws;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Proxy connection state.
#[derive(Debug, Clone, PartialEq)]
pub enum ProxyState {
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

/// A single recent connection, recorded for the GUI connections monitor.
/// Lightweight metadata only — no payload is ever captured.
#[derive(Debug, Clone)]
pub struct ConnectionRecord {
    /// `host:port` of the requested destination.
    pub dest: String,
    /// Listener protocol that accepted it ("SOCKS5" or "HTTP").
    pub proto: &'static str,
    /// Unix seconds when the connection was accepted.
    pub started_unix: u64,
    /// Bytes sent upstream before close (0 while still open).
    pub bytes_up: u64,
    /// Bytes received downstream before close (0 while still open).
    pub bytes_down: u64,
    /// Whether the connection is still open.
    pub active: bool,
    pub completion: Option<&'static str>,
    live: Option<Arc<(AtomicU64, AtomicU64)>>,
}

/// Max number of recent connection records retained.
const RECENT_CONNECTIONS_CAP: usize = 120;

/// Anonymous TUN resource telemetry. It does not assert LAN reachability.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TunDiagnostics {
    pub sampled_at: u64,
    pub running: bool,
    pub active: usize,
    pub rejected: u64,
    pub packet_bytes: usize,
    pub packet_limit: usize,
    pub receive_bytes: usize,
    pub tcp_states: [usize; 10],
    pub oldest_close_secs: u64,
}

/// Connection and traffic statistics, shared across server tasks.
#[derive(Debug, Clone)]
pub struct ProxyStats {
    inner: Arc<StatsInner>,
}

#[derive(Debug)]
struct StatsInner {
    active_connections: AtomicU64,
    total_connections: AtomicU64,
    bytes_sent: AtomicU64,
    bytes_received: AtomicU64,
    recent: std::sync::Mutex<std::collections::VecDeque<ConnectionRecord>>,
    slots: Arc<tokio::sync::Semaphore>,
    tun: std::sync::Mutex<Option<TunDiagnostics>>,
    completed: AtomicU64,
    interrupted: AtomicU64,
}

impl Default for ProxyStats {
    fn default() -> Self {
        Self {
            inner: Arc::new(StatsInner {
                slots: Arc::new(tokio::sync::Semaphore::new(
                    crate::resources::ResourceBudget::detect().tun_streams,
                )),
                tun: std::sync::Mutex::new(None),
                completed: AtomicU64::new(0),
                interrupted: AtomicU64::new(0),
                active_connections: AtomicU64::new(0),
                total_connections: AtomicU64::new(0),
                bytes_sent: AtomicU64::new(0),
                bytes_received: AtomicU64::new(0),
                recent: std::sync::Mutex::new(std::collections::VecDeque::new()),
            }),
        }
    }
}

impl ProxyStats {
    pub(crate) fn update_tun(&self, diagnostics: TunDiagnostics) {
        *self.inner.tun.lock().unwrap() = Some(diagnostics);
    }
    pub(crate) fn stop_tun(&self) {
        if let Some(tun) = self.inner.tun.lock().unwrap().as_mut() {
            tun.running = false;
            tun.active = 0;
        }
    }
    pub fn runtime_diagnostics(&self) -> serde_json::Value {
        serde_json::json!({"active_connections":self.active_connections(), "bytes_sent":self.bytes_sent(),
            "bytes_received":self.bytes_received(),"completed":self.inner.completed.load(Ordering::Relaxed),
            "interrupted":self.inner.interrupted.load(Ordering::Relaxed), "tun":self.inner.tun.lock().unwrap().clone(),
            "lan_reachability":"unverified"})
    }
    pub fn active_connections(&self) -> u64 {
        self.inner.active_connections.load(Ordering::Relaxed)
    }

    pub fn total_connections(&self) -> u64 {
        self.inner.total_connections.load(Ordering::Relaxed)
    }

    pub fn bytes_sent(&self) -> u64 {
        self.inner.bytes_sent.load(Ordering::Relaxed)
    }

    pub fn bytes_received(&self) -> u64 {
        self.inner.bytes_received.load(Ordering::Relaxed)
    }

    pub(crate) fn add_connection(&self) {
        self.inner
            .active_connections
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn remove_connection(&self) {
        self.inner
            .active_connections
            .fetch_sub(1, Ordering::Relaxed);
    }

    /// Accumulate bytes into the shared totals. Called by connection handlers
    /// (via [`ProxyStats::close_connection`]) — kept public(crate) for tests.
    #[allow(dead_code)] // only exercised from tests on some build configs
    pub(crate) fn add_bytes(&self, sent: u64, received: u64) {
        self.inner.bytes_sent.fetch_add(sent, Ordering::Relaxed);
        self.inner
            .bytes_received
            .fetch_add(received, Ordering::Relaxed);
    }

    pub fn reset(&self) {
        // Live admission guards must retain their counters and record IDs.
        // Restarting byte totals must not underflow a later guard release.
        self.inner.bytes_sent.store(0, Ordering::Relaxed);
        self.inner.bytes_received.store(0, Ordering::Relaxed);
        if let Ok(mut recent) = self.inner.recent.lock()
            && self.active_connections() == 0
        {
            recent.clear();
            self.inner.total_connections.store(0, Ordering::Relaxed);
        }
    }

    /// Record a newly accepted connection. Returns its id so the handler can
    /// later finalize it with [`ProxyStats::close_connection`].
    ///
    /// The id is assigned with a single `fetch_add` *while holding the ring
    /// lock*, so concurrent connections always get unique, dense ids that
    /// match the ring order — previously `add_connection` incremented the
    /// counter and this method re-derived the id via `load() - 1`, which let
    /// two concurrent connections share an id.
    pub fn record_connection(&self, dest: String, proto: &'static str) -> u64 {
        let started_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if let Ok(mut recent) = self.inner.recent.lock() {
            let id = self.inner.total_connections.fetch_add(1, Ordering::Relaxed);
            recent.push_front(ConnectionRecord {
                dest,
                proto,
                started_unix,
                bytes_up: 0,
                bytes_down: 0,
                active: true,
                completion: None,
                live: None,
            });
            while recent.len() > RECENT_CONNECTIONS_CAP {
                recent.pop_back();
            }
            id
        } else {
            // Lock poisoned: still hand out a unique id.
            self.inner.total_connections.fetch_add(1, Ordering::Relaxed)
        }
    }

    /// Mark a recorded connection closed with its final byte counts.
    /// `id` is the value returned by [`ProxyStats::record_connection`].
    ///
    /// Also accumulates the totals (`bytes_sent`/`bytes_received`) that feed
    /// the GUI speed readouts — previously only `add_bytes` did that and it
    /// had no callers, so the totals stayed at 0 forever.
    pub fn close_connection(&self, id: u64, bytes_up: u64, bytes_down: u64) {
        self.inner.bytes_sent.fetch_add(bytes_up, Ordering::Relaxed);
        self.inner
            .bytes_received
            .fetch_add(bytes_down, Ordering::Relaxed);
        // Ring front always holds the highest id (total-1); a record with the
        // given id sits at offset (total-1 - id) while still retained.
        if let Ok(mut recent) = self.inner.recent.lock() {
            // Read the newest id while holding the same lock that inserts it.
            // Otherwise an insertion between the snapshot and lock shifts rows.
            let last = self
                .inner
                .total_connections
                .load(Ordering::Relaxed)
                .saturating_sub(1);
            if id > last {
                return;
            }
            if let Some(rec) = recent.get_mut((last - id) as usize) {
                rec.active = false;
                rec.completion = Some("closed");
                rec.bytes_up = bytes_up;
                rec.bytes_down = bytes_down;
            }
        }
    }

    pub(crate) fn try_slot(&self) -> Option<ConnectionSlot> {
        let permit = self.inner.slots.clone().try_acquire_owned().ok()?;
        self.add_connection();
        Some(ConnectionSlot {
            stats: self.clone(),
            _permit: permit,
        })
    }

    pub(crate) fn track(&self, dest: String, proto: &'static str) -> ConnectionGuard {
        let id = self.record_connection(dest, proto);
        let counts = Arc::new((AtomicU64::new(0), AtomicU64::new(0)));
        if let Ok(mut recent) = self.inner.recent.lock() {
            let last = self.total_connections().saturating_sub(1);
            if id <= last
                && let Some(record) = recent.get_mut((last - id) as usize)
            {
                record.live = Some(counts.clone());
            }
        }
        ConnectionGuard {
            stats: self.clone(),
            id,
            finished: false,
            counts,
            active_owned: false,
        }
    }

    pub(crate) fn track_active(&self, dest: String, proto: &'static str) -> ConnectionGuard {
        self.add_connection();
        let mut guard = self.track(dest, proto);
        guard.active_owned = true;
        guard
    }

    /// Snapshot of recent connections, newest first.
    ///
    /// `keep` filters and `limit` caps the clone *inside* the lock, so callers
    /// (the GUI renders at most 60 rows) no longer clone the whole 120-record
    /// ring on every repaint.
    pub fn recent_connections(
        &self,
        limit: usize,
        keep: impl Fn(&ConnectionRecord) -> bool,
    ) -> Vec<ConnectionRecord> {
        self.inner
            .recent
            .lock()
            .map(|r| {
                r.iter()
                    .map(|record| {
                        let mut record = record.clone();
                        if let Some(live) = &record.live {
                            record.bytes_up = live.0.load(Ordering::Relaxed);
                            record.bytes_down = live.1.load(Ordering::Relaxed);
                        }
                        record
                    })
                    .filter(|record| keep(record))
                    .take(limit)
                    .collect()
            })
            .unwrap_or_default()
    }
}

pub(crate) const PROXY_HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

pub(crate) struct ConnectionSlot {
    stats: ProxyStats,
    _permit: tokio::sync::OwnedSemaphorePermit,
}
impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.stats.remove_connection();
    }
}
pub(crate) struct ConnectionGuard {
    stats: ProxyStats,
    id: u64,
    finished: bool,
    active_owned: bool,
    counts: std::sync::Arc<(AtomicU64, AtomicU64)>,
}
impl ConnectionGuard {
    pub(crate) fn uploaded(&self, bytes: usize) {
        self.counts.0.fetch_add(bytes as u64, Ordering::Relaxed);
        self.stats.add_bytes(bytes as u64, 0);
    }
    pub(crate) fn finish(mut self, up: u64, down: u64) {
        let live_up = self.counts.0.load(Ordering::Relaxed);
        let live_down = self.counts.1.load(Ordering::Relaxed);
        self.stats.close_connection(
            self.id,
            up.saturating_sub(live_up),
            down.saturating_sub(live_down),
        );
        self.update_record(up.max(live_up), down.max(live_down), "completed");
        self.stats.inner.completed.fetch_add(1, Ordering::Relaxed);
        self.finished = true;
    }
}
impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        if self.active_owned {
            self.stats.remove_connection();
        }
        if !self.finished {
            self.stats.inner.interrupted.fetch_add(1, Ordering::Relaxed);
            self.stats.close_connection(self.id, 0, 0);
            self.update_record(
                self.counts.0.load(Ordering::Relaxed),
                self.counts.1.load(Ordering::Relaxed),
                "interrupted",
            );
        }
    }
}
impl ConnectionGuard {
    fn update_record(&self, up: u64, down: u64, reason: &'static str) {
        if let Ok(mut recent) = self.stats.inner.recent.lock() {
            let last = self.stats.total_connections().saturating_sub(1);
            if self.id <= last
                && let Some(record) = recent.get_mut((last - self.id) as usize)
            {
                record.bytes_up = up;
                record.bytes_down = down;
                record.completion = Some(reason);
                record.live = None;
            }
        }
    }
    pub(crate) fn observe_owned<S>(&self, stream: S) -> ObservedIo<S> {
        ObservedIo {
            stream,
            stats: self.stats.clone(),
            counts: self.counts.clone(),
        }
    }
    pub(crate) fn observe<'a, S: ?Sized>(&self, stream: &'a mut S) -> ObservedIo<&'a mut S> {
        ObservedIo {
            stream,
            stats: self.stats.clone(),
            counts: self.counts.clone(),
        }
    }
}
pub(crate) struct ObservedIo<S> {
    stream: S,
    stats: ProxyStats,
    counts: std::sync::Arc<(AtomicU64, AtomicU64)>,
}
impl<S: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for ObservedIo<S> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let result = std::pin::Pin::new(&mut self.stream).poll_read(cx, buf);
        let n = (buf.filled().len() - before) as u64;
        if n > 0 {
            self.counts.0.fetch_add(n, Ordering::Relaxed);
            self.stats.add_bytes(n, 0);
        }
        result
    }
}
impl<S: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for ObservedIo<S> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let result = std::pin::Pin::new(&mut self.stream).poll_write(cx, buf);
        if let std::task::Poll::Ready(Ok(n)) = result {
            self.counts.1.fetch_add(n as u64, Ordering::Relaxed);
            self.stats.add_bytes(0, n as u64);
        }
        result
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Concurrent record_connection calls must yield unique, dense ids, and
    /// close_connection must finalize exactly the record with that id.
    #[test]
    fn connection_ids_unique_and_close_targets_right_record() {
        let stats = ProxyStats::default();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .build()
            .unwrap();

        const N: u64 = 64;
        let ids: Vec<u64> = rt.block_on(async {
            let mut handles = Vec::new();
            for i in 0..N {
                let s = stats.clone();
                handles.push(tokio::spawn(async move {
                    s.add_connection();
                    // Yield to force interleaving between add and record.
                    tokio::task::yield_now().await;
                    s.record_connection(format!("host-{i}:443"), "SOCKS5")
                }));
            }
            let mut ids = Vec::new();
            for h in handles {
                ids.push(h.await.unwrap());
            }
            ids
        });

        // All ids unique and dense (0..N).
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..N).collect::<Vec<_>>());
        assert_eq!(stats.total_connections(), N);
        assert_eq!(stats.active_connections(), N);

        // Close each connection with bytes equal to its id; then verify the
        // ring position math landed on the right record for every id.
        for id in ids {
            stats.close_connection(id, id, id * 10);
        }
        let recent = stats.recent_connections(usize::MAX, |_| true);
        assert_eq!(recent.len(), N as usize);
        for (idx, rec) in recent.iter().enumerate() {
            // Front of the ring holds the highest id: id = (N-1) - idx.
            let expected_id = (N - 1) - idx as u64;
            assert!(!rec.active);
            assert_eq!(rec.bytes_up, expected_id);
            assert_eq!(rec.bytes_down, expected_id * 10);
        }
    }

    /// close_connection must accumulate the shared totals that drive the GUI
    /// speed readouts — regression test for the "traffic counters always empty" bug where
    /// bytes_sent/bytes_received were never incremented.
    #[test]
    fn close_connection_accumulates_totals() {
        let stats = ProxyStats::default();
        let a = stats.record_connection("a:443".into(), "HTTP");
        let b = stats.record_connection("b:443".into(), "SOCKS5");
        stats.close_connection(a, 100, 1000);
        stats.close_connection(b, 200, 2000);
        assert_eq!(stats.bytes_sent(), 300);
        assert_eq!(stats.bytes_received(), 3000);
    }
}

pub mod wireguard;

pub mod connector;
pub mod factory;
pub mod health;
pub mod http;
pub mod hysteria2;
pub mod outbound_bind;
pub mod pool;
pub mod quic_conn;
pub mod relay;
pub mod routing;
pub mod service;
pub mod shadow_tls;
pub mod socks5;
pub mod speedtest;
pub mod ss;
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
}

/// Max number of recent connection records retained.
const RECENT_CONNECTIONS_CAP: usize = 120;

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
}

impl Default for ProxyStats {
    fn default() -> Self {
        Self {
            inner: Arc::new(StatsInner {
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
        self.inner.active_connections.store(0, Ordering::Relaxed);
        self.inner.total_connections.store(0, Ordering::Relaxed);
        self.inner.bytes_sent.store(0, Ordering::Relaxed);
        self.inner.bytes_received.store(0, Ordering::Relaxed);
        if let Ok(mut recent) = self.inner.recent.lock() {
            recent.clear();
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
        let last = self
            .inner
            .total_connections
            .load(Ordering::Relaxed)
            .saturating_sub(1);
        if id > last {
            return;
        }
        let idx = (last - id) as usize;
        if let Ok(mut recent) = self.inner.recent.lock()
            && let Some(rec) = recent.get_mut(idx)
        {
            rec.active = false;
            rec.bytes_up = bytes_up;
            rec.bytes_down = bytes_down;
        }
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
                    .filter(|rec| keep(rec))
                    .take(limit)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
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

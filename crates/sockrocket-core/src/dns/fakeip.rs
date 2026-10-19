//! Fake-IP pool for TUN-mode transparent proxying.
//!
//! Why this exists: some relay/airport exits cannot dial arbitrary public
//! IPs — they work by resolving the DOMAIN server-side (their own
//! hijacked/unlock DNS) and connecting to the resulting unlock address.
//! Transparent proxying breaks that model: the client resolves the name
//! first (getting the real IP), then sends bare-IP packets into the TUN,
//! and the daemon can only ask the node to dial that IP — which the exit
//! cannot reach (observed on a DNS-unlock airport exit:
//! `dial tcp4 <real google ip>:443: connection timed out`, while the same
//! site worked instantly over SOCKS5 where the domain travels upstream).
//!
//! Fake-IP restores the domain path: the DNS layer answers international
//! A-queries with addresses from the fake range and remembers the
//! fake-IP → domain mapping; when TUN traffic arrives for a fake address
//! the stream handler dials the DOMAIN through the outbound, so the node
//! resolves it server-side exactly like the SOCKS5 path.
//!
//! Domestic names never enter the pool: they are answered with real
//! addresses so `geoip:CN → direct` routing keeps them off the proxy.
//!
//! Persistence (Clash Meta / sing-box style): the domain↔fake-IP map is
//! optionally written to disk so a TUN restart reuses the same addresses.
//! Clients that still hold a cached Fake-IP after a toggle then reverse-map
//! correctly instead of waiting for DNS TTL expiry (~minutes on ISP DNS).

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::net::{IpAddr, Ipv4Addr};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lru::LruCache;

/// Fake-IP range: 198.18.0.0/16 — the RFC 2544 benchmarking range, never
/// routed on the public internet and the de-facto standard for this
/// purpose (Clash/sing-box use the same). The router's mangle rules do NOT
/// exempt it, so client traffic to these addresses is steered into the TUN
/// like any other internet destination.
const FAKE_IP_BASE: u32 = 0xC612_0000; // 198.18.0.0
const FAKE_IP_MASK: u32 = 0xFFFF_0000; // /16

/// Start allocating at .2 (.0 is the network address; .1 stays unused so
/// it can never collide with gateway-style uses of the range).
const FIRST_HOST_OFFSET: u32 = 2;
/// .0 and .1 of the /16 are skipped; the broadcast address (.255.255) is
/// never handed out because allocation stops at MAX_ENTRIES.
const MAX_ENTRIES: usize = 8192;

/// How many recently-seen domestic real IPs to remember. These feed the
/// TUN layer's direct-dial decision, so the table only needs to cover
/// addresses clients could still be holding from recent DNS answers.
/// Generous on purpose: CDN rotation makes a household churn thousands of
/// distinct IPs per hour, and eviction of a hot name's address (observed
/// a hot domestic name at cap 4096) silently drops it back onto the
/// geoip path — which is exactly the path this table exists to bypass.
const DOMESTIC_MAX_ENTRIES: usize = 16384;

/// Debounce disk writes on JFFS/flash: at most one persist every N seconds
/// while allocating; shutdown always flushes.
const PERSIST_MIN_INTERVAL: Duration = Duration::from_secs(10);

/// Test whether an address belongs to the fake range. Cheap and pure, so
/// hot paths (every TUN stream) can call it without touching the pool.
pub fn is_fake_ip(ip: Ipv4Addr) -> bool {
    u32::from(ip) & FAKE_IP_MASK == FAKE_IP_BASE
}

/// Bidirectional fake-IP ↔ domain map with LRU eviction.
///
/// Cheap to clone (Arc inside and out); the DNS resolver and the TUN
/// stream handler share one instance so an address handed out by the
/// resolver is always resolvable by the stream handler.
pub struct FakeIpPool {
    inner: Mutex<Inner>,
    persist_path: Option<PathBuf>,
    last_persist: Mutex<Option<Instant>>,
}

struct Inner {
    /// Next host offset to try when allocating (wraps within the /16).
    next: u32,
    by_domain: HashMap<String, Ipv4Addr>,
    by_ip: HashMap<Ipv4Addr, String>,
    /// Insertion order (oldest first) for LRU eviction.
    order: VecDeque<String>,
    /// Real addresses (v4 and v6) recently answered for domestic-group
    /// (China) names, mapped to the name that produced them.
    domestic: LruCache<IpAddr, String>,
}

impl FakeIpPool {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                next: FIRST_HOST_OFFSET,
                by_domain: HashMap::new(),
                by_ip: HashMap::new(),
                order: VecDeque::new(),
                domestic: LruCache::new(
                    NonZeroUsize::new(DOMESTIC_MAX_ENTRIES).expect("capacity is non-zero"),
                ),
            }),
            persist_path: None,
            last_persist: Mutex::new(None),
        })
    }

    /// Load a previously persisted store (Clash Meta–style cache), or start
    /// empty when the file is missing/corrupt. Always attaches `path` for
    /// subsequent [`Self::persist`] / debounced writes from [`Self::allocate`].
    pub fn load_or_new(path: impl Into<PathBuf>) -> Arc<Self> {
        let path = path.into();
        let pool = Arc::new(Self {
            inner: Mutex::new(Inner {
                next: FIRST_HOST_OFFSET,
                by_domain: HashMap::new(),
                by_ip: HashMap::new(),
                order: VecDeque::new(),
                domestic: LruCache::new(
                    NonZeroUsize::new(DOMESTIC_MAX_ENTRIES).expect("capacity is non-zero"),
                ),
            }),
            persist_path: Some(path.clone()),
            last_persist: Mutex::new(None),
        });
        if let Err(e) = pool.load_from(&path) {
            tracing::warn!(
                "fake-IP store load from {}: {e:#} — starting empty",
                path.display()
            );
        } else {
            let n = pool.inner.lock().map(|g| g.by_domain.len()).unwrap_or(0);
            if n > 0 {
                tracing::info!(
                    "fake-IP store loaded {} mapping(s) from {}",
                    n,
                    path.display()
                );
            }
        }
        pool
    }

    fn load_from(&self, path: &Path) -> std::io::Result<()> {
        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        let mut inner = self.inner.lock().expect("fake-ip pool poisoned");
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(rest) = line.strip_prefix("next=") {
                if let Ok(n) = rest.parse::<u32>() {
                    inner.next = n.max(FIRST_HOST_OFFSET);
                }
                continue;
            }
            let mut parts = line.split_whitespace();
            let (Some(ip_s), Some(domain)) = (parts.next(), parts.next()) else {
                continue;
            };
            let Ok(ip) = ip_s.parse::<Ipv4Addr>() else {
                continue;
            };
            if !is_fake_ip(ip) {
                continue;
            }
            let domain = domain.to_ascii_lowercase();
            if inner.by_domain.len() >= MAX_ENTRIES {
                break;
            }
            if inner.by_domain.contains_key(&domain) || inner.by_ip.contains_key(&ip) {
                continue;
            }
            inner.by_domain.insert(domain.clone(), ip);
            inner.by_ip.insert(ip, domain.clone());
            inner.order.push_back(domain);
        }
        Ok(())
    }

    /// Flush the domain↔IP map to disk. No-op when persistence is disabled.
    pub fn persist(&self) -> std::io::Result<()> {
        let Some(path) = self.persist_path.as_ref() else {
            return Ok(());
        };
        let (next, rows) = {
            let inner = self.inner.lock().expect("fake-ip pool poisoned");
            let rows: Vec<(Ipv4Addr, String)> = inner
                .order
                .iter()
                .filter_map(|d| inner.by_domain.get(d).map(|ip| (*ip, d.clone())))
                .collect();
            (inner.next, rows)
        };
        let mut out = String::from("# sockrocket fake-ip store v1\n");
        out.push_str(&format!("next={next}\n"));
        for (ip, domain) in rows {
            out.push_str(&format!("{ip} {domain}\n"));
        }
        let tmp = path.with_extension("store.tmp");
        fs::write(&tmp, out)?;
        fs::rename(&tmp, path)?;
        if let Ok(mut last) = self.last_persist.lock() {
            *last = Some(Instant::now());
        }
        Ok(())
    }

    fn maybe_persist(&self) {
        let Ok(last) = self.last_persist.lock() else {
            return;
        };
        if last.is_some_and(|t| t.elapsed() < PERSIST_MIN_INTERVAL) {
            return;
        }
        drop(last);
        if let Err(e) = self.persist() {
            tracing::debug!("fake-IP persist skipped/failed: {e}");
        }
    }

    /// Return the fake address for `domain`, allocating one on first use.
    /// Repeated calls for the same domain return the same address, which
    /// keeps client-side and dnsmasq caches coherent.
    pub fn allocate(&self, domain: &str) -> Ipv4Addr {
        let domain = domain.to_ascii_lowercase();
        let mut inner = self.inner.lock().expect("fake-ip pool poisoned");

        if let Some(ip) = inner.by_domain.get(&domain) {
            return *ip;
        }

        // Evict the oldest mapping when full.
        if inner.by_domain.len() >= MAX_ENTRIES
            && let Some(oldest) = inner.order.pop_front()
            && let Some(ip) = inner.by_domain.remove(&oldest)
        {
            inner.by_ip.remove(&ip);
        }

        let offset = inner.next;
        inner.next = if inner.next as usize >= MAX_ENTRIES + FIRST_HOST_OFFSET as usize {
            FIRST_HOST_OFFSET
        } else {
            inner.next + 1
        };
        let ip = Ipv4Addr::from(FAKE_IP_BASE | offset);

        // The freshly-allocated offset can still be mapped from a previous
        // wrap-around: drop the stale forward entry so lookups stay 1:1.
        if let Some(old_domain) = inner.by_ip.remove(&ip) {
            inner.by_domain.remove(&old_domain);
        }

        inner.by_domain.insert(domain.clone(), ip);
        inner.by_ip.insert(ip, domain.clone());
        inner.order.push_back(domain);
        drop(inner);
        self.maybe_persist();
        ip
    }

    /// Resolve a fake address back to its domain. `None` for addresses
    /// outside the range or mappings already evicted.
    pub fn lookup(&self, ip: Ipv4Addr) -> Option<String> {
        if !is_fake_ip(ip) {
            return None;
        }
        self.inner
            .lock()
            .expect("fake-ip pool poisoned")
            .by_ip
            .get(&ip)
            .cloned()
    }

    /// Remember that `ip` was just answered for the domestic-group name
    /// `domain` (see [`Inner::domestic`]).
    pub fn record_domestic(&self, ip: IpAddr, domain: &str) {
        self.inner
            .lock()
            .expect("fake-ip pool poisoned")
            .domestic
            .put(ip, domain.to_ascii_lowercase());
    }

    /// The domestic-group name that most recently resolved to `ip`, if any.
    pub fn lookup_domestic(&self, ip: IpAddr) -> Option<String> {
        self.inner
            .lock()
            .expect("fake-ip pool poisoned")
            .domestic
            .peek(&ip)
            .cloned()
    }

    /// Number of live mappings (diagnostics/tests).
    #[cfg(test)]
    fn len(&self) -> usize {
        self.inner
            .lock()
            .expect("fake-ip pool poisoned")
            .by_domain
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocate_is_stable_per_domain() {
        let pool = FakeIpPool::new();
        let a1 = pool.allocate("Example.COM");
        let a2 = pool.allocate("example.com");
        assert_eq!(a1, a2, "same domain (any case) must reuse one fake IP");
        assert!(is_fake_ip(a1));
    }

    #[test]
    fn distinct_domains_get_distinct_ips() {
        let pool = FakeIpPool::new();
        let a = pool.allocate("a.example.com");
        let b = pool.allocate("b.example.com");
        assert_ne!(a, b);
    }

    #[test]
    fn lookup_roundtrips() {
        let pool = FakeIpPool::new();
        let ip = pool.allocate("www.youtube.com");
        assert_eq!(pool.lookup(ip).as_deref(), Some("www.youtube.com"));
    }

    #[test]
    fn lookup_rejects_non_fake_ips() {
        let pool = FakeIpPool::new();
        assert_eq!(pool.lookup(Ipv4Addr::new(8, 8, 8, 8)), None);
        assert_eq!(pool.lookup(Ipv4Addr::new(192, 168, 1, 1)), None);
    }

    #[test]
    fn range_check_boundaries() {
        assert!(is_fake_ip(Ipv4Addr::new(198, 18, 0, 2)));
        assert!(is_fake_ip(Ipv4Addr::new(198, 18, 255, 254)));
        assert!(!is_fake_ip(Ipv4Addr::new(198, 19, 0, 1)));
        assert!(!is_fake_ip(Ipv4Addr::new(198, 17, 255, 255)));
    }

    #[test]
    fn lru_eviction_drops_oldest() {
        let pool = FakeIpPool::new();
        let first = pool.allocate("first.example.com");
        for i in 1..MAX_ENTRIES {
            pool.allocate(&format!("d{i}.example.com"));
        }
        assert_eq!(pool.len(), MAX_ENTRIES);
        pool.allocate("overflow.example.com");
        assert_eq!(pool.len(), MAX_ENTRIES);
        assert_eq!(pool.lookup(first), None, "oldest mapping must be evicted");
    }

    #[test]
    fn domestic_record_roundtrips() {
        let pool = FakeIpPool::new();
        let ip = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 10));
        assert_eq!(pool.lookup_domestic(ip), None);
        pool.record_domestic(ip, "example.cn");
        assert_eq!(pool.lookup_domestic(ip).as_deref(), Some("example.cn"));
        assert_eq!(
            pool.lookup_domestic(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 11))),
            None
        );
        let v6 = IpAddr::V6("2001:db8::1".parse().unwrap());
        pool.record_domestic(v6, "example.cn");
        assert_eq!(pool.lookup_domestic(v6).as_deref(), Some("example.cn"));
    }

    #[test]
    fn persist_roundtrip_reuses_same_ip() {
        let dir = std::env::temp_dir().join(format!("sockrocket-fakeip-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("fakeip.store");
        let _ = fs::remove_file(&path);

        let pool = FakeIpPool::load_or_new(&path);
        let ip = pool.allocate("www.google.com");
        pool.persist().unwrap();

        let pool2 = FakeIpPool::load_or_new(&path);
        assert_eq!(pool2.lookup(ip).as_deref(), Some("www.google.com"));
        assert_eq!(pool2.allocate("www.google.com"), ip);

        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&dir);
    }
}

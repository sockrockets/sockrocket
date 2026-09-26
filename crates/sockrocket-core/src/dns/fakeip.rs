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

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr};
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

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
    ///
    /// Why this exists: geoip is the only signal the TUN layer has for
    /// bare-IP traffic, and geoip databases routinely miss CDN ranges
    /// (observed: a domestic site → CDN
    /// a domestic CDN address not classified CN → forced through the proxy → the
    /// unlock-model exit cannot dial the bare IP → site dead). The DNS
    /// split already decided these names are domestic; remembering their
    /// answers lets the TUN handler honor that decision and dial direct.
    /// Keeping the domain alongside lets explicit user routing rules
    /// (domain/domain-suffix/keyword) override the direct default.
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
        })
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
    /// `domain` (see [`Inner::domestic`]). Called by the resolver on every
    /// domestic-group A/AAAA answer, including cache hits — the repeated
    /// `put` refreshes recency, which is what keeps a hot name's address
    /// from being LRU-evicted by one-off CDN answers.
    pub fn record_domestic(&self, ip: IpAddr, domain: &str) {
        self.inner
            .lock()
            .expect("fake-ip pool poisoned")
            .domestic
            .put(ip, domain.to_ascii_lowercase());
    }

    /// The domestic-group name that most recently resolved to `ip`, if any.
    /// Presence means "the DNS split considers this a domestic address" —
    /// the TUN handler dials it direct unless a user rule says otherwise.
    pub fn lookup_domestic(&self, ip: IpAddr) -> Option<String> {
        // peek (not get): lookups here must not promote recency — only a
        // fresh DNS answer proves the address is still being handed out.
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
        // Fill the pool to capacity.
        for i in 1..MAX_ENTRIES {
            pool.allocate(&format!("d{i}.example.com"));
        }
        assert_eq!(pool.len(), MAX_ENTRIES);
        // One more allocation evicts the oldest ("first.example.com").
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
        // Unrelated addresses stay unknown.
        assert_eq!(
            pool.lookup_domestic(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 11))),
            None
        );
        // IPv6 records work too (dual-stack clients may prefer v6).
        let v6 = IpAddr::V6("2001:db8::1".parse().unwrap());
        pool.record_domestic(v6, "example.cn");
        assert_eq!(pool.lookup_domestic(v6).as_deref(), Some("example.cn"));
    }
}

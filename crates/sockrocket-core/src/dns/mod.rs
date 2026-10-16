//! DNS resolver module for sockrocket.
//!
//! Provides a configurable DNS resolver supporting:
//! - System DNS (uses OS resolver)
//! - Custom UDP DNS servers
//! - DNS-over-HTTPS (DoH) via standard wireformat
//! - DNS split: route China domains to local DNS, others to remote DNS

pub mod fakeip;
mod listener;
pub use listener::check_listener as check_dns_listener;
mod upstream;
mod wire;
pub use wire::{
    build_dns_error_response, build_dns_response, build_dns_response_with_ttl, parse_dns_query,
};
use wire::{build_dns_query, parse_dns_response, validate_response};

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use lru::LruCache;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, RwLock};

use crate::proxy::connector::{BoxProxyStream, SharedOutbound, connect_upstream};
use crate::router::GeoIpDb;

use self::fakeip::FakeIpPool;

/// Cap for the resolve-then-geoip CN-direct decision cache.
const CN_DIRECT_CACHE_MAX: usize = 4096;

/// DNS resolver configuration.
#[derive(Debug, Clone)]
pub struct DnsConfig {
    /// Primary DNS servers (IP:port or DoH URL)
    pub servers: Vec<DnsServer>,
    /// Second server group, selected per-domain via [`DnsConfig::domain_rules`]
    /// (e.g. domestic resolvers for China domains). Deliberately NOT used as a
    /// failure fallback for the primary group: cross-group fallback would
    /// re-query failed international domains against domestic resolvers,
    /// which return GFW-poisoned records for those names.
    pub fallback: Vec<DnsServer>,
    /// Domain rules: domain suffix -> server group (e.g., "cn" -> use fallback)
    pub domain_rules: HashMap<String, DnsGroup>,
    /// Cache TTL override (0 = use DNS TTL, >0 = override)
    pub cache_ttl: u32,
    /// Enable DNS cache
    pub cache_enabled: bool,
}

impl Default for DnsConfig {
    fn default() -> Self {
        Self {
            servers: vec![
                DnsServer::Udp("8.8.8.8:53".parse().unwrap()),
                DnsServer::Udp("1.1.1.1:53".parse().unwrap()),
            ],
            fallback: vec![
                DnsServer::Udp("223.5.5.5:53".parse().unwrap()),
                DnsServer::Udp("119.29.29.29:53".parse().unwrap()),
            ],
            domain_rules: HashMap::new(),
            cache_ttl: 0,
            cache_enabled: true,
        }
    }
}

/// Which DNS server group to use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsGroup {
    Primary,
    Fallback,
}

/// DNS server types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsServer {
    /// Standard UDP DNS
    Udp(SocketAddr),
    /// DNS-over-HTTPS (e.g., "https://dns.google/dns-query")
    DoH(String),
    /// DNS-over-TCP through a proxy outbound (e.g. the active proxy node).
    ///
    /// The wire query is sent as `[u16 length][message]` over a TCP stream
    /// dialed via [`SharedOutbound`], so the lookup exits through the proxy
    /// node and is immune to on-path UDP DNS poisoning (GFW fake replies).
    ProxyTcp(SocketAddr),
    /// DNS-over-TLS (RFC 7858) through a proxy outbound: `(server, SNI)`.
    ///
    /// Preferred over [`DnsServer::ProxyTcp`]: plaintext TCP-53 through a
    /// commercial relay is routinely hijacked at the exit (observed
    /// youtube → unlock-server IPs with mismatched certs,
    /// wikipedia/zlibrary → GFW-pool fake answers, while google was
    /// special-cased clean). TLS to port 853 cannot be transparently
    /// hijacked without a valid certificate for the SNI, so the exit can
    /// only block it outright — never silently poison it.
    ProxyTls(SocketAddr, String),
}

/// DNS query type for A (IPv4) records.
pub(crate) const QTYPE_A: u16 = 1;
/// DNS query type for AAAA (IPv6) records.
pub(crate) const QTYPE_AAAA: u16 = 28;

/// Cached DNS entry. An empty `addresses` vec is a *negative* entry
/// (NXDOMAIN / empty answer / upstream failure) with a short TTL.
#[derive(Debug, Clone)]
struct CacheEntry {
    addresses: Vec<IpAddr>,
    expires_at: Instant,
}

/// Cache key: answers are per (domain, qtype) so a negative AAAA entry can
/// never poison A lookups for the same name (and vice versa).
type CacheKey = (String, u16);

/// Maximum number of entries in the DNS cache before LRU eviction.
const DNS_CACHE_MAX_ENTRIES: usize = 1024;

/// TTL for negative cache entries (NXDOMAIN, empty answers, timeouts).
/// Short so a name that starts resolving recovers quickly.
const NEGATIVE_CACHE_TTL: Duration = Duration::from_secs(30);

/// How long past expiry a stale positive entry may still be served
/// (stale-while-revalidate).
const STALE_WHILE_REVALIDATE: Duration = Duration::from_secs(300);

/// In-flight DNS query entry for deduplication.
#[derive(Clone)]
struct LookupAnswer {
    addresses: Vec<IpAddr>,
    ttl: u32,
}
type InflightEntry = Arc<tokio::sync::OnceCell<Result<LookupAnswer, String>>>;
type InflightMap = Arc<std::sync::Mutex<HashMap<CacheKey, InflightEntry>>>;

// Synchronous cleanup also runs when a DNS request future is cancelled.
struct InflightGuard {
    map: InflightMap,
    key: CacheKey,
    cell: InflightEntry,
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        let mut map = self.map.lock().unwrap();
        // Keep the shared cell while another caller is still using it.
        if Arc::strong_count(&self.cell) == 2
            && map
                .get(&self.key)
                .is_some_and(|entry| Arc::ptr_eq(entry, &self.cell))
        {
            map.remove(&self.key);
        }
    }
}

/// Number of slots in each proxy-DNS connection pool. Four concurrent
/// international lookups cover a typical multi-domain page load without
/// stampeding the proxy node with dials.
const PROXY_DNS_POOL_SIZE: usize = 4;

/// A small pool of independently-locked proxy-DNS connection slots.
/// `acquire()` picks a free slot without waiting (round-robin + try_lock)
/// and only queues when every slot is busy.
struct PooledDnsStream {
    stream: BoxProxyStream,
    outbound: SharedOutbound,
    server: DnsServer,
}
struct ProxyConnPool {
    slots: Vec<Mutex<Option<PooledDnsStream>>>,
    cursor: std::sync::atomic::AtomicUsize,
}

impl ProxyConnPool {
    fn new(size: usize) -> Self {
        Self {
            slots: (0..size).map(|_| Mutex::new(None)).collect(),
            cursor: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    async fn acquire(&self) -> tokio::sync::MutexGuard<'_, Option<PooledDnsStream>> {
        use std::sync::atomic::Ordering;
        let n = self.slots.len();
        let start = self.cursor.fetch_add(1, Ordering::Relaxed);
        for i in 0..n {
            let idx = (start + i) % n;
            if let Ok(guard) = self.slots[idx].try_lock() {
                return guard;
            }
        }
        self.slots[start % n].lock().await
    }
}

struct RuntimeDnsCache {
    outbound: SharedOutbound,
    cache: Arc<RwLock<LruCache<CacheKey, CacheEntry>>>,
    inflight: InflightMap,
    cn_direct: Arc<RwLock<LruCache<String, bool>>>,
}

/// The main DNS resolver.
///
/// Cheap to clone: all state (config, cache, DoH client, in-flight map) is
/// shared, so background refreshes go through the same in-flight dedup as
/// foreground queries.
#[derive(Clone)]
pub struct DnsResolver {
    config: Arc<DnsConfig>,
    /// LRU cache; bounded to [`DNS_CACHE_MAX_ENTRIES`] by the LRU itself.
    /// Fresh hits only need a read lock (recency is not promoted on read —
    /// expiry and capacity bounds are unaffected).
    cache: Arc<RwLock<LruCache<CacheKey, CacheEntry>>>,
    /// Shared reqwest client for DoH queries (avoids per-query creation)
    doh_client: reqwest::Client,
    /// In-flight queries: concurrent resolves for the same (domain, qtype)
    /// share one query
    inflight: InflightMap,
    /// Bound detached stale-cache refreshes as well as foreground DNS work.
    refresh_slots: Arc<tokio::sync::Semaphore>,
    /// Outbound used by [`DnsServer::ProxyTcp`] servers. `None` until
    /// [`Self::set_outbound`] is called; ProxyTcp queries fail fast with a
    /// clear error in that state.
    outbound: Option<SharedOutbound>,
    follow_runtime: bool,
    primary_override: Option<Arc<Vec<DnsServer>>>,
    runtime_cache: Arc<std::sync::Mutex<Option<RuntimeDnsCache>>>,
    /// Shared DNS-over-TCP connections through the proxy. Small pool of
    /// slots, each independently locked: queries within one slot are
    /// serialized (one outstanding exchange per connection), but different
    /// slots run concurrently, so a multi-domain page load no longer queues
    /// every international lookup behind a single connection. Using pooled
    /// persistent connections — instead of dialing per query — keeps the
    /// number of concurrent proxy dials bounded (observed on the router:
    /// unbounded racing hysteria2 dials hung the stream forever).
    proxy_tcp_conn: Arc<ProxyConnPool>,
    /// Same pattern as `proxy_tcp_conn`, for [`DnsServer::ProxyTls`].
    proxy_tls_conn: Arc<ProxyConnPool>,
    /// Shared TLS connector (webpki root store) for DoT connections.
    tls_connector: tokio_rustls::TlsConnector,
    /// Fake-IP pool (TUN transparent proxy only). `None` in plain
    /// socks/http mode, where clients need real addresses.
    fakeip: Option<Arc<FakeIpPool>>,
    /// Optional GeoIP DB for resolve-then-geoip before fake-IP allocation.
    /// When set with [`Self::set_fakeip`], Primary-group A queries first
    /// resolve via the clean primary upstream; if any answer is `geoip:CN`,
    /// the real addresses are returned (domestic direct) instead of a
    /// fake-IP. Avoids relying on an incomplete domain suffix list.
    geoip: Option<Arc<GeoIpDb>>,
    /// Cached per-domain CN-direct decisions from resolve-then-geoip
    /// (`true` = return real IPs / direct; `false` = fake-IP / proxy).
    cn_direct: Arc<RwLock<LruCache<String, bool>>>,
    /// Proxy-server hostnames (the tunnel endpoints). These must NEVER be
    /// answered with fake IPs and must resolve via the direct (fallback)
    /// group: the router's own tooling (Web UI speedtest uses system DNS)
    /// and LAN clients need real, directly-reachable addresses for them —
    /// a fake answer makes every speedtest time out, and resolving a server
    /// through the proxy would be circular.
    server_domains: Arc<std::sync::RwLock<std::collections::HashSet<String>>>,
}

impl DnsResolver {
    /// Create a new resolver with default config (8.8.8.8 + 1.1.1.1).
    pub fn new() -> Self {
        Self::with_config(DnsConfig::default())
    }
}

impl Default for DnsResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl DnsResolver {
    /// Create a new resolver with custom config.
    pub fn with_config(config: DnsConfig) -> Self {
        let doh_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .pool_max_idle_per_host(2)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            config: Arc::new(config),
            cache: Arc::new(RwLock::new(LruCache::new(
                NonZeroUsize::new(DNS_CACHE_MAX_ENTRIES).expect("capacity is non-zero"),
            ))),
            doh_client,
            inflight: Arc::new(std::sync::Mutex::new(HashMap::new())),
            refresh_slots: Arc::new(tokio::sync::Semaphore::new(16)),
            outbound: None,
            follow_runtime: false,
            primary_override: None,
            runtime_cache: Arc::new(std::sync::Mutex::new(None)),
            proxy_tcp_conn: Arc::new(ProxyConnPool::new(PROXY_DNS_POOL_SIZE)),
            proxy_tls_conn: Arc::new(ProxyConnPool::new(PROXY_DNS_POOL_SIZE)),
            tls_connector: build_dot_tls_connector(),
            fakeip: None,
            geoip: None,
            cn_direct: Arc::new(RwLock::new(LruCache::new(
                NonZeroUsize::new(CN_DIRECT_CACHE_MAX).expect("capacity is non-zero"),
            ))),
            server_domains: Arc::new(std::sync::RwLock::new(std::collections::HashSet::new())),
        }
    }

    /// Attach the shared proxy-server hostname set. The resolver treats
    /// these names (and their subdomains) as fallback-group/direct no matter
    /// what the suffix tables say, so they resolve to real addresses via
    /// domestic upstreams and never enter the fake-IP pool. The handle is
    /// shared: the caller may repopulate it on config reload without
    /// rebuilding the resolver.
    pub fn set_server_domains(
        &mut self,
        handle: Arc<std::sync::RwLock<std::collections::HashSet<String>>>,
    ) {
        self.server_domains = handle;
    }

    /// Whether `domain` is (a subdomain of) a configured proxy-server host.
    fn is_server_domain(&self, domain: &str) -> bool {
        let Ok(set) = self.server_domains.read() else {
            return false;
        };
        if set.contains(domain) {
            return true;
        }
        // Subdomain check without allocation: for each dot boundary, the
        // remaining suffix could only be a registered server host if that
        // host equals the suffix... registered hosts are full hostnames, so
        // instead check whether any host is a suffix at a dot boundary.
        set.iter().any(|host| {
            domain.len() > host.len()
                && domain.ends_with(host.as_str())
                && domain.as_bytes()[domain.len() - host.len() - 1] == b'.'
        })
    }

    /// Attach a fake-IP pool (see [`fakeip`]). With a pool attached, A
    /// queries for primary-group domains may be answered from the fake
    /// range (see [`Self::set_geoip`] for resolve-then-geoip). Domestic
    /// (fallback-group) names still resolve to real addresses.
    pub fn set_fakeip(&mut self, pool: Arc<FakeIpPool>) {
        self.fakeip = Some(pool);
    }

    /// Attach a GeoIP database so fake-IP mode can classify Primary-group
    /// names by **resolved IP country** (`geoip:CN` → real address / direct;
    /// otherwise fake-IP / proxy). Without this, only the domain suffix
    /// table decides who gets a fake-IP.
    pub fn set_geoip(&mut self, db: Arc<GeoIpDb>) {
        self.geoip = Some(db);
    }

    /// Whether `ip` belongs to the fake-IP range this resolver hands out.
    /// Cheap pure range check; safe on hot paths.
    pub fn is_fake(&self, ip: &IpAddr) -> bool {
        self.fakeip.is_some() && matches!(ip, IpAddr::V4(v4) if fakeip::is_fake_ip(*v4))
    }

    /// Whether a fake-IP pool is attached (TUN transparent proxy mode).
    pub fn has_fakeip(&self) -> bool {
        self.fakeip.is_some()
    }

    /// Attach an outbound for [`DnsServer::ProxyTcp`] servers. Cheap to call
    /// again: the resolver stores the `SharedOutbound` clone, so swapping the
    /// underlying node (via `SwappableOutbound`) needs no re-attachment.
    pub fn set_outbound(&mut self, outbound: SharedOutbound) {
        self.outbound = Some(outbound);
    }

    /// Follow a published outbound's DNS policy, including direct/proxy changes.
    pub fn follow_runtime(&mut self, outbound: SharedOutbound) {
        self.outbound = Some(outbound);
        self.follow_runtime = true;
    }

    fn query_snapshot(&self) -> Self {
        let mut snapshot = self.clone();
        if let Some(outbound) = &self.outbound
            && let Some((current, config)) = outbound.0.dns_snapshot().or_else(|| {
                outbound
                    .0
                    .runtime_snapshot()
                    .map(|outbound| (outbound, None))
            })
        {
            if let Some(config) = config {
                snapshot.config = config;
            }
            let mut generation = self.runtime_cache.lock().unwrap();
            if generation
                .as_ref()
                .is_none_or(|cache| !Arc::ptr_eq(&cache.outbound.0, &current.0))
            {
                *generation = Some(RuntimeDnsCache {
                    outbound: current.clone(),
                    cache: Arc::new(RwLock::new(LruCache::new(
                        NonZeroUsize::new(DNS_CACHE_MAX_ENTRIES).unwrap(),
                    ))),
                    inflight: Arc::new(std::sync::Mutex::new(HashMap::new())),
                    cn_direct: Arc::new(RwLock::new(LruCache::new(
                        NonZeroUsize::new(CN_DIRECT_CACHE_MAX).unwrap(),
                    ))),
                });
            }
            let generation = generation.as_ref().unwrap();
            snapshot.cache = generation.cache.clone();
            snapshot.inflight = generation.inflight.clone();
            snapshot.cn_direct = generation.cn_direct.clone();
            snapshot.outbound = Some(current);
        }
        if self.follow_runtime {
            if snapshot
                .outbound
                .as_ref()
                .is_some_and(|outbound| outbound.0.is_direct())
            {
                snapshot.fakeip = None;
            }
            snapshot.primary_override = Some(Arc::new(
                if snapshot
                    .outbound
                    .as_ref()
                    .is_some_and(|outbound| outbound.0.is_direct())
                {
                    vec![
                        DnsServer::Udp(([8, 8, 8, 8], 53).into()),
                        DnsServer::Udp(([1, 1, 1, 1], 53).into()),
                    ]
                } else {
                    vec![
                        DnsServer::ProxyTls(([8, 8, 8, 8], 853).into(), "dns.google".into()),
                        DnsServer::ProxyTls(
                            ([1, 1, 1, 1], 853).into(),
                            "cloudflare-dns.com".into(),
                        ),
                    ]
                },
            ));
        }
        snapshot
    }

    /// Resolve a hostname to IP addresses (A-record query).
    pub async fn resolve(&self, domain: &str) -> Result<Vec<IpAddr>> {
        self.resolve_with_qtype(domain, QTYPE_A).await
    }

    /// Resolve a hostname with an explicit query type (1 = A, 28 = AAAA).
    ///
    /// Answers are cached per (domain, qtype); failures (NXDOMAIN, empty
    /// answers, timeouts) are cached as negative entries for
    /// [`NEGATIVE_CACHE_TTL`] and resolve to an empty address list.
    pub async fn resolve_with_qtype(&self, domain: &str, qtype: u16) -> Result<Vec<IpAddr>> {
        self.query_snapshot()
            .resolve_qtype_snapshot(domain, qtype)
            .await
    }

    async fn resolve_qtype_snapshot(&self, domain: &str, qtype: u16) -> Result<Vec<IpAddr>> {
        // DNS names are case-insensitive. Canonicalize before policy, cache
        // and fake-IP lookup so uppercase/trailing-dot names cannot bypass
        // the domestic group or create unrelated cache entries.
        let canonical = domain.trim_end_matches('.').to_ascii_lowercase();
        if canonical.len() > 253
            || (!canonical.is_empty()
                && canonical
                    .split('.')
                    .any(|label| label.is_empty() || label.len() > 63))
        {
            bail!("Invalid DNS name length");
        }
        let domain = canonical.as_str();
        let result = self.resolve_qtype_inner(domain, qtype).await;
        // Domestic-group answers are real addresses; remember them on EVERY
        // return path (fresh resolve, cache hit, stale serve) so the TUN
        // layer can dial them direct instead of trusting geoip for bare-IP
        // traffic — and so a hot name keeps its recency refreshed instead of
        // being LRU-evicted by one-off CDN churn (see
        // [`FakeIpPool::record_domestic`]). Primary-group answers are fake
        // pool addresses (or nothing), never real IPs, so they must not be
        // recorded here.
        if let (Ok(addrs), Some(pool)) = (&result, &self.fakeip)
            && self.match_domain_group(domain) == DnsGroup::Fallback
        {
            for addr in addrs {
                pool.record_domestic(*addr, domain);
            }
        }
        result
    }

    /// Inner resolution path: IP-literal short-circuit, fake-IP hook,
    /// cache, dedup. See [`Self::resolve_with_qtype`] for the contract.
    async fn resolve_qtype_inner(&self, domain: &str, qtype: u16) -> Result<Vec<IpAddr>> {
        // IP literals short-circuit; only the queried family answers.
        if let Ok(ip) = domain.parse::<IpAddr>() {
            return Ok(match (qtype, ip) {
                (QTYPE_A, IpAddr::V4(_)) | (QTYPE_AAAA, IpAddr::V6(_)) => vec![ip],
                _ => vec![],
            });
        }

        // Fake-IP interception (transparent proxy) for Primary-group names.
        // Fallback-group (curated China suffixes / server hosts) always get
        // real addresses below. When a GeoIP DB is attached, Primary names
        // are classified by resolve-then-geoip on a *clean* primary upstream
        // (never domestic UDP — those poison google et al.): any `CN` answer
        // → real IPs + domestic direct table; otherwise → fake-IP so the TUN
        // dials by name (DNS-unlock exits).
        if let Some(pool) = &self.fakeip
            && self.match_domain_group(domain) == DnsGroup::Primary
        {
            if self.geoip.is_some() {
                return self.resolve_primary_fakeip_or_cn(domain, qtype, pool).await;
            }
            if qtype == QTYPE_A {
                return Ok(vec![IpAddr::V4(pool.allocate(domain))]);
            }
            // AAAA (and every other type): NODATA under pure fake-IP mode.
            return Ok(Vec::new());
        }

        // Check cache — fresh hits (positive and negative) need only a read lock.
        if self.config.cache_enabled {
            let now = Instant::now();
            let key: CacheKey = (domain.to_string(), qtype);
            let cached = {
                let cache = self.cache.read().await;
                cache.peek(&key).and_then(|entry| {
                    if entry.expires_at > now {
                        Some((entry.addresses.clone(), false))
                    } else if !entry.addresses.is_empty()
                        && now.duration_since(entry.expires_at) < STALE_WHILE_REVALIDATE
                    {
                        // Positive entry past TTL but within the stale window.
                        Some((entry.addresses.clone(), true))
                    } else {
                        None
                    }
                })
            };
            match cached {
                Some((addrs, false)) => return Ok(addrs),
                Some((addrs, true)) => {
                    // Stale-while-revalidate: serve the stale answer now and
                    // refresh in the background through the normal resolve
                    // path so in-flight dedup applies (no duplicate query if
                    // a foreground resolve for the same key is running).
                    if let Ok(slot) = self.refresh_slots.clone().try_acquire_owned() {
                        let resolver = self.clone();
                        let domain_owned = domain.to_string();
                        tokio::spawn(async move {
                            let _slot = slot;
                            let _ = resolver.resolve_dedup(&domain_owned, qtype).await;
                        });
                    }
                    return Ok(addrs);
                }
                None => {}
            }
        }

        self.resolve_dedup(domain, qtype).await
    }

    /// Primary + fake-IP + GeoIP: resolve-then-geoip, then real CN or fake.
    async fn resolve_primary_fakeip_or_cn(
        &self,
        domain: &str,
        qtype: u16,
        pool: &FakeIpPool,
    ) -> Result<Vec<IpAddr>> {
        let key = domain.to_lowercase();
        if let Some(cn) = {
            let cache = self.cn_direct.read().await;
            cache.peek(&key).copied()
        } {
            return if cn {
                self.answer_cn_direct(domain, qtype, pool).await
            } else {
                Ok(Self::answer_fakeip(pool, domain, qtype))
            };
        }

        // Probe A on the clean primary upstream (ProxyTls / 8.8.8.8) — same
        // path international clients need, without re-entering this hook.
        let probe = self.resolve_dedup(domain, QTYPE_A).await?;
        let cn = self.ips_are_cn(&probe);
        {
            let mut cache = self.cn_direct.write().await;
            cache.put(key, cn);
        }

        if cn {
            self.record_probe_domestic(domain, pool, &probe);
            if qtype == QTYPE_A {
                return Ok(probe);
            }
            return self.answer_cn_direct(domain, qtype, pool).await;
        }
        Ok(Self::answer_fakeip(pool, domain, qtype))
    }

    fn answer_fakeip(pool: &FakeIpPool, domain: &str, qtype: u16) -> Vec<IpAddr> {
        if qtype == QTYPE_A {
            vec![IpAddr::V4(pool.allocate(domain))]
        } else {
            // NODATA for AAAA so dual-stack clients use the fake A answer.
            Vec::new()
        }
    }

    async fn answer_cn_direct(
        &self,
        domain: &str,
        qtype: u16,
        pool: &FakeIpPool,
    ) -> Result<Vec<IpAddr>> {
        let addrs = self.resolve_dedup(domain, qtype).await?;
        self.record_probe_domestic(domain, pool, &addrs);
        Ok(addrs)
    }

    fn ips_are_cn(&self, addrs: &[IpAddr]) -> bool {
        let Some(geo) = self.geoip.as_ref() else {
            return false;
        };
        addrs.iter().any(|ip| geo.is_cn(*ip))
    }

    fn record_probe_domestic(&self, domain: &str, pool: &FakeIpPool, addrs: &[IpAddr]) {
        for addr in addrs {
            pool.record_domestic(*addr, domain);
        }
    }

    /// Resolve with in-flight dedup: concurrent resolves for the same
    /// (domain, qtype) share one upstream query. Writes the outcome
    /// (positive or negative) to the cache.
    async fn resolve_dedup(&self, domain: &str, qtype: u16) -> Result<Vec<IpAddr>> {
        let key: CacheKey = (domain.to_string(), qtype);
        let cell = {
            let mut inflight = self.inflight.lock().unwrap();
            inflight
                .entry(key.clone())
                .or_insert_with(|| Arc::new(tokio::sync::OnceCell::new()))
                .clone()
        };

        let guard = InflightGuard {
            map: self.inflight.clone(),
            key: key.clone(),
            cell,
        };
        // Only the first caller runs resolve_uncached(); others wait for its result.
        let result = guard
            .cell
            .get_or_init(|| async {
                self.resolve_uncached(domain, qtype)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await;

        let result = result.clone().map_err(|e| anyhow::anyhow!("{}", e));

        if self.config.cache_enabled {
            let (addresses, ttl) = match &result {
                Ok(answer) if !answer.addresses.is_empty() => {
                    let ttl = if self.config.cache_ttl > 0 {
                        self.config.cache_ttl
                    } else {
                        answer.ttl
                    };
                    (answer.addresses.clone(), Duration::from_secs(ttl as u64))
                }
                _ => (Vec::new(), NEGATIVE_CACHE_TTL),
            };
            if ttl.is_zero() {
                self.cache.write().await.pop(&key);
            } else {
                self.cache.write().await.put(
                    key,
                    CacheEntry {
                        addresses,
                        expires_at: Instant::now() + ttl,
                    },
                );
            }
        }
        result.map(|answer| answer.addresses)
    }

    async fn resolve_uncached(&self, domain: &str, qtype: u16) -> Result<LookupAnswer> {
        let query = build_dns_query(domain, qtype);
        let reply = self.exchange_raw(&query).await?;
        let addresses = parse_dns_response(&reply)?
            .into_iter()
            .filter(|ip| match qtype {
                1 => ip.is_ipv4(),
                28 => ip.is_ipv6(),
                _ => true,
            })
            .collect();
        Ok(LookupAnswer {
            addresses,
            ttl: wire::answer_ttl(&reply)?,
        })
    }

    /// Resolve to first IPv4 address.
    pub async fn resolve_ipv4(&self, domain: &str) -> Result<Ipv4Addr> {
        let addrs = self.resolve_with_qtype(domain, QTYPE_A).await?;
        addrs
            .into_iter()
            .find_map(|a| match a {
                IpAddr::V4(v4) => Some(v4),
                _ => None,
            })
            .ok_or_else(|| anyhow::anyhow!("No IPv4 address for {}", domain))
    }

    /// Resolve to first IPv6 address.
    ///
    /// Mirrors [`Self::resolve_ipv4`], issuing a real AAAA query (qtype 28)
    /// with its own cache entry.
    pub async fn resolve_ipv6(&self, domain: &str) -> Result<Ipv6Addr> {
        let addrs = self.resolve_with_qtype(domain, QTYPE_AAAA).await?;
        addrs
            .into_iter()
            .find_map(|a| match a {
                IpAddr::V6(v6) => Some(v6),
                _ => None,
            })
            .ok_or_else(|| anyhow::anyhow!("No IPv6 address for {}", domain))
    }

    /// Clear the DNS cache.
    pub async fn clear_cache(&self) {
        let snapshot = self.query_snapshot();
        let mut cache = snapshot.cache.write().await;
        cache.clear();
    }

    /// Get cache statistics: (total_entries, expired_entries).
    pub async fn cache_stats(&self) -> (usize, usize) {
        let snapshot = self.query_snapshot();
        let cache = snapshot.cache.read().await;
        let now = Instant::now();
        let total = cache.len();
        let expired = cache.iter().filter(|(_, e)| e.expires_at <= now).count();
        (total, expired)
    }

    fn match_domain_group(&self, domain: &str) -> DnsGroup {
        // Proxy-server hostnames are always direct: they are the tunnel
        // endpoints, must resolve to real addresses via domestic upstreams,
        // and must never receive fake-IP answers (router-local tooling like
        // the Web UI speedtest resolves them through this pipeline).
        if self.is_server_domain(domain) {
            return DnsGroup::Fallback;
        }
        // Longest suffix match, without allocating the suffix strings:
        // check the full domain first, then each dot-delimited suffix.
        if let Some(group) = self.config.domain_rules.get(domain) {
            return group.clone();
        }
        for (i, b) in domain.bytes().enumerate() {
            if b == b'.'
                && let Some(group) = self.config.domain_rules.get(&domain[i + 1..])
            {
                return group.clone();
            }
        }
        DnsGroup::Primary
    }
}

// --- DNS wire format helpers ---

/// Build the TLS connector used for DNS-over-TLS through the proxy.
///
/// Root store comes from `webpki-roots` (compiled in): the static musl
/// binary on routers has no system CA bundle to fall back on.
/// The crypto provider is pinned to ring explicitly: other crates in the
/// build graph (QUIC stacks) also enable aws-lc-rs, and rustls panics when
/// it cannot auto-pick a process-level default provider.
fn build_dot_tls_connector() -> tokio_rustls::TlsConnector {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("ring supports rustls' safe default protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    tokio_rustls::TlsConnector::from(Arc::new(config))
}

/// Build a minimal DNS query packet for the given query type (1 = A, 28 = AAAA).
/// China domain suffixes for DNS splitting (same curated list as routing).
///
/// Names on this list use Fallback (domestic resolvers + real IPs, never
/// fake-IP). Everything else is Primary and may get a fake-IP in TUN mode.
/// Must stay in sync with [`crate::router::china_domain_suffix_list`] —
/// otherwise CDN hostnames of domestic apps miss the list, get fake-IP,
/// and Rule mode dials them through the proxy.
pub fn china_domain_suffixes() -> Vec<String> {
    crate::router::china_domain_suffix_list()
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

/// Resolve `domain` for an A record via a plain UDP nameserver.
///
/// Used when system DNS would return a Sockrocket fake-IP (Merlin LAN
/// hijack loops back into this daemon) and GeoIP needs a real address.
pub(crate) async fn lookup_a_udp(server: SocketAddr, domain: &str) -> Option<IpAddr> {
    let query = build_dns_query(domain, QTYPE_A);
    let socket = UdpSocket::bind("0.0.0.0:0").await.ok()?;
    crate::proxy::outbound_bind::apply_to_udp_socket(&socket, server.is_ipv6()).ok()?;
    socket.send_to(&query, server).await.ok()?;
    let mut buf = [0u8; 512];
    let (n, _) = tokio::time::timeout(Duration::from_secs(2), socket.recv_from(&mut buf))
        .await
        .ok()?
        .ok()?;
    let addrs = parse_dns_response(&buf[..n]).ok()?;
    addrs.into_iter().find(|a| a.is_ipv4())
}

/// Create a DnsConfig with China DNS splitting.
/// China domains use domestic DNS (223.5.5.5, 119.29.29.29),
/// others use international DNS (8.8.8.8, 1.1.1.1).
pub fn china_split_dns_config() -> DnsConfig {
    let mut rules = HashMap::new();
    for suffix in china_domain_suffixes() {
        rules.insert(suffix, DnsGroup::Fallback);
    }

    DnsConfig {
        servers: vec![
            DnsServer::Udp("8.8.8.8:53".parse().unwrap()),
            DnsServer::Udp("1.1.1.1:53".parse().unwrap()),
        ],
        fallback: vec![
            DnsServer::Udp("223.5.5.5:53".parse().unwrap()),
            DnsServer::Udp("119.29.29.29:53".parse().unwrap()),
        ],
        domain_rules: rules,
        cache_ttl: 600,
        cache_enabled: true,
    }
}

/// China-split config whose international group resolves via DNS-over-TLS
/// ([`DnsServer::ProxyTls`]) through the proxy node.
///
/// Two layers of poisoning are defended here: plain UDP to 8.8.8.8 from a
/// China WAN is GFW-poisoned (fake answers for google/youtube), and the
/// earlier plaintext DNS-over-TCP variant turned out to be hijacked by
/// relay exits themselves (observed: youtube resolved to an
/// unlock server with a mismatched cert — Chrome ERR_CERT_COMMON_NAME_INVALID;
/// wikipedia/zlibrary answered with GFW-pool fakes like 2001::1). TLS to port
/// 853 defeats both: it cannot be transparently hijacked or injected.
/// With a direct (no-node) outbound this degrades to plain DoT, which works
/// where DoT is not blocked. The caller must attach an outbound via
/// [`DnsResolver::set_outbound`].
pub fn china_split_dns_config_proxy_tls() -> DnsConfig {
    let mut config = china_split_dns_config();
    config.servers = vec![
        DnsServer::ProxyTls("8.8.8.8:853".parse().unwrap(), "dns.google".to_string()),
        DnsServer::ProxyTls(
            "1.1.1.1:853".parse().unwrap(),
            "cloudflare-dns.com".to_string(),
        ),
    ];
    config
}

/// Answer one raw DNS query using `resolver` (5s timeout), returning the
/// response bytes. On any resolution/parsing failure a SERVFAIL (rcode 2) or
/// FORMERR (rcode 1, malformed query) response is synthesized instead so the
/// caller can always answer the client.
///
/// Shared by the TUN port-53 interception and the standalone UDP DNS listener
/// (`serve_dns_udp`) used on the router, where dnsmasq forwards LAN queries to
/// this port for China-domain splitting.
pub async fn answer_raw_dns_query(resolver: &DnsResolver, query: &[u8]) -> Vec<u8> {
    if parse_dns_query(query).is_err() {
        return build_dns_error_response(query, 1);
    }
    let snapshot = resolver.query_snapshot();
    match tokio::time::timeout(Duration::from_secs(5), answer_wire(&snapshot, query)).await {
        Ok(Ok(reply)) => reply,
        _ => build_dns_error_response(query, 2),
    }
}

async fn answer_wire(resolver: &DnsResolver, query: &[u8]) -> Result<Vec<u8>> {
    let (domain, qtype) = parse_dns_query(query)?;
    let domain = domain.trim_end_matches('.').to_ascii_lowercase();
    let Some(pool) = &resolver.fakeip else {
        return resolver.exchange_raw(query).await;
    };
    if !matches!(qtype, 1 | 28) {
        return resolver.exchange_raw(query).await;
    }
    if resolver.match_domain_group(&domain) == DnsGroup::Fallback {
        let mut reply = resolver.exchange_raw(query).await?;
        wire::cap_ttl(&mut reply, 30)?;
        if let Ok(addresses) = parse_dns_response(&reply) {
            resolver.record_probe_domestic(&domain, pool, &addresses);
        }
        return Ok(reply);
    }
    // Keep negative responses intact; fake addresses only represent successful A answers.
    let probe_query = if qtype == 1 {
        query.to_vec()
    } else {
        build_dns_query(&domain, 1)
    };
    let mut probe = resolver.exchange_raw(&probe_query).await?;
    if probe[3] & 0x0f != 0 {
        if qtype == 1 {
            return Ok(probe);
        }
        return resolver.exchange_raw(query).await;
    }
    let addresses = parse_dns_response(&probe)?;
    if resolver.ips_are_cn(&addresses) {
        wire::cap_ttl(&mut probe, 30)?;
        resolver.record_probe_domestic(&domain, pool, &addresses);
        if qtype == 1 {
            return Ok(probe);
        }
        let reply = resolver.exchange_raw(query).await?;
        if let Ok(addresses) = parse_dns_response(&reply) {
            resolver.record_probe_domestic(&domain, pool, &addresses);
        }
        return Ok(reply);
    }
    let fake = if addresses.is_empty() || qtype != 1 {
        vec![]
    } else {
        vec![IpAddr::V4(pool.allocate(&domain))]
    };
    build_dns_response_with_ttl(query, &fake, 5)
}

/// Serve DNS over UDP on `bind_addr:port` until `shutdown` fires.
///
/// Binds the socket, then answers every datagram with [`answer_raw_dns_query`]
/// (one task per query so slow upstreams don't head-of-line-block other
/// clients). Used on AsusWRT-Merlin to back the dnsmasq split-DNS config,
/// which forwards LAN queries to `127.0.0.1:<dns_port>`.
pub async fn serve_dns_udp(
    bind_addr: &str,
    port: u16,
    resolver: DnsResolver,
    shutdown: Arc<tokio::sync::Notify>,
) -> Result<()> {
    listener::serve(bind_addr, port, resolver, shutdown).await
}

async fn serve_dns_socket(
    socket: UdpSocket,
    resolver: DnsResolver,
    shutdown: Arc<tokio::sync::Notify>,
    max_inflight: usize,
) -> Result<()> {
    let socket = Arc::new(socket);
    let mut queries = tokio::task::JoinSet::new();
    let mut buf = [0u8; 65535];

    loop {
        let (n, peer) = tokio::select! {
            biased;
            _ = shutdown.notified() => break,
            Some(_) = queries.join_next(), if !queries.is_empty() => continue,
            res = socket.recv_from(&mut buf) => res?,
        };
        if n < 12 {
            continue; // too short to be a DNS packet
        }
        // Upstream timeouts must not turn a LAN DNS burst into an
        // unbounded number of tasks waiting for resolver/pool slots.
        if queries.len() >= max_inflight {
            let response = build_dns_error_response(&buf[..n], 2);
            let _ = socket.send_to(&response, peer).await;
            continue;
        }
        let query = buf[..n].to_vec();
        let socket = socket.clone();
        let resolver = resolver.clone();
        queries.spawn(async move {
            let resp = wire::udp_response(&query, answer_raw_dns_query(&resolver, &query).await);
            if let Err(e) = socket.send_to(&resp, peer).await {
                tracing::debug!("DNS send to {peer} failed: {e}");
            }
        });
    }

    // In-flight tasks own socket clones. Cancel and reap them before
    // returning so shutdown releases the port even with stalled upstreams.
    queries.abort_all();
    while queries.join_next().await.is_some() {}
    tracing::info!("DNS listener stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_proxy_conn_pool_distributes_and_queues() {
        let pool = ProxyConnPool::new(2);
        let g1 = pool.acquire().await;
        let g2 = pool.acquire().await;
        // Both slots held: a third acquire must queue, not steal.
        let third = tokio::time::timeout(Duration::from_millis(50), pool.acquire()).await;
        assert!(
            third.is_err(),
            "third acquire should queue while all slots busy"
        );
        // Freeing a slot unblocks a waiting acquirer.
        drop(g1);
        let g3 = tokio::time::timeout(Duration::from_secs(1), pool.acquire())
            .await
            .expect("acquire succeeds after a slot is freed");
        drop(g2);
        drop(g3);
    }

    #[test]
    fn test_build_dns_query() {
        let query = build_dns_query("example.com", 1);
        assert!(query.len() > 12);
        // Check header flags
        assert_eq!(query[2], 0x01); // RD flag
        assert_eq!(query[4], 0x00);
        assert_eq!(query[5], 0x01); // QDCOUNT = 1
    }

    #[test]
    fn test_parse_dns_response_a_record() {
        // Minimal synthetic DNS response with one A record
        let mut resp = Vec::new();
        // Header
        resp.extend_from_slice(&[0x00, 0x01]); // ID
        resp.extend_from_slice(&[0x81, 0x80]); // Flags: response, recursion
        resp.extend_from_slice(&[0x00, 0x01]); // QDCOUNT
        resp.extend_from_slice(&[0x00, 0x01]); // ANCOUNT
        resp.extend_from_slice(&[0x00, 0x00]); // NSCOUNT
        resp.extend_from_slice(&[0x00, 0x00]); // ARCOUNT

        // Question: example.com A IN
        resp.push(7);
        resp.extend_from_slice(b"example");
        resp.push(3);
        resp.extend_from_slice(b"com");
        resp.push(0);
        resp.extend_from_slice(&[0x00, 0x01]); // A
        resp.extend_from_slice(&[0x00, 0x01]); // IN

        // Answer: pointer to name, A, IN, TTL=300, 4 bytes, 93.184.216.34
        resp.extend_from_slice(&[0xC0, 0x0C]); // Name pointer
        resp.extend_from_slice(&[0x00, 0x01]); // A
        resp.extend_from_slice(&[0x00, 0x01]); // IN
        resp.extend_from_slice(&[0x00, 0x00, 0x01, 0x2C]); // TTL = 300
        resp.extend_from_slice(&[0x00, 0x04]); // RDLENGTH = 4
        resp.extend_from_slice(&[93, 184, 216, 34]); // IP

        let addrs = parse_dns_response(&resp).unwrap();
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0], IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)));
    }

    #[test]
    fn test_dns_resolver_ip_passthrough() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let resolver = DnsResolver::new();
        let result = rt.block_on(resolver.resolve("1.2.3.4")).unwrap();
        assert_eq!(result, vec![IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4))]);
    }

    #[test]
    fn test_domain_group_matching() {
        let mut rules = HashMap::new();
        rules.insert("cn".to_string(), DnsGroup::Fallback);
        rules.insert("baidu.com".to_string(), DnsGroup::Fallback);

        let config = DnsConfig {
            domain_rules: rules,
            ..Default::default()
        };
        let resolver = DnsResolver::with_config(config);

        assert_eq!(
            resolver.match_domain_group("www.baidu.com"),
            DnsGroup::Fallback
        );
        assert_eq!(resolver.match_domain_group("test.cn"), DnsGroup::Fallback);
        assert_eq!(resolver.match_domain_group("google.com"), DnsGroup::Primary);
    }

    #[test]
    fn test_server_domains_forced_fallback() {
        let resolver_domains: Arc<std::sync::RwLock<std::collections::HashSet<String>>> = Arc::new(
            std::sync::RwLock::new(["node1.example.com".to_string()].into_iter().collect()),
        );
        let mut resolver = DnsResolver::with_config(DnsConfig::default());
        resolver.set_server_domains(resolver_domains.clone());

        // Exact match and subdomain are forced direct...
        assert_eq!(
            resolver.match_domain_group("node1.example.com"),
            DnsGroup::Fallback
        );
        assert_eq!(
            resolver.match_domain_group("a.node1.example.com"),
            DnsGroup::Fallback
        );
        // ...while lookalikes and unrelated names stay primary.
        assert_eq!(
            resolver.match_domain_group("not-node1.example.com.evil.com"),
            DnsGroup::Primary
        );
        assert_eq!(
            resolver.match_domain_group("example.com"),
            DnsGroup::Primary
        );
        assert_eq!(resolver.match_domain_group("google.com"), DnsGroup::Primary);

        // Hot update through the shared handle is visible without rebuilding.
        resolver_domains
            .write()
            .unwrap()
            .insert("jp2.example.com".to_string());
        assert_eq!(
            resolver.match_domain_group("jp2.example.com"),
            DnsGroup::Fallback
        );
    }

    #[test]
    fn test_china_split_config() {
        let config = china_split_dns_config();
        assert!(!config.domain_rules.is_empty());
        assert!(config.domain_rules.contains_key("baidu.com"));
        assert!(config.cache_enabled);
    }

    #[test]
    fn test_parse_error_response() {
        // NXDOMAIN response
        let mut resp = vec![0u8; 12];
        resp[2] = 0x81;
        resp[3] = 0x83; // rcode = 3 (NXDOMAIN)
        resp[5] = 0; // no questions

        let result = parse_dns_response(&resp);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_dns_query_and_build_response() {
        // Build a query for "example.com" A record
        let query = build_dns_query("example.com", 1);
        let (domain, qtype) = parse_dns_query(&query).unwrap();
        assert_eq!(domain, "example.com");
        assert_eq!(qtype, 1);

        // Build a response with one A record
        let addrs = vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))];
        let resp = build_dns_response(&query, &addrs).unwrap();

        // Verify it's a valid DNS response
        assert!(resp.len() > 12);
        // QR bit should be set (response)
        assert_eq!(resp[2] & 0x80, 0x80);
        // ANCOUNT should be 1
        assert_eq!(u16::from_be_bytes([resp[6], resp[7]]), 1);

        // Parse the response to extract the IP
        let parsed = parse_dns_response(&resp).unwrap();
        assert_eq!(parsed, addrs);
    }

    #[test]
    fn test_build_dns_error_response() {
        let query = build_dns_query("fail.example.com", 1);
        let resp = build_dns_error_response(&query, 2); // SERVFAIL
        assert!(resp.len() >= 12);
        assert_eq!(resp[3] & 0x0F, 2); // rcode = SERVFAIL
    }

    /// Mock UDP DNS server: answers A queries with `v4` and AAAA queries with
    /// `v6` (or NXDOMAIN when `nxdomain` is set). Returns its address and a
    /// counter of received queries.
    async fn spawn_mock_dns(
        v4: Vec<IpAddr>,
        v6: Vec<IpAddr>,
        nxdomain: bool,
    ) -> (SocketAddr, Arc<std::sync::atomic::AtomicUsize>) {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        let queries = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let queries_clone = queries.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 512];
            while let Ok((n, peer)) = socket.recv_from(&mut buf).await {
                queries_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let query = &buf[..n];
                let resp = match parse_dns_query(query) {
                    Ok((_, qtype)) if !nxdomain => {
                        let addrs = match qtype {
                            QTYPE_A => v4.clone(),
                            QTYPE_AAAA => v6.clone(),
                            _ => vec![],
                        };
                        build_dns_response(query, &addrs).unwrap()
                    }
                    Ok(_) => build_dns_error_response(query, 3), // NXDOMAIN
                    Err(_) => continue,
                };
                let _ = socket.send_to(&resp, peer).await;
            }
        });
        (addr, queries)
    }

    fn resolver_to(server: SocketAddr) -> DnsResolver {
        DnsResolver::with_config(DnsConfig {
            servers: vec![DnsServer::Udp(server)],
            fallback: vec![],
            domain_rules: HashMap::new(),
            cache_ttl: 0,
            cache_enabled: true,
        })
    }

    /// Mock DNS-over-TCP server: reads the 2-byte length prefix + wire query,
    /// answers A queries with `v4` and AAAA queries with `v6`.
    /// `DirectOutbound` plays the proxy role so this exercises the exact
    /// framing `query_proxy_tcp` uses against a real socket. Increments
    /// `conns` for every accepted TCP connection so tests can assert reuse.
    async fn spawn_mock_dns_tcp(
        v4: Vec<IpAddr>,
        v6: Vec<IpAddr>,
        conns: Arc<std::sync::atomic::AtomicUsize>,
    ) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let v4 = Arc::new(v4);
        let v6 = Arc::new(v6);
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                conns.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let v4 = v4.clone();
                let v6 = v6.clone();
                tokio::spawn(async move {
                    let mut len_buf = [0u8; 2];
                    loop {
                        if stream.read_exact(&mut len_buf).await.is_err() {
                            return;
                        }
                        let n = u16::from_be_bytes(len_buf) as usize;
                        let mut query = vec![0u8; n];
                        if stream.read_exact(&mut query).await.is_err() {
                            return;
                        }
                        let resp = match parse_dns_query(&query) {
                            Ok((_, qtype)) => {
                                let addrs = match qtype {
                                    QTYPE_A => (*v4).clone(),
                                    QTYPE_AAAA => (*v6).clone(),
                                    _ => vec![],
                                };
                                build_dns_response(&query, &addrs).unwrap()
                            }
                            Err(_) => return,
                        };
                        let mut framed = Vec::with_capacity(resp.len() + 2);
                        framed.extend_from_slice(&(resp.len() as u16).to_be_bytes());
                        framed.extend_from_slice(&resp);
                        if stream.write_all(&framed).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        addr
    }

    #[tokio::test]
    async fn proxy_tcp_resolves_through_outbound() {
        let v4 = vec![IpAddr::V4(Ipv4Addr::new(142, 250, 72, 46))];
        let v6 = vec![IpAddr::V6("2607:f8b0::1".parse().unwrap())];
        let conns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let server = spawn_mock_dns_tcp(v4.clone(), v6.clone(), conns.clone()).await;

        let mut resolver = DnsResolver::with_config(DnsConfig {
            servers: vec![DnsServer::ProxyTcp(server)],
            fallback: vec![],
            domain_rules: HashMap::new(),
            cache_ttl: 0,
            cache_enabled: true,
        });
        resolver.set_outbound(SharedOutbound::direct());

        let a = resolver.resolve("www.google.com").await.unwrap();
        assert_eq!(a, v4);

        let aaaa = resolver
            .resolve_with_qtype("www.google.com", QTYPE_AAAA)
            .await
            .unwrap();
        assert_eq!(aaaa, v6);

        // Pool semantics: sequential queries rotate through slots, so up to
        // PROXY_DNS_POOL_SIZE connections may exist — but never more, since
        // each slot reuses its own persistent connection.
        resolver.clear_cache().await;
        let a2 = resolver.resolve("mail.google.com").await.unwrap();
        assert_eq!(a2, v4);
        for i in 0..8 {
            resolver.clear_cache().await;
            resolver
                .resolve(&format!("host{i}.google.com"))
                .await
                .unwrap();
        }
        let total = conns.load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            (1..=PROXY_DNS_POOL_SIZE).contains(&total),
            "pool of {PROXY_DNS_POOL_SIZE} slots must bound connections, got {total}"
        );
    }

    #[tokio::test]
    async fn proxy_tcp_without_outbound_fails_fast() {
        let resolver = DnsResolver::with_config(DnsConfig {
            servers: vec![DnsServer::ProxyTcp("127.0.0.1:9".parse().unwrap())],
            fallback: vec![],
            domain_rules: HashMap::new(),
            cache_ttl: 0,
            cache_enabled: false,
        });

        let err = resolver.resolve("example.com").await.unwrap_err();
        assert!(
            err.to_string().contains("no outbound"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn proxy_tcp_truncated_response_is_error() {
        // Server that replies with a length prefix but fewer bytes than
        // announced: read_exact must fail rather than hang forever.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let mut len_buf = [0u8; 2];
                    if stream.read_exact(&mut len_buf).await.is_err() {
                        return;
                    }
                    let n = u16::from_be_bytes(len_buf) as usize;
                    let mut query = vec![0u8; n];
                    if stream.read_exact(&mut query).await.is_err() {
                        return;
                    }
                    // Lie about the length: prefix says 100, send only 3 bytes.
                    let _ = stream.write_all(&[0x00, 100, 0x81, 0x80, 0x00]).await;
                    let _ = tokio::time::sleep(Duration::from_secs(30)).await;
                });
            }
        });

        let mut resolver = DnsResolver::with_config(DnsConfig {
            servers: vec![DnsServer::ProxyTcp(addr)],
            fallback: vec![],
            domain_rules: HashMap::new(),
            cache_ttl: 0,
            cache_enabled: false,
        });
        resolver.set_outbound(SharedOutbound::direct());

        let start = Instant::now();
        let result = resolver.resolve("example.com").await;
        assert!(result.is_err());
        // Each attempt is bounded by the 8s exchange timeout; the retry on a
        // fresh connection doubles that, but must never hang indefinitely.
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "truncated response should error via timeouts, took {:?}",
            start.elapsed()
        );
    }

    #[tokio::test]
    async fn cache_hit_does_not_requery_upstream() {
        let ip = IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34));
        let (server, queries) = spawn_mock_dns(vec![ip], vec![], false).await;
        let resolver = resolver_to(server);

        let a = resolver.resolve("example.com").await.unwrap();
        let b = resolver.resolve("example.com").await.unwrap();
        assert_eq!(a, vec![ip]);
        assert_eq!(b, vec![ip]);
        assert_eq!(queries.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cache_is_keyed_by_qtype() {
        let v4 = IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34));
        let v6 = IpAddr::V6("2606:2800:220:1:248:1893:25c8:1946".parse().unwrap());
        let (server, queries) = spawn_mock_dns(vec![v4], vec![v6], false).await;
        let resolver = resolver_to(server);

        let a = resolver
            .resolve_with_qtype("example.com", QTYPE_A)
            .await
            .unwrap();
        let aaaa = resolver
            .resolve_with_qtype("example.com", QTYPE_AAAA)
            .await
            .unwrap();
        assert_eq!(a, vec![v4]);
        assert_eq!(aaaa, vec![v6]);
        // Distinct cache entries: one upstream query per qtype, then cached.
        let a2 = resolver
            .resolve_with_qtype("example.com", QTYPE_A)
            .await
            .unwrap();
        let aaaa2 = resolver
            .resolve_with_qtype("example.com", QTYPE_AAAA)
            .await
            .unwrap();
        assert_eq!(a2, vec![v4]);
        assert_eq!(aaaa2, vec![v6]);
        assert_eq!(queries.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn negative_answers_are_cached_briefly() {
        let (server, queries) = spawn_mock_dns(vec![], vec![], true).await; // NXDOMAIN
        let resolver = resolver_to(server);

        // First query fails (NXDOMAIN) and is cached as a negative entry.
        assert!(resolver.resolve("nope.example.com").await.is_err());
        // Second query is served from the negative cache: empty answer, no
        // new upstream query.
        let addrs = resolver.resolve("nope.example.com").await.unwrap();
        assert!(addrs.is_empty());
        assert_eq!(queries.load(std::sync::atomic::Ordering::SeqCst), 1);

        // A negative AAAA entry must not affect A queries for the same name.
        let (server2, queries2) =
            spawn_mock_dns(vec![IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4))], vec![], false).await;
        let resolver2 = resolver_to(server2);
        // Empty AAAA answer (NODATA) is a valid result, cached negative for
        // qtype 28 only.
        let aaaa = resolver2
            .resolve_with_qtype("mixed.example.com", QTYPE_AAAA)
            .await
            .unwrap();
        assert!(aaaa.is_empty());
        let a = resolver2
            .resolve_with_qtype("mixed.example.com", QTYPE_A)
            .await
            .unwrap();
        assert_eq!(a, vec![IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4))]);
        assert_eq!(queries2.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn expired_entry_serves_stale_and_revalidates() {
        let ip = IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34));
        let (server, queries) = spawn_mock_dns(vec![ip], vec![], false).await;
        let config = DnsConfig {
            servers: vec![DnsServer::Udp(server)],
            fallback: vec![],
            domain_rules: HashMap::new(),
            cache_ttl: 1, // expire after 1s
            cache_enabled: true,
        };
        let resolver = DnsResolver::with_config(config);

        assert_eq!(resolver.resolve("example.com").await.unwrap(), vec![ip]);
        // Let the entry expire (but stay within the stale window).
        tokio::time::sleep(Duration::from_millis(1100)).await;
        // Stale-while-revalidate: the stale answer is returned immediately...
        assert_eq!(resolver.resolve("example.com").await.unwrap(), vec![ip]);
        // ...and a background refresh re-queries upstream.
        for _ in 0..100 {
            if queries.load(std::sync::atomic::Ordering::SeqCst) >= 2 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("stale hit did not trigger a background refresh");
    }

    #[tokio::test]
    async fn cache_stats_and_clear() {
        let ip = IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34));
        let (server, _) = spawn_mock_dns(vec![ip], vec![], false).await;
        let resolver = resolver_to(server);

        resolver.resolve("example.com").await.unwrap();
        let (total, expired) = resolver.cache_stats().await;
        assert_eq!(total, 1);
        assert_eq!(expired, 0);
        resolver.clear_cache().await;
        assert_eq!(resolver.cache_stats().await, (0, 0));
    }

    #[tokio::test]
    async fn serve_dns_udp_answers_queries_and_stops() {
        let resolver = DnsResolver::with_config(china_split_dns_config());
        let shutdown = Arc::new(tokio::sync::Notify::new());

        // Bind an OS-assigned port to avoid collisions in CI.
        let probe = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);

        let shutdown_clone = shutdown.clone();
        let server = tokio::spawn(async move {
            serve_dns_udp("127.0.0.1", port, resolver, shutdown_clone)
                .await
                .unwrap();
        });
        // Give the listener a moment to bind.
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Send a well-formed A query for example.com. Offline, upstream
        // resolution will fail or time out; either way the listener must send
        // back a response with the same transaction ID (FORMERR/SERVFAIL is
        // fine — the contract under test is the UDP plumbing, not upstream).
        // Use tokio's socket: a blocking std socket here would stall the
        // current-thread test runtime that also drives the listener.
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mut query = Vec::new();
        query.extend_from_slice(&[0xAB, 0xCD]); // ID
        query.extend_from_slice(&[0x01, 0x00]); // RD
        query.extend_from_slice(&[0x00, 0x01]); // QDCOUNT
        query.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        query.push(7);
        query.extend_from_slice(b"example");
        query.push(3);
        query.extend_from_slice(b"com");
        query.push(0);
        query.extend_from_slice(&[0x00, 0x01]); // A
        query.extend_from_slice(&[0x00, 0x01]); // IN
        client
            .send_to(&query, format!("127.0.0.1:{port}"))
            .await
            .unwrap();

        let mut resp = vec![0u8; 512];
        let (n, _) = tokio::time::timeout(Duration::from_secs(10), client.recv_from(&mut resp))
            .await
            .expect("listener must answer every well-formed query")
            .unwrap();
        assert!(n >= 12, "response must be a DNS packet");
        assert_eq!(resp[0], 0xAB, "transaction ID must echo the query");
        // QR bit must be set (this is a response, not a forwarded query).
        assert_ne!(resp[2] & 0x80, 0, "QR bit must be set in the response");

        // Shutdown: the listener task must exit promptly.
        shutdown.notify_waiters();
        tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .expect("listener must stop on shutdown")
            .unwrap();
    }

    #[tokio::test]
    async fn fakeip_resolve_then_geoip_cn_returns_real_ip() {
        let cn_ip = IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4));
        let (server, _) = spawn_mock_dns(vec![cn_ip], vec![], false).await;
        let mut resolver = resolver_to(server);
        let pool = FakeIpPool::new();
        resolver.set_fakeip(pool.clone());
        let mut geo = GeoIpDb::new();
        geo.add_entry("1.2.3.0/24", "CN");
        geo.build();
        resolver.set_geoip(Arc::new(geo));

        // Domain is Primary (empty domain_rules) → would have been fake-IP
        // without geoip; with geoip:CN on the probe answer → real IP.
        let addrs = resolver.resolve("cdn.random-domestic.test").await.unwrap();
        assert_eq!(addrs, vec![cn_ip]);
        assert!(!resolver.is_fake(&addrs[0]));
        assert_eq!(
            pool.lookup_domestic(cn_ip).as_deref(),
            Some("cdn.random-domestic.test")
        );
    }

    #[tokio::test]
    async fn fakeip_resolve_then_geoip_non_cn_stays_fake() {
        let overseas = IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8));
        let (server, _) = spawn_mock_dns(vec![overseas], vec![], false).await;
        let mut resolver = resolver_to(server);
        let pool = FakeIpPool::new();
        resolver.set_fakeip(pool.clone());
        let mut geo = GeoIpDb::new();
        geo.add_entry("1.2.3.0/24", "CN");
        geo.build();
        resolver.set_geoip(Arc::new(geo));

        let addrs = resolver.resolve("www.google.test").await.unwrap();
        assert_eq!(addrs.len(), 1);
        assert!(resolver.is_fake(&addrs[0]));
        assert_eq!(
            pool.lookup(match addrs[0] {
                IpAddr::V4(v) => v,
                _ => panic!("expected v4 fake"),
            })
            .as_deref(),
            Some("www.google.test")
        );
    }
}

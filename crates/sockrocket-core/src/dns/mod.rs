//! DNS resolver module for sockrocket.
//!
//! Provides a configurable DNS resolver supporting:
//! - System DNS (uses OS resolver)
//! - Custom UDP DNS servers
//! - DNS-over-HTTPS (DoH) via standard wireformat
//! - DNS split: route China domains to local DNS, others to remote DNS

pub mod fakeip;

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

use self::fakeip::FakeIpPool;

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
#[derive(Debug, Clone)]
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
type InflightEntry = Arc<tokio::sync::OnceCell<Result<Vec<IpAddr>, String>>>;

/// Number of slots in each proxy-DNS connection pool. Four concurrent
/// international lookups cover a typical multi-domain page load without
/// stampeding the proxy node with dials.
const PROXY_DNS_POOL_SIZE: usize = 4;

/// A small pool of independently-locked proxy-DNS connection slots.
/// `acquire()` picks a free slot without waiting (round-robin + try_lock)
/// and only queues when every slot is busy.
struct ProxyConnPool {
    slots: Vec<Mutex<Option<BoxProxyStream>>>,
    cursor: std::sync::atomic::AtomicUsize,
}

impl ProxyConnPool {
    fn new(size: usize) -> Self {
        Self {
            slots: (0..size).map(|_| Mutex::new(None)).collect(),
            cursor: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    async fn acquire(&self) -> tokio::sync::MutexGuard<'_, Option<BoxProxyStream>> {
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
    inflight: Arc<Mutex<HashMap<CacheKey, InflightEntry>>>,
    /// Outbound used by [`DnsServer::ProxyTcp`] servers. `None` until
    /// [`Self::set_outbound`] is called; ProxyTcp queries fail fast with a
    /// clear error in that state.
    outbound: Option<SharedOutbound>,
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
            inflight: Arc::new(Mutex::new(HashMap::new())),
            outbound: None,
            proxy_tcp_conn: Arc::new(ProxyConnPool::new(PROXY_DNS_POOL_SIZE)),
            proxy_tls_conn: Arc::new(ProxyConnPool::new(PROXY_DNS_POOL_SIZE)),
            tls_connector: build_dot_tls_connector(),
            fakeip: None,
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
    /// queries for primary-group (international) domains are answered from
    /// the fake range; the TUN stream handler owns the same pool and dials
    /// the domain by name. Domestic (fallback-group) names still resolve
    /// normally so geoip-based direct routing keeps working for them.
    pub fn set_fakeip(&mut self, pool: Arc<FakeIpPool>) {
        self.fakeip = Some(pool);
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

        // Fake-IP interception (transparent proxy): international domains
        // (primary group) are answered from the fake pool instead of real
        // upstream resolution. The TUN stream handler maps the fake address
        // back to the domain and dials by NAME, which keeps the relay's
        // server-side DNS resolution (unlock model) alive — dialing the
        // client-resolved real IP fails on exits that can only reach their
        // own DNS-resolved unlock addresses. Domestic names never enter the
        // pool: geoip:CN → direct routing needs their real addresses.
        if let Some(pool) = &self.fakeip
            && self.match_domain_group(domain) == DnsGroup::Primary
        {
            if qtype == QTYPE_A {
                return Ok(vec![IpAddr::V4(pool.allocate(domain))]);
            }
            // AAAA (and every other type): NODATA, so dual-stack clients
            // fall back to the A answer with its fake IPv4 instead of
            // holding a real IPv6 that would bypass the domain-dial path.
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
                    let resolver = self.clone();
                    let domain_owned = domain.to_string();
                    tokio::spawn(async move {
                        let _ = resolver.resolve_dedup(&domain_owned, qtype).await;
                    });
                    return Ok(addrs);
                }
                None => {}
            }
        }

        self.resolve_dedup(domain, qtype).await
    }

    /// Resolve with in-flight dedup: concurrent resolves for the same
    /// (domain, qtype) share one upstream query. Writes the outcome
    /// (positive or negative) to the cache.
    async fn resolve_dedup(&self, domain: &str, qtype: u16) -> Result<Vec<IpAddr>> {
        let key: CacheKey = (domain.to_string(), qtype);
        let cell = {
            let mut inflight = self.inflight.lock().await;
            inflight
                .entry(key.clone())
                .or_insert_with(|| Arc::new(tokio::sync::OnceCell::new()))
                .clone()
        };

        // Only the first caller runs resolve_uncached(); others wait for its result.
        let result = cell
            .get_or_init(|| async {
                self.resolve_uncached(domain, qtype)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await;

        // Clean up inflight entry so future queries aren't stale
        {
            let mut inflight = self.inflight.lock().await;
            inflight.remove(&key);
        }

        let result = result.clone().map_err(|e| anyhow::anyhow!("{}", e));

        // Cache the outcome: positive answers get the configured TTL,
        // failures get a short negative TTL so repeated queries for a
        // failing name don't each pay the full timeout chain.
        if self.config.cache_enabled {
            let (addresses, ttl) = match &result {
                Ok(addrs) if !addrs.is_empty() => {
                    let ttl = if self.config.cache_ttl > 0 {
                        self.config.cache_ttl
                    } else {
                        300 // default 5 minutes
                    };
                    (addrs.clone(), Duration::from_secs(ttl as u64))
                }
                _ => (Vec::new(), NEGATIVE_CACHE_TTL),
            };
            let mut cache = self.cache.write().await;
            cache.put(
                key,
                CacheEntry {
                    addresses,
                    expires_at: Instant::now() + ttl,
                },
            );
        }

        result
    }

    /// Perform the actual DNS resolution (post-cache, no dedup).
    ///
    /// The matched server group is queried with all its servers racing — the
    /// first answer (even an empty/NODATA one, which is a legitimate reply)
    /// wins. There is deliberately NO cross-group fallback: an international
    /// domain whose primary group fails must NOT be re-queried against the
    /// domestic fallback, because domestic resolvers return GFW-poisoned
    /// fake records for exactly those domains (observed: 223.5.5.5 answers
    /// www.google.com with poisoned A/AAAA records). A clean SERVFAIL is
    /// strictly better than a poisoned answer.
    async fn resolve_uncached(&self, domain: &str, qtype: u16) -> Result<Vec<IpAddr>> {
        let group = self.match_domain_group(domain);
        let servers = match group {
            DnsGroup::Primary => &self.config.servers,
            DnsGroup::Fallback => &self.config.fallback,
        };

        self.race_group(servers, domain, qtype).await
    }

    /// Query all servers in a group concurrently; the first answer wins.
    ///
    /// An empty answer (NODATA) from a reachable server is a valid result and
    /// wins immediately — it must not fall through to another group (see
    /// [`Self::resolve_uncached`]).
    async fn race_group(
        &self,
        servers: &[DnsServer],
        domain: &str,
        qtype: u16,
    ) -> Result<Vec<IpAddr>> {
        if servers.is_empty() {
            bail!("No DNS servers configured");
        }

        let mut set = tokio::task::JoinSet::new();
        for server in servers {
            let resolver = self.clone();
            let server = server.clone();
            let domain = domain.to_string();
            set.spawn(async move { resolver.query_server(&server, &domain, qtype).await });
        }

        let mut last_err = None;
        while let Some(res) = set.join_next().await {
            match res {
                Ok(Ok(addrs)) => {
                    set.abort_all();
                    return Ok(addrs);
                }
                Ok(Err(e)) => last_err = Some(e),
                Err(e) => last_err = Some(anyhow::anyhow!("DNS query task failed: {}", e)),
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("No DNS servers configured")))
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
        let mut cache = self.cache.write().await;
        cache.clear();
    }

    /// Get cache statistics: (total_entries, expired_entries).
    pub async fn cache_stats(&self) -> (usize, usize) {
        let cache = self.cache.read().await;
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

    async fn query_server(
        &self,
        server: &DnsServer,
        domain: &str,
        qtype: u16,
    ) -> Result<Vec<IpAddr>> {
        match server {
            DnsServer::Udp(addr) => self.query_udp(*addr, domain, qtype).await,
            DnsServer::DoH(url) => self.query_doh(url, domain, qtype).await,
            DnsServer::ProxyTcp(addr) => self.query_proxy_tcp(*addr, domain, qtype).await,
            DnsServer::ProxyTls(addr, sni) => self.query_proxy_tls(*addr, sni, domain, qtype).await,
        }
    }

    /// Query DNS over TCP through the configured proxy outbound.
    ///
    /// Draws a slot from the resolver's connection pool: exchanges within a
    /// slot are serialized (each bounded by an 8s timeout), slots run
    /// concurrently. On any failure the slot's connection is dropped and the
    /// query retried once on a fresh connection.
    async fn query_proxy_tcp(
        &self,
        server: SocketAddr,
        domain: &str,
        qtype: u16,
    ) -> Result<Vec<IpAddr>> {
        let outbound = self
            .outbound
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("ProxyTcp DNS server configured but no outbound set"))?;
        let query = build_dns_query(domain, qtype);

        let mut conn = self.proxy_tcp_conn.acquire().await;
        let mut last_err: Option<anyhow::Error> = None;
        for attempt in 0..2 {
            if conn.is_none() {
                tracing::debug!(
                    "ProxyTcp: dialing {} for {} (attempt {})",
                    server,
                    domain,
                    attempt
                );
                match connect_upstream(outbound, &server.ip().to_string(), server.port()).await {
                    Ok(stream) => {
                        tracing::debug!("ProxyTcp: connected to {}", server);
                        *conn = Some(stream);
                    }
                    Err(e) => {
                        tracing::debug!("ProxyTcp: connect to {} failed: {:#}", server, e);
                        last_err = Some(e);
                        continue;
                    }
                }
            }
            let stream = conn.as_mut().expect("connection just ensured");

            match tokio::time::timeout(Duration::from_secs(8), dns_tcp_exchange(stream, &query))
                .await
            {
                Ok(Ok(addrs)) => return Ok(addrs),
                Ok(Err(e)) => {
                    tracing::debug!("ProxyTcp: exchange with {} failed: {:#}", server, e);
                    last_err = Some(e);
                }
                Err(_) => {
                    tracing::debug!("ProxyTcp: exchange with {} timed out", server);
                    last_err = Some(anyhow::anyhow!("ProxyTcp DNS query timeout"));
                }
            }
            // Broken connection (or a poisoned/stuck exchange): drop it so
            // the retry dials a fresh one.
            *conn = None;
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("ProxyTcp DNS query failed")))
    }

    /// Query DNS over TLS (RFC 7858) through the configured proxy outbound.
    ///
    /// Same pooled-connection discipline as [`Self::query_proxy_tcp`],
    /// plus a TLS handshake after the proxy dial. TLS is the whole point:
    /// relay exits that transparently hijack plaintext port-53 DNS
    /// cannot touch port 853 without a valid certificate for `sni`.
    async fn query_proxy_tls(
        &self,
        server: SocketAddr,
        sni: &str,
        domain: &str,
        qtype: u16,
    ) -> Result<Vec<IpAddr>> {
        let outbound = self
            .outbound
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("ProxyTls DNS server configured but no outbound set"))?;
        let query = build_dns_query(domain, qtype);
        let server_name = rustls_pki_types::ServerName::try_from(sni.to_owned())
            .map_err(|e| anyhow::anyhow!("invalid DoT SNI '{}': {}", sni, e))?;

        let mut conn = self.proxy_tls_conn.acquire().await;
        let mut last_err: Option<anyhow::Error> = None;
        for attempt in 0..2 {
            if conn.is_none() {
                tracing::debug!(
                    "ProxyTls: dialing {} (SNI {}) for {} (attempt {})",
                    server,
                    sni,
                    domain,
                    attempt
                );
                match connect_upstream(outbound, &server.ip().to_string(), server.port()).await {
                    Ok(stream) => {
                        match self
                            .tls_connector
                            .connect(server_name.clone(), stream)
                            .await
                        {
                            Ok(tls_stream) => {
                                tracing::debug!(
                                    "ProxyTls: TLS established to {} ({})",
                                    server,
                                    sni
                                );
                                *conn = Some(Box::new(tls_stream) as BoxProxyStream);
                            }
                            Err(e) => {
                                tracing::debug!(
                                    "ProxyTls: TLS handshake with {} failed: {:#}",
                                    server,
                                    e
                                );
                                last_err =
                                    Some(anyhow::anyhow!("DoT TLS handshake failed: {:#}", e));
                                continue;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::debug!("ProxyTls: connect to {} failed: {:#}", server, e);
                        last_err = Some(e);
                        continue;
                    }
                }
            }
            let stream = conn.as_mut().expect("connection just ensured");

            match tokio::time::timeout(Duration::from_secs(8), dns_tcp_exchange(stream, &query))
                .await
            {
                Ok(Ok(addrs)) => return Ok(addrs),
                Ok(Err(e)) => {
                    tracing::debug!("ProxyTls: exchange with {} failed: {:#}", server, e);
                    last_err = Some(e);
                }
                Err(_) => {
                    tracing::debug!("ProxyTls: exchange with {} timed out", server);
                    last_err = Some(anyhow::anyhow!("ProxyTls DNS query timeout"));
                }
            }
            *conn = None;
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("ProxyTls DNS query failed")))
    }

    /// Query DNS over UDP.
    async fn query_udp(&self, server: SocketAddr, domain: &str, qtype: u16) -> Result<Vec<IpAddr>> {
        let query = build_dns_query(domain, qtype);
        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        socket.send_to(&query, server).await?;

        let mut buf = [0u8; 512];
        let timeout = tokio::time::timeout(Duration::from_secs(3), socket.recv_from(&mut buf));
        let (n, _) = timeout
            .await
            .map_err(|_| anyhow::anyhow!("DNS query timeout"))??;

        parse_dns_response(&buf[..n])
    }

    /// Query DNS over HTTPS (DoH) using wireformat.
    async fn query_doh(&self, url: &str, domain: &str, qtype: u16) -> Result<Vec<IpAddr>> {
        let query = build_dns_query(domain, qtype);

        let resp = self
            .doh_client
            .post(url)
            .header("Content-Type", "application/dns-message")
            .header("Accept", "application/dns-message")
            .body(query)
            .send()
            .await?;

        if !resp.status().is_success() {
            bail!("DoH server returned status {}", resp.status());
        }

        let body = resp.bytes().await?;
        parse_dns_response(&body)
    }
}

// --- DNS wire format helpers ---

/// One RFC 7858-style DNS exchange over a (possibly TLS-wrapped) TCP stream:
/// sends `[u16 length][query]`, reads the framed reply, parses it.
/// Shared by the plain (`ProxyTcp`) and TLS (`ProxyTls`) proxy paths.
async fn dns_tcp_exchange(stream: &mut BoxProxyStream, query: &[u8]) -> Result<Vec<IpAddr>> {
    let mut framed = Vec::with_capacity(query.len() + 2);
    framed.extend_from_slice(&(query.len() as u16).to_be_bytes());
    framed.extend_from_slice(query);
    stream.write_all(&framed).await?;

    let mut len_buf = [0u8; 2];
    stream.read_exact(&mut len_buf).await?;
    let resp_len = u16::from_be_bytes(len_buf) as usize;
    if resp_len == 0 {
        bail!("Empty DNS-over-TCP response");
    }
    let mut resp = vec![0u8; resp_len];
    stream.read_exact(&mut resp).await?;

    // Orphan-response guard. The exchange runs on a SHARED connection, and a
    // racing query that lost `race_group` is aborted mid-exchange — its query
    // was already written, so its response arrives LATE and sits in the read
    // buffer. Without ID validation the NEXT exchange (any domain) reads that
    // stale response, caches it under the wrong name for 600s, and clients
    // connect to a different site's IP — observed as github.com
    // answering with Google IPs and youtube serving Apple IPs, producing
    // ERR_CERT_COMMON_NAME_INVALID all over the LAN. A mismatched ID means
    // the stream is desynced by at least one orphan; bail and let the caller
    // drop the connection and redial (both proxy exchange paths do).
    if resp.len() >= 2 && query.len() >= 2 && resp[0..2] != query[0..2] {
        bail!(
            "DNS response ID mismatch (query {:02x}{:02x}, got {:02x}{:02x}) — stale orphan response on shared connection",
            query[0],
            query[1],
            resp[0],
            resp[1]
        );
    }

    parse_dns_response(&resp)
}

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
fn build_dns_query(domain: &str, qtype: u16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64);

    // Header
    let id: u16 = rand::random();
    buf.extend_from_slice(&id.to_be_bytes()); // ID
    buf.extend_from_slice(&[0x01, 0x00]); // Flags: standard query, recursion desired
    buf.extend_from_slice(&[0x00, 0x01]); // QDCOUNT: 1
    buf.extend_from_slice(&[0x00, 0x00]); // ANCOUNT: 0
    buf.extend_from_slice(&[0x00, 0x00]); // NSCOUNT: 0
    buf.extend_from_slice(&[0x00, 0x00]); // ARCOUNT: 0

    // Question section
    for label in domain.split('.') {
        let bytes = label.as_bytes();
        buf.push(bytes.len() as u8);
        buf.extend_from_slice(bytes);
    }
    buf.push(0x00); // Root label

    buf.extend_from_slice(&qtype.to_be_bytes()); // QTYPE
    buf.extend_from_slice(&[0x00, 0x01]); // QCLASS: IN

    buf
}

/// Parse a DNS response and extract IP addresses.
fn parse_dns_response(data: &[u8]) -> Result<Vec<IpAddr>> {
    if data.len() < 12 {
        bail!("DNS response too short");
    }

    let ancount = u16::from_be_bytes([data[6], data[7]]) as usize;
    let rcode = data[3] & 0x0F;

    if rcode != 0 {
        bail!("DNS response error: rcode={}", rcode);
    }

    // Skip header (12 bytes) and question section
    let mut pos = 12;

    // Skip question section (QDCOUNT = data[4..6])
    let qdcount = u16::from_be_bytes([data[4], data[5]]) as usize;
    for _ in 0..qdcount {
        pos = skip_dns_name(data, pos)?;
        pos += 4; // QTYPE + QCLASS
    }

    // Parse answer section
    let mut addrs = Vec::new();
    for _ in 0..ancount {
        if pos >= data.len() {
            break;
        }
        pos = skip_dns_name(data, pos)?;
        if pos + 10 > data.len() {
            break;
        }

        let rtype = u16::from_be_bytes([data[pos], data[pos + 1]]);
        let rdlength = u16::from_be_bytes([data[pos + 8], data[pos + 9]]) as usize;
        pos += 10;

        if pos + rdlength > data.len() {
            break;
        }

        match rtype {
            1 if rdlength == 4 => {
                // A record
                let ip = Ipv4Addr::new(data[pos], data[pos + 1], data[pos + 2], data[pos + 3]);
                addrs.push(IpAddr::V4(ip));
            }
            28 if rdlength == 16 => {
                // AAAA record
                let mut octets = [0u8; 16];
                octets.copy_from_slice(&data[pos..pos + 16]);
                addrs.push(IpAddr::V6(octets.into()));
            }
            _ => {}
        }
        pos += rdlength;
    }

    Ok(addrs)
}

/// Skip a DNS name (handles compression pointers).
#[allow(unused_assignments)]
fn skip_dns_name(data: &[u8], mut pos: usize) -> Result<usize> {
    let mut jumped = false;
    loop {
        if pos >= data.len() {
            bail!("DNS name extends beyond packet");
        }
        let len = data[pos] as usize;
        if len == 0 {
            if !jumped {
                pos += 1;
            }
            break;
        }
        if len & 0xC0 == 0xC0 {
            // Compression pointer
            if !jumped {
                pos += 2;
                jumped = true;
            }
            break;
        }
        if !jumped {
            pos += 1 + len;
        } else {
            break;
        }
    }
    Ok(pos)
}

/// Extract the queried domain name and query type from a raw DNS packet.
///
/// Returns `(domain, qtype)` where qtype is 1 for A, 28 for AAAA, etc.
pub fn parse_dns_query(data: &[u8]) -> Result<(String, u16)> {
    if data.len() < 12 {
        bail!("DNS packet too short");
    }
    let mut pos = 12; // skip header
    let mut domain = String::new();
    loop {
        if pos >= data.len() {
            bail!("DNS question extends beyond packet");
        }
        let len = data[pos] as usize;
        if len == 0 {
            pos += 1;
            break;
        }
        if len & 0xC0 == 0xC0 {
            bail!("Unexpected compression pointer in question");
        }
        pos += 1;
        if pos + len > data.len() {
            bail!("DNS label extends beyond packet");
        }
        if !domain.is_empty() {
            domain.push('.');
        }
        domain.push_str(&String::from_utf8_lossy(&data[pos..pos + len]));
        pos += len;
    }
    if pos + 4 > data.len() {
        bail!("DNS question QTYPE/QCLASS missing");
    }
    let qtype = u16::from_be_bytes([data[pos], data[pos + 1]]);
    Ok((domain, qtype))
}

/// Build a DNS response packet from a query packet and a set of resolved IPs.
///
/// Preserves the query ID and question section, adds A/AAAA answers with
/// the default TTL (300s).
pub fn build_dns_response(query: &[u8], addresses: &[IpAddr]) -> Result<Vec<u8>> {
    build_dns_response_with_ttl(query, addresses, 300)
}

/// Like [`build_dns_response`] but with an explicit answer TTL.
///
/// Fake-IP answers need a short TTL (a few seconds): the fake address is
/// only meaningful while the pool remembers the mapping, and long client /
/// dnsmasq caching would outlive evictions and resolvers being restarted.
pub fn build_dns_response_with_ttl(
    query: &[u8],
    addresses: &[IpAddr],
    ttl: u32,
) -> Result<Vec<u8>> {
    if query.len() < 12 {
        bail!("Query packet too short to build response");
    }

    // Find end of question section
    let mut qend = 12;
    loop {
        if qend >= query.len() {
            bail!("Malformed DNS query: question section overflows");
        }
        let len = query[qend] as usize;
        if len == 0 {
            qend += 1; // null terminator
            break;
        }
        if len & 0xC0 == 0xC0 {
            qend += 2;
            break;
        }
        qend += 1 + len;
    }
    qend += 4; // QTYPE + QCLASS

    let an_count = addresses.len() as u16;
    let mut resp = Vec::with_capacity(qend + addresses.len() * 16 + 32);

    // Copy header from query
    resp.extend_from_slice(&query[..2]); // ID

    // Flags: response, recursion desired+available, no error
    resp.extend_from_slice(&[0x81, 0x80]);

    // QDCOUNT = 1
    resp.extend_from_slice(&[0x00, 0x01]);
    // ANCOUNT
    resp.extend_from_slice(&an_count.to_be_bytes());
    // NSCOUNT, ARCOUNT = 0
    resp.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);

    // Copy question section from query
    resp.extend_from_slice(&query[12..qend]);

    // Answer records
    for addr in addresses {
        // Name pointer to question
        resp.extend_from_slice(&[0xC0, 0x0C]);
        match addr {
            IpAddr::V4(v4) => {
                resp.extend_from_slice(&[0x00, 0x01]); // A
                resp.extend_from_slice(&[0x00, 0x01]); // IN
                resp.extend_from_slice(&ttl.to_be_bytes());
                resp.extend_from_slice(&[0x00, 0x04]); // RDLENGTH
                resp.extend_from_slice(&v4.octets());
            }
            IpAddr::V6(v6) => {
                resp.extend_from_slice(&[0x00, 0x1C]); // AAAA
                resp.extend_from_slice(&[0x00, 0x01]); // IN
                resp.extend_from_slice(&ttl.to_be_bytes());
                resp.extend_from_slice(&[0x00, 0x10]); // RDLENGTH
                resp.extend_from_slice(&v6.octets());
            }
        }
    }

    Ok(resp)
}

/// Build a DNS NXDOMAIN (or SERVFAIL) error response from a query packet.
pub fn build_dns_error_response(query: &[u8], rcode: u8) -> Vec<u8> {
    if query.len() < 12 {
        return Vec::new();
    }
    let mut resp = query.to_vec();
    // Set QR=1 (response), keep RD, set RA, set rcode
    resp[2] = 0x81;
    resp[3] = 0x80 | (rcode & 0x0F);
    // Zero out ANCOUNT
    resp[6] = 0;
    resp[7] = 0;
    resp
}

/// Common China domain suffixes for DNS splitting.
pub fn china_domain_suffixes() -> Vec<String> {
    vec![
        "cn".to_string(),
        "com.cn".to_string(),
        "net.cn".to_string(),
        "org.cn".to_string(),
        "baidu.com".to_string(),
        "qq.com".to_string(),
        "taobao.com".to_string(),
        "tmall.com".to_string(),
        "jd.com".to_string(),
        "alipay.com".to_string(),
        "weibo.com".to_string(),
        "bilibili.com".to_string(),
        "zhihu.com".to_string(),
        "163.com".to_string(),
        "douyin.com".to_string(),
        "tiktok.com".to_string(),
        "sina.com".to_string(),
        "sohu.com".to_string(),
        "csdn.net".to_string(),
        "aliyun.com".to_string(),
        // Domestic services on non-.cn domains whose CDN ranges geoip
        // databases often miss — the DNS-split decision is the reliable
        // signal (see FakeIpPool::record_domestic).
    ]
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
    match parse_dns_query(query) {
        Ok((domain, qtype)) => {
            tracing::trace!("DNS: {} (type {})", domain, qtype);

            // Resolve with the client's query type: AAAA queries go upstream
            // as real AAAA queries and are cached per (domain, qtype).
            let timeout = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                resolver.resolve_with_qtype(&domain, qtype),
            );

            match timeout.await {
                Ok(Ok(addrs)) => {
                    // Defensive filter by query type: A (1) → IPv4 only,
                    // AAAA (28) → IPv6 only. Cache entries are already keyed
                    // by qtype; this guards against mixed upstream answers.
                    let filtered: Vec<IpAddr> = match qtype {
                        1 => addrs.into_iter().filter(|a| a.is_ipv4()).collect(),
                        28 => addrs.into_iter().filter(|a| a.is_ipv6()).collect(),
                        _ => addrs,
                    };
                    // Fake-IP answers get a short TTL: the address is only
                    // valid while the pool mapping lives, and client/dnsmasq
                    // caches must not outlive pool evictions or daemon
                    // restarts. Domestic real answers get a medium TTL: the
                    // domestic-IP table is process memory, so after a daemon
                    // restart a longer-cached answer would point at an
                    // address the new daemon no longer recognizes as
                    // domestic (and geoip would then misroute it).
                    let ttl = if filtered.iter().any(|a| resolver.is_fake(a)) {
                        5
                    } else if resolver.has_fakeip() {
                        30
                    } else {
                        300
                    };
                    match build_dns_response_with_ttl(query, &filtered, ttl) {
                        Ok(resp) => resp,
                        Err(e) => {
                            tracing::debug!("DNS response build error for {}: {}", domain, e);
                            build_dns_error_response(query, 2)
                        }
                    }
                }
                Ok(Err(e)) => {
                    tracing::debug!("DNS resolve error for {}: {}", domain, e);
                    build_dns_error_response(query, 2) // SERVFAIL
                }
                Err(_) => {
                    tracing::debug!("DNS timeout for {}", domain);
                    build_dns_error_response(query, 2) // SERVFAIL
                }
            }
        }
        Err(e) => {
            tracing::trace!("DNS parse error: {}", e);
            build_dns_error_response(query, 1) // FORMERR
        }
    }
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
    let socket = UdpSocket::bind((bind_addr, port)).await?;
    tracing::info!("DNS listener: udp://{bind_addr}:{port}");
    let socket = Arc::new(socket);

    loop {
        let mut buf = vec![0u8; 1500];
        let (n, peer) = tokio::select! {
            _ = shutdown.notified() => break,
            res = socket.recv_from(&mut buf) => res?,
        };
        if n < 12 {
            continue; // too short to be a DNS packet
        }
        let query = buf[..n].to_vec();
        let socket = socket.clone();
        let resolver = resolver.clone();
        tokio::spawn(async move {
            let resp = answer_raw_dns_query(&resolver, &query).await;
            if let Err(e) = socket.send_to(&resp, peer).await {
                tracing::debug!("DNS send to {peer} failed: {e}");
            }
        });
    }

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
            total >= 1 && total <= PROXY_DNS_POOL_SIZE,
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
}

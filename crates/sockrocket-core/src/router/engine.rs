use std::borrow::Cow;
use std::net::IpAddr;
use std::num::NonZeroUsize;
use std::sync::{Arc, RwLock};

use lru::LruCache;

use super::geoip::GeoIpDb;
use crate::config::model::RoutingRule;

/// Maximum number of cached route decisions.
const ROUTE_CACHE_CAP: usize = 4096;

/// Action to take for a matched connection.
#[derive(Debug, Clone, PartialEq)]
pub enum RouteAction {
    /// Use the configured proxy outbound.
    Proxy,
    /// Connect directly, bypassing proxy.
    Direct,
    /// Drop/reject the connection.
    Reject,
}

/// A compiled match rule for fast evaluation.
#[derive(Debug, Clone)]
pub enum MatchRule {
    /// Exact domain match: "example.com"
    Domain(String),
    /// Domain suffix match: ".example.com" matches "foo.example.com"
    DomainSuffix(String),
    /// Domain keyword match: contains "google"
    DomainKeyword(String),
    /// IPv4/IPv6 CIDR match
    IpCidr { addr: IpAddr, prefix_len: u8 },
    /// GeoIP country code (e.g., "CN", "US")
    GeoIp(String),
    /// Match all (catch-all rule)
    MatchAll,
}

/// A single rule with its action.
#[derive(Debug, Clone)]
pub struct RuleEntry {
    pub rule: MatchRule,
    pub action: RouteAction,
}

/// A set of routing rules, compiled for efficient matching.
#[derive(Debug, Clone)]
pub struct RuleSet {
    rules: Vec<RuleEntry>,
    default_action: RouteAction,
}

impl RuleSet {
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            default_action: RouteAction::Proxy,
        }
    }

    /// Set the default action when no rule matches.
    pub fn set_default(&mut self, action: RouteAction) {
        self.default_action = action;
    }

    /// Add a rule to the set.
    pub fn add_rule(&mut self, rule: MatchRule, action: RouteAction) {
        self.rules.push(RuleEntry { rule, action });
    }

    /// Build from config RoutingRules. Disabled rules are skipped.
    pub fn from_config(rules: &[RoutingRule]) -> Self {
        let mut set = Self::new();
        for r in rules {
            if !r.enabled {
                continue;
            }
            let action = match r.target.to_lowercase().as_str() {
                "direct" => RouteAction::Direct,
                "reject" | "block" => RouteAction::Reject,
                _ => RouteAction::Proxy,
            };

            let rule = match r.rule_type.to_lowercase().as_str() {
                "domain" => MatchRule::Domain(r.pattern.to_lowercase()),
                "domain-suffix" => {
                    let suffix = r.pattern.to_lowercase();
                    MatchRule::DomainSuffix(if suffix.starts_with('.') {
                        suffix
                    } else {
                        format!(".{}", suffix)
                    })
                }
                "domain-keyword" => MatchRule::DomainKeyword(r.pattern.to_lowercase()),
                "ip-cidr" => match parse_cidr(&r.pattern) {
                    Some((addr, prefix_len)) => MatchRule::IpCidr { addr, prefix_len },
                    None => {
                        tracing::warn!("Invalid CIDR: {}", r.pattern);
                        continue;
                    }
                },
                "geoip" => MatchRule::GeoIp(r.pattern.to_uppercase()),
                "match" | "final" => MatchRule::MatchAll,
                _ => {
                    tracing::warn!("Unknown rule type: {}", r.rule_type);
                    continue;
                }
            };

            set.add_rule(rule, action);
        }
        set
    }

    pub fn rules(&self) -> &[RuleEntry] {
        &self.rules
    }
}

impl Default for RuleSet {
    fn default() -> Self {
        Self::new()
    }
}

/// Cache key for route lookups: rule evaluation lowercases the host
/// before matching, so the cache must be keyed by the lowercased host too
/// (otherwise "EXAMPLE.com" and "example.com" are two entries). Borrows
/// when the host is already lowercase ASCII — the common case — avoiding
/// an allocation on the hit fast path.
fn cache_key(host: &str) -> Cow<'_, str> {
    if host.is_ascii() && !host.bytes().any(|b| b.is_ascii_uppercase()) {
        Cow::Borrowed(host)
    } else {
        Cow::Owned(host.to_lowercase())
    }
}

/// Router evaluates rules against connection targets.
pub struct Router {
    rules: RuleSet,
    geoip: Option<Arc<GeoIpDb>>,
    /// LRU route cache: evicts least-recently-used entries when full (avoids thundering-herd on full flush).
    /// Keyed by lowercased host (see [`cache_key`]).
    cache: RwLock<LruCache<String, RouteAction>>,
}

impl Router {
    pub fn new(rules: RuleSet) -> Self {
        Self {
            rules,
            geoip: None,
            cache: RwLock::new(LruCache::new(NonZeroUsize::new(ROUTE_CACHE_CAP).unwrap())),
        }
    }

    pub fn with_geoip(mut self, geoip: Arc<GeoIpDb>) -> Self {
        self.geoip = Some(geoip);
        self
    }

    /// Evaluate routing rules for a given destination.
    /// `host` can be a domain name or IP address string.
    pub fn route(&self, host: &str, port: u16) -> RouteAction {
        let key = cache_key(host);
        // get() (not peek()) so a hit promotes the entry in LRU order —
        // without promotion a hot route stays evictable under cache churn.
        // This needs the write lock, but it is a std RwLock held only for
        // the lookup, so it is cheap.
        if let Ok(mut cache) = self.cache.write()
            && let Some(action) = cache.get(key.as_ref())
        {
            return action.clone();
        }

        let action = self.route_uncached(host, port);

        // Store in cache — LruCache::put auto-evicts the LRU entry when full
        if let Ok(mut cache) = self.cache.write() {
            cache.put(key.into_owned(), action.clone());
        }

        action
    }

    /// Perform rule matching without cache.
    fn route_uncached(&self, host: &str, port: u16) -> RouteAction {
        let host_lower = host.to_lowercase();
        let ip: Option<IpAddr> = host.parse().ok();

        for entry in &self.rules.rules {
            if self.rule_matches(&entry.rule, &host_lower, ip) {
                tracing::debug!(
                    "Route match: {}:{} -> {:?} (rule: {:?})",
                    host,
                    port,
                    entry.action,
                    entry.rule
                );
                return entry.action.clone();
            }
        }

        self.rules.default_action.clone()
    }

    /// Evaluate only the DOMAIN rules (domain / domain-suffix /
    /// domain-keyword) for `host`; `None` when no domain rule matches.
    ///
    /// Unlike [`route`](Self::route) this never falls through to IP rules,
    /// the catch-all, or the default action — it answers the narrow
    /// question "did the user explicitly rule on this name?". The TUN
    /// layer uses it to let explicit user rules override the built-in
    /// domestic-IP direct dial without disturbing unrouted names. Not
    /// cached: callers invoke it once per new TUN stream, and rule
    /// evaluation is pure string matching.
    pub fn route_domain_only(&self, host: &str) -> Option<RouteAction> {
        let host_lower = host.to_lowercase();
        for entry in &self.rules.rules {
            let matched = match &entry.rule {
                MatchRule::Domain(_) | MatchRule::DomainSuffix(_) | MatchRule::DomainKeyword(_) => {
                    self.rule_matches(&entry.rule, &host_lower, None)
                }
                _ => false,
            };
            if matched {
                return Some(entry.action.clone());
            }
        }
        None
    }

    /// Route like [`route`](Self::route), but resolves domain hosts via the
    /// system DNS when evaluation reaches an IP-based rule (GeoIP / IP-CIDR).
    ///
    /// Rule order is preserved: domain rules are tried first, and a DNS
    /// lookup happens only if evaluation reaches an IP-based rule without a
    /// prior match — at most once per call. The result shares the route
    /// cache, so repeat connections to the same host never re-resolve.
    pub async fn route_resolving(&self, host: &str, port: u16) -> RouteAction {
        // Fast path: same cache as `route` (same lowercased keying, same
        // LRU promotion on hit).
        let key = cache_key(host);
        if let Ok(mut cache) = self.cache.write()
            && let Some(action) = cache.get(key.as_ref())
        {
            return action.clone();
        }

        let action = self.route_uncached_resolving(host, port).await;

        if let Ok(mut cache) = self.cache.write() {
            cache.put(key.into_owned(), action.clone());
        }

        action
    }

    async fn route_uncached_resolving(&self, host: &str, port: u16) -> RouteAction {
        let host_lower = host.to_lowercase();
        let mut ip: Option<IpAddr> = host.parse().ok();
        let mut dns_attempted = ip.is_some();

        for entry in &self.rules.rules {
            // Lazily resolve the first time evaluation reaches an IP-based
            // rule for a domain target.
            if !dns_attempted
                && matches!(entry.rule, MatchRule::IpCidr { .. } | MatchRule::GeoIp(_))
            {
                dns_attempted = true;
                ip = resolve_host(&host_lower).await;
            }

            if self.rule_matches(&entry.rule, &host_lower, ip) {
                tracing::debug!(
                    "Route match: {}:{} -> {:?} (rule: {:?})",
                    host,
                    port,
                    entry.action,
                    entry.rule
                );
                return entry.action.clone();
            }
        }

        self.rules.default_action.clone()
    }

    fn rule_matches(&self, rule: &MatchRule, host_lower: &str, ip: Option<IpAddr>) -> bool {
        match rule {
            MatchRule::Domain(domain) => host_lower == domain,

            MatchRule::DomainSuffix(suffix) => {
                host_lower.ends_with(suffix.as_str())
                    || host_lower == suffix.trim_start_matches('.')
            }

            MatchRule::DomainKeyword(keyword) => host_lower.contains(keyword.as_str()),

            MatchRule::IpCidr { addr, prefix_len } => {
                ip.is_some_and(|ip| cidr_match(ip, *addr, *prefix_len))
            }

            MatchRule::GeoIp(country) => {
                if let (Some(ip), Some(geoip)) = (ip, &self.geoip) {
                    geoip
                        .lookup(ip)
                        .is_some_and(|cc| cc.eq_ignore_ascii_case(country))
                } else {
                    false
                }
            }

            MatchRule::MatchAll => true,
        }
    }
}

/// Resolve a host via the system resolver, preferring IPv4 (the GeoIP
/// database is primarily v4). None on failure/timeout — IP-based rules then
/// simply don't match, and evaluation falls through to the default action.
async fn resolve_host(host: &str) -> Option<IpAddr> {
    let lookup = tokio::net::lookup_host((host, 0));
    let addrs = match tokio::time::timeout(std::time::Duration::from_secs(3), lookup).await {
        Ok(Ok(iter)) => iter,
        _ => return None,
    };
    let addrs: Vec<_> = addrs.collect();
    addrs
        .iter()
        .find(|sa| sa.is_ipv4())
        .or_else(|| addrs.first())
        .map(|sa| sa.ip())
}

/// Parse a CIDR string like "192.168.0.0/16" or "::1/128".
fn parse_cidr(cidr: &str) -> Option<(IpAddr, u8)> {
    let parts: Vec<&str> = cidr.splitn(2, '/').collect();
    if parts.len() != 2 {
        return None;
    }
    let addr: IpAddr = parts[0].parse().ok()?;
    let prefix_len: u8 = parts[1].parse().ok()?;

    match addr {
        IpAddr::V4(_) if prefix_len > 32 => return None,
        IpAddr::V6(_) if prefix_len > 128 => return None,
        _ => {}
    }

    Some((addr, prefix_len))
}

/// Check if an IP matches a CIDR.
fn cidr_match(ip: IpAddr, network: IpAddr, prefix_len: u8) -> bool {
    match (ip, network) {
        (IpAddr::V4(ip), IpAddr::V4(net)) => {
            if prefix_len == 0 {
                return true;
            }
            let ip_bits = u32::from(ip);
            let net_bits = u32::from(net);
            let mask = u32::MAX.checked_shl(32 - prefix_len as u32).unwrap_or(0);
            (ip_bits & mask) == (net_bits & mask)
        }
        (IpAddr::V6(ip), IpAddr::V6(net)) => {
            if prefix_len == 0 {
                return true;
            }
            let ip_bits = u128::from(ip);
            let net_bits = u128::from(net);
            let mask = u128::MAX.checked_shl(128 - prefix_len as u32).unwrap_or(0);
            (ip_bits & mask) == (net_bits & mask)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_route_resolving_geoip_matches_domain_via_dns() {
        let make = || {
            let mut rules = RuleSet::new();
            rules.add_rule(MatchRule::GeoIp("PRIVATE".into()), RouteAction::Direct);
            rules.add_rule(MatchRule::MatchAll, RouteAction::Proxy);
            Router::new(rules).with_geoip(Arc::new(GeoIpDb::builtin()))
        };

        // Sync route: GeoIP cannot match a domain without resolution.
        // (Note: sync route() caches its decision; production connect paths
        // always use route_resolving, which performs the DNS lookup.)
        assert_eq!(make().route("localhost", 80), RouteAction::Proxy);

        // Resolving route: localhost -> 127.0.0.1 -> PRIVATE -> Direct.
        let router = make();
        assert_eq!(
            router.route_resolving("localhost", 80).await,
            RouteAction::Direct
        );
        // Second call is served from the route cache.
        assert_eq!(
            router.route_resolving("localhost", 80).await,
            RouteAction::Direct
        );
    }

    #[test]
    fn test_from_config_skips_disabled_rules() {
        let rules = vec![
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "example.com".into(),
                target: "direct".into(),
                enabled: false, // disabled — must not apply
            },
            RoutingRule {
                rule_type: "match".into(),
                pattern: "*".into(),
                target: "proxy".into(),
                enabled: true,
            },
        ];
        let router = Router::new(RuleSet::from_config(&rules));
        assert_eq!(router.route("api.example.com", 443), RouteAction::Proxy);
    }

    #[test]
    fn test_route_domain_exact() {
        let mut rules = RuleSet::new();
        rules.add_rule(MatchRule::Domain("example.com".into()), RouteAction::Direct);
        let router = Router::new(rules);

        assert_eq!(router.route("example.com", 443), RouteAction::Direct);
        assert_eq!(router.route("other.com", 443), RouteAction::Proxy);
    }

    #[test]
    fn test_route_domain_suffix() {
        let mut rules = RuleSet::new();
        rules.add_rule(
            MatchRule::DomainSuffix(".google.com".into()),
            RouteAction::Proxy,
        );
        rules.add_rule(MatchRule::DomainSuffix(".cn".into()), RouteAction::Direct);
        rules.set_default(RouteAction::Proxy);
        let router = Router::new(rules);

        assert_eq!(router.route("www.google.com", 443), RouteAction::Proxy);
        assert_eq!(router.route("google.com", 443), RouteAction::Proxy);
        assert_eq!(router.route("baidu.cn", 80), RouteAction::Direct);
        assert_eq!(router.route("example.org", 80), RouteAction::Proxy);
    }

    #[test]
    fn test_route_domain_keyword() {
        let mut rules = RuleSet::new();
        rules.add_rule(
            MatchRule::DomainKeyword("google".into()),
            RouteAction::Proxy,
        );
        let router = Router::new(rules);

        assert_eq!(router.route("www.google.com", 443), RouteAction::Proxy);
        assert_eq!(router.route("googleapis.com", 443), RouteAction::Proxy);
        assert_eq!(router.route("example.com", 443), RouteAction::Proxy); // default
    }

    #[test]
    fn test_route_ip_cidr_v4() {
        let mut rules = RuleSet::new();
        rules.add_rule(
            MatchRule::IpCidr {
                addr: "192.168.0.0".parse().unwrap(),
                prefix_len: 16,
            },
            RouteAction::Direct,
        );
        rules.add_rule(
            MatchRule::IpCidr {
                addr: "10.0.0.0".parse().unwrap(),
                prefix_len: 8,
            },
            RouteAction::Direct,
        );
        let router = Router::new(rules);

        assert_eq!(router.route("192.168.1.1", 80), RouteAction::Direct);
        assert_eq!(router.route("192.168.255.255", 80), RouteAction::Direct);
        assert_eq!(router.route("192.169.0.1", 80), RouteAction::Proxy);
        assert_eq!(router.route("10.1.2.3", 80), RouteAction::Direct);
        assert_eq!(router.route("11.0.0.1", 80), RouteAction::Proxy);
    }

    #[test]
    fn test_route_ip_cidr_v6() {
        let mut rules = RuleSet::new();
        rules.add_rule(
            MatchRule::IpCidr {
                addr: "::1".parse().unwrap(),
                prefix_len: 128,
            },
            RouteAction::Direct,
        );
        rules.add_rule(
            MatchRule::IpCidr {
                addr: "fd00::".parse().unwrap(),
                prefix_len: 8,
            },
            RouteAction::Direct,
        );
        let router = Router::new(rules);

        assert_eq!(router.route("::1", 80), RouteAction::Direct);
        assert_eq!(router.route("fd12::1", 80), RouteAction::Direct);
        assert_eq!(router.route("2001:db8::1", 80), RouteAction::Proxy);
    }

    #[test]
    fn test_route_match_all() {
        let mut rules = RuleSet::new();
        rules.add_rule(MatchRule::DomainSuffix(".cn".into()), RouteAction::Direct);
        rules.add_rule(MatchRule::MatchAll, RouteAction::Proxy);
        let router = Router::new(rules);

        assert_eq!(router.route("baidu.cn", 80), RouteAction::Direct);
        assert_eq!(router.route("anything.com", 443), RouteAction::Proxy);
    }

    #[test]
    fn test_route_reject() {
        let mut rules = RuleSet::new();
        rules.add_rule(MatchRule::DomainKeyword("ads".into()), RouteAction::Reject);
        let router = Router::new(rules);

        assert_eq!(router.route("ads.example.com", 80), RouteAction::Reject);
    }

    #[test]
    fn test_from_config() {
        let rules = vec![
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "cn".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "ip-cidr".into(),
                pattern: "192.168.0.0/16".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "domain-keyword".into(),
                pattern: "google".into(),
                target: "proxy".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "match".into(),
                pattern: "".into(),
                target: "proxy".into(),
                enabled: true,
            },
        ];

        let ruleset = RuleSet::from_config(&rules);
        assert_eq!(ruleset.rules().len(), 4);

        let router = Router::new(ruleset);
        assert_eq!(router.route("baidu.cn", 80), RouteAction::Direct);
        assert_eq!(router.route("192.168.1.1", 443), RouteAction::Direct);
        assert_eq!(router.route("www.google.com", 443), RouteAction::Proxy);
    }

    #[test]
    fn test_parse_cidr() {
        assert_eq!(
            parse_cidr("192.168.0.0/16"),
            Some(("192.168.0.0".parse().unwrap(), 16))
        );
        assert_eq!(parse_cidr("::1/128"), Some(("::1".parse().unwrap(), 128)));
        assert_eq!(parse_cidr("invalid"), None);
        assert_eq!(parse_cidr("192.168.0.0/33"), None);
    }

    #[test]
    fn test_cidr_match() {
        assert!(cidr_match(
            "192.168.1.1".parse().unwrap(),
            "192.168.0.0".parse().unwrap(),
            16
        ));
        assert!(!cidr_match(
            "192.169.0.1".parse().unwrap(),
            "192.168.0.0".parse().unwrap(),
            16
        ));
        assert!(cidr_match(
            "10.0.0.0".parse().unwrap(),
            "0.0.0.0".parse().unwrap(),
            0
        ));
    }

    #[test]
    fn test_case_insensitive() {
        let mut rules = RuleSet::new();
        rules.add_rule(
            MatchRule::Domain("Example.COM".to_lowercase()),
            RouteAction::Direct,
        );
        let router = Router::new(rules);

        assert_eq!(router.route("EXAMPLE.COM", 80), RouteAction::Direct);
        assert_eq!(router.route("example.com", 80), RouteAction::Direct);
    }

    #[test]
    fn test_route_cache_key_is_lowercased() {
        let mut rules = RuleSet::new();
        rules.add_rule(MatchRule::Domain("example.com".into()), RouteAction::Direct);
        let router = Router::new(rules);

        assert_eq!(router.route("EXAMPLE.COM", 80), RouteAction::Direct);

        // Cached under the lowercased host only — matching lowercases too.
        let cache = router.cache.read().unwrap();
        assert!(cache.peek("example.com").is_some());
        assert!(cache.peek("EXAMPLE.COM").is_none());
        assert_eq!(cache.len(), 1);
    }

    #[tokio::test]
    async fn test_route_and_route_resolving_share_lowercased_cache() {
        let mut rules = RuleSet::new();
        rules.add_rule(
            MatchRule::DomainSuffix(".example.com".into()),
            RouteAction::Direct,
        );
        let router = Router::new(rules);

        assert_eq!(router.route("WWW.EXAMPLE.COM", 443), RouteAction::Direct);
        assert_eq!(
            router.route_resolving("www.example.com", 443).await,
            RouteAction::Direct
        );
        assert_eq!(
            router.route_resolving("Www.Example.Com", 443).await,
            RouteAction::Direct
        );

        // All case variants map to a single cache entry.
        let cache = router.cache.read().unwrap();
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.peek("www.example.com"), Some(&RouteAction::Direct));
    }
}

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::num::NonZeroUsize;
use std::sync::{Arc, RwLock};

use lru::LruCache;

use super::geoip::{GeoIpDb, parse_cidr};
use crate::config::model::RoutingRule;

mod bindings;
use bindings::{DeviceBindings, RuleView};
mod trace;
pub use trace::{RouteTrace, RouteTraceStep};

/// Maximum number of cached route decisions.
const ROUTE_CACHE_CAP: usize = 4096;

/// Validate saved rules before publishing a new runtime. Invalid enabled
/// rules must not silently disappear or turn an unknown target into proxy.
pub fn validate_rule(rule: &RoutingRule) -> Result<(), &'static str> {
    if !rule.enabled {
        return Ok(());
    }
    let target = rule.target.as_str();
    if !matches!(
        target.to_ascii_lowercase().as_str(),
        "direct" | "proxy" | "reject" | "block"
    ) && !target
        .strip_prefix("group:")
        .is_some_and(crate::config::group::valid_group_name)
        && crate::config::group::node_target_id(target).is_none()
    {
        return Err("Invalid routing target");
    }
    match rule.rule_type.to_ascii_lowercase().as_str() {
        "domain" | "domain-suffix" | "domain-keyword" => {
            if rule.pattern.trim_matches('.').is_empty()
                || rule.pattern.chars().any(char::is_whitespace)
            {
                return Err("Invalid domain routing pattern");
            }
        }
        "ip-cidr" if parse_cidr(&rule.pattern).is_some() => {}
        "geoip"
            if rule.pattern.eq_ignore_ascii_case("PRIVATE")
                || (rule.pattern.len() == 2
                    && rule.pattern.bytes().all(|b| b.is_ascii_alphabetic())) => {}
        "dst-port" | "port" if parse_port_range(&rule.pattern).is_some_and(|(min, _)| min > 0) => {}
        "match" | "final" => {}
        _ => return Err("Unknown rule type or invalid routing pattern"),
    }
    normalize_rule_source(&rule.source).map_err(|_| "Invalid routing source")?;
    Ok(())
}

/// Action to take for a matched connection.
#[derive(Debug, Clone, PartialEq)]
pub enum RouteAction {
    /// Use the configured proxy outbound.
    Proxy,
    /// A named proxy group; never falls back to the global node.
    Group(String),
    /// One saved node, by its public id. A missing node fails closed.
    Node(String),
    /// Connect directly, bypassing proxy.
    Direct,
    /// Drop/reject the connection.
    Reject,
}

impl RouteAction {
    fn from_target(target: &str) -> Self {
        if let Some(name) = target.strip_prefix("group:") {
            RouteAction::Group(name.to_owned())
        } else if let Some(id) = target.strip_prefix("node:") {
            RouteAction::Node(id.to_ascii_lowercase())
        } else {
            match target.to_lowercase().as_str() {
                "direct" => RouteAction::Direct,
                "reject" | "block" => RouteAction::Reject,
                _ => RouteAction::Proxy,
            }
        }
    }
}

impl serde::Serialize for RouteAction {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Proxy => serializer.serialize_str("proxy"),
            Self::Direct => serializer.serialize_str("direct"),
            Self::Reject => serializer.serialize_str("reject"),
            Self::Group(name) => serializer.serialize_str(&format!("group:{name}")),
            Self::Node(id) => serializer.serialize_str(&format!("node:{id}")),
        }
    }
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
    /// Destination port or inclusive range (e.g. pattern `443` or `1000-2000`)
    DstPort { min: u16, max: u16 },
    /// Match all (catch-all rule)
    MatchAll,
    /// TUN DNS mapping already identified this destination as domestic.
    DomesticHint,
}

/// A single rule with its action.
#[derive(Debug, Clone)]
pub struct RuleEntry {
    /// Original saved-rule index; None for built-in rules.
    pub config_index: Option<usize>,
    pub source: Option<(IpAddr, u8)>,
    pub rule: MatchRule,
    pub action: RouteAction,
}

impl RuleEntry {
    fn compile(index: usize, r: &RoutingRule) -> Option<Self> {
        let action = RouteAction::from_target(&r.target);

        let rule = match r.rule_type.to_lowercase().as_str() {
            "ip-cidr" => match parse_cidr(&r.pattern) {
                Some((addr, prefix_len)) => MatchRule::IpCidr { addr, prefix_len },
                None => {
                    tracing::warn!("Invalid CIDR: {}", r.pattern);
                    return None;
                }
            },
            "geoip" => MatchRule::GeoIp(r.pattern.to_uppercase()),
            "dst-port" | "port" => match parse_port_range(&r.pattern) {
                Some((min, max)) => MatchRule::DstPort { min, max },
                None => {
                    tracing::warn!("Invalid dst-port pattern: {}", r.pattern);
                    return None;
                }
            },
            "match" | "final" => MatchRule::MatchAll,
            kind => MatchRule::compile_domain(kind, &r.pattern).or_else(|| {
                tracing::warn!("Unknown rule type: {}", r.rule_type);
                None
            })?,
        };

        let source = if r.source.is_empty() {
            None
        } else {
            let Ok(normalized) = normalize_rule_source(&r.source) else {
                return None;
            };
            parse_cidr(&normalized)
        };
        Some(Self {
            config_index: Some(index),
            source,
            rule,
            action,
        })
    }
}

/// A set of routing rules, compiled for efficient matching.
#[derive(Debug, Clone)]
pub struct RuleSet {
    rules: Vec<RuleEntry>,
    default_action: RouteAction,
    bindings: DeviceBindings,
    bindings_start: usize,
    expanded: std::sync::OnceLock<Vec<RuleEntry>>,
    device_scopes: Vec<(IpAddr, u8)>,
    paused_sources: crate::config::model::PausedDeviceSources,
    builtin_start: usize,
    global_final_start: Option<usize>,
}

impl RuleSet {
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            default_action: RouteAction::Proxy,
            bindings: Default::default(),
            bindings_start: 0,
            expanded: Default::default(),
            device_scopes: Vec::new(),
            paused_sources: Default::default(),
            builtin_start: 0,
            global_final_start: None,
        }
    }

    /// Set the default action when no rule matches.
    pub fn set_default(&mut self, action: RouteAction) {
        self.default_action = action;
    }

    /// Add a rule to the set.
    pub fn add_rule(&mut self, rule: MatchRule, action: RouteAction) {
        self.expanded.take();
        self.rules.push(RuleEntry {
            config_index: None,
            source: None,
            rule,
            action,
        });
    }

    /// Build from config RoutingRules. Disabled rules are skipped.
    ///
    /// Rules are ordered by **priority descending**, then original list
    /// index (stable). Higher `priority` wins when patterns overlap.
    pub fn from_config(rules: &[RoutingRule]) -> Self {
        Self::from_indexed_config(rules.iter().enumerate())
    }

    pub(crate) fn from_indexed_config<'a>(
        rules: impl IntoIterator<Item = (usize, &'a RoutingRule)>,
    ) -> Self {
        let indexed = rules.into_iter().filter(|(_, rule)| rule.enabled).collect();
        Self::from_enabled_rules(indexed)
    }

    fn from_enabled_rules(mut indexed: Vec<(usize, &RoutingRule)>) -> Self {
        let mut set = Self::new();
        indexed.sort_by_key(|(index, r)| (std::cmp::Reverse(r.priority), *index));
        set.rules = indexed
            .into_iter()
            .filter_map(|(index, r)| RuleEntry::compile(index, r))
            .collect();
        set
    }

    pub(crate) fn isolate_devices<'a>(
        &mut self,
        builtin_start: usize,
        sources: impl Iterator<Item = &'a str>,
    ) {
        self.builtin_start = builtin_start;
        self.global_final_start = Some(self.rules.len());
        self.device_scopes = sources
            .filter(|s| !s.is_empty())
            .filter_map(|s| parse_cidr(&normalize_rule_source(s).ok()?))
            .collect();
        self.device_scopes.sort_unstable();
        self.device_scopes.dedup();
    }

    pub fn with_paused_devices(
        mut self,
        paused: &crate::config::model::PausedDeviceSources,
    ) -> Self {
        self.paused_sources = paused.clone();
        self
    }

    fn is_paused(&self, source: Option<IpAddr>) -> bool {
        source.is_some_and(|ip| {
            self.paused_sources
                .0
                .iter()
                .any(|network| network.contains(&ip))
        })
    }

    fn has_device_scope(&self, source: Option<IpAddr>) -> bool {
        source.is_some()
            && self
                .device_scopes
                .iter()
                .any(|scope| source_matches(Some(*scope), source))
    }

    fn entries_for(
        &self,
        source: Option<IpAddr>,
        trace: bool,
    ) -> impl Iterator<Item = RuleView<'_>> {
        let paused = self.is_paused(source);
        let assigned = !paused && self.has_device_scope(source);
        let globals_start = if assigned {
            self.builtin_start
        } else {
            self.bindings_start
        };
        let end = if assigned {
            self.global_final_start.unwrap_or(self.rules.len())
        } else {
            self.rules.len()
        };
        self.rules[..self.bindings_start]
            .iter()
            .map(RuleView::from)
            .chain(self.bindings.entries(source, trace, paused))
            .chain(self.rules[globals_start..end].iter().map(RuleView::from))
            .filter(move |entry| !paused || entry.source.is_none())
    }

    pub(crate) fn bind_devices(
        &mut self,
        rules: &[(usize, &RoutingRule)],
        links: &[crate::config::model::DeviceRuleGroup],
    ) {
        self.bindings_start = self.rules.len();
        self.bindings = DeviceBindings::compile(rules, links);
    }

    pub(crate) fn extend(&mut self, other: Self) {
        self.expanded.take();
        self.rules.extend(other.rules);
    }

    pub(crate) fn base_len(&self) -> usize {
        self.rules.len()
    }

    /// Inspect the complete ruleset including every binding in evaluation order.
    /// The routing and trace paths read shared group members directly.
    pub fn rules(&self) -> &[RuleEntry] {
        if self.bindings.is_empty() {
            return &self.rules;
        }
        self.expanded.get_or_init(|| {
            let mut entries = self.rules[..self.bindings_start].to_vec();
            entries.extend(
                self.bindings
                    .entries(None, true, false)
                    .map(|view| RuleEntry {
                        config_index: view.config_index,
                        source: view.source,
                        rule: view.rule.clone(),
                        action: view.action.clone(),
                    }),
            );
            entries.extend_from_slice(&self.rules[self.bindings_start..]);
            entries
        })
    }
}

impl Default for RuleSet {
    fn default() -> Self {
        Self::new()
    }
}

/// Keep addresses and ports in their native representation: cache hits should
/// not format IPs or allocate intermediate host/port/context strings.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RouteCacheKey {
    host: String,
    port: u16,
    source: Option<IpAddr>,
    destination: (Option<IpAddr>, bool),
}

fn client_cache_key(host: &str, port: u16, source: Option<IpAddr>) -> RouteCacheKey {
    RouteCacheKey {
        host: host.to_lowercase(),
        port,
        source,
        destination: crate::proxy::routing::tun_destination(),
    }
}

/// IP-based Direct decisions carry only addresses that matched the winning rule.
/// Other decisions leave DNS selection to the chosen outbound.
#[derive(Debug, Clone)]
pub(crate) struct RouteDecision {
    pub(crate) action: RouteAction,
    pub(crate) direct_ips: Vec<IpAddr>,
}

impl RouteDecision {
    fn unbound(action: &RouteAction) -> Self {
        Self {
            action: action.clone(),
            direct_ips: Vec::new(),
        }
    }
}

impl MatchRule {
    pub(crate) fn compile_domain(kind: &str, pattern: &str) -> Option<Self> {
        match kind {
            "domain" => Some(Self::Domain(pattern.to_lowercase())),
            "domain-suffix" => {
                let suffix = pattern.to_lowercase();
                Some(Self::DomainSuffix(if suffix.starts_with('.') {
                    suffix
                } else {
                    format!(".{suffix}")
                }))
            }
            "domain-keyword" => Some(Self::DomainKeyword(pattern.to_lowercase())),
            _ => None,
        }
    }

    pub(crate) fn matches_domain(&self, host_lower: &str) -> bool {
        match self {
            Self::Domain(domain) => host_lower == domain,
            Self::DomainSuffix(suffix) => {
                host_lower.ends_with(suffix.as_str())
                    || host_lower == suffix.trim_start_matches('.')
            }
            Self::DomainKeyword(keyword) => host_lower.contains(keyword.as_str()),
            _ => false,
        }
    }

    fn needs_ip(&self) -> bool {
        matches!(self, Self::IpCidr { .. } | Self::GeoIp(_))
    }
}

/// Router evaluates rules against connection targets.
pub struct Router {
    rules: RuleSet,
    pub(crate) route_domestic_hints: bool,
    geoip: Option<Arc<GeoIpDb>>,
    device_identities: Option<Arc<RwLock<BTreeMap<Ipv6Addr, Ipv4Addr>>>>,
    /// LRU route cache: evicts least-recently-used entries when full (avoids thundering-herd on full flush).
    /// Keyed by lowercased host, port, and the complete client/TUN context.
    cache: RwLock<LruCache<RouteCacheKey, RouteDecision>>,
}

impl Router {
    pub fn new(rules: RuleSet) -> Self {
        let route_domestic_hints = rules.rules.iter().any(|entry| {
            matches!(entry.rule, MatchRule::DomesticHint) && entry.action != RouteAction::Direct
        });
        Self {
            route_domestic_hints,
            rules,
            geoip: None,
            device_identities: None,
            cache: RwLock::new(LruCache::new(NonZeroUsize::new(ROUTE_CACHE_CAP).unwrap())),
        }
    }

    pub fn with_geoip(mut self, geoip: Arc<GeoIpDb>) -> Self {
        self.geoip = Some(geoip);
        self
    }

    /// Share the live LAN IPv6-to-IPv4 identity mapping across routing snapshots.
    pub fn with_device_identities(
        mut self,
        identities: Arc<RwLock<BTreeMap<Ipv6Addr, Ipv4Addr>>>,
    ) -> Self {
        self.device_identities = Some(identities);
        self
    }

    fn has_source_policy(&self, source: Option<IpAddr>) -> bool {
        self.rules.has_device_scope(source)
            || (self.rules.global_final_start.is_none()
                && self
                    .rules
                    .rules
                    .iter()
                    .any(|entry| entry.source.is_some() && source_matches(entry.source, source)))
    }

    fn effective_source(&self, source: Option<IpAddr>) -> Option<IpAddr> {
        let (Some(IpAddr::V6(v6)), Some(identities)) = (source, &self.device_identities) else {
            return source;
        };
        if self.rules.is_paused(source) || self.has_source_policy(source) {
            return source;
        }
        let v4 = identities
            .read()
            .ok()
            .and_then(|map| map.get(&v6).copied())
            .map(IpAddr::V4);
        if v4.is_some() && (self.rules.is_paused(v4) || self.has_source_policy(v4)) {
            v4
        } else {
            source
        }
    }

    fn client_source(&self) -> Option<IpAddr> {
        self.effective_source(crate::proxy::routing::client_source())
    }

    /// Whether the current client has any enabled device-specific policy.
    pub fn has_client_rules(&self) -> bool {
        let source = self.client_source();
        !self.rules.is_paused(source) && self.has_source_policy(source)
    }

    /// Evaluate routing rules for a given destination.
    /// `host` can be a domain name or IP address string.
    pub fn route(&self, host: &str, port: u16) -> RouteAction {
        self.cached_decision(host, port, self.client_source())
            .action
    }

    fn cached_decision(&self, host: &str, port: u16, source: Option<IpAddr>) -> RouteDecision {
        let key = client_cache_key(host, port, source);
        if let Ok(mut cache) = self.cache.write()
            && let Some(decision) = cache.get(&key)
        {
            return decision.clone();
        }

        let ip = host.parse::<IpAddr>().ok().or(key.destination.0);
        let decision = self
            .rules
            .entries_for(source, false)
            .filter(|entry| source_matches(entry.source, source))
            .find_map(|entry| self.match_decision(entry, &key.host, ip.as_slice(), port))
            .unwrap_or_else(|| RouteDecision::unbound(&self.rules.default_action));
        if let Ok(mut cache) = self.cache.write() {
            cache.put(key, decision.clone());
        }
        decision
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
        let source = self.client_source();
        for entry in self.rules.entries_for(source, false) {
            if !source_matches(entry.source, source) {
                continue;
            }
            if entry.rule.matches_domain(&host_lower) {
                return Some(entry.action.clone());
            }
        }
        None
    }

    /// Route like [`route`](Self::route), but resolves domain hosts via the
    /// request-scoped DNS (or system DNS) when reaching an IP-based rule.
    ///
    /// Rule order is preserved: domain rules are tried first, and a DNS
    /// lookup happens only if evaluation reaches an IP-based rule without a
    /// prior match — at most once per call. Domain decisions are re-evaluated
    /// against the TTL-aware DNS cache so address changes and transient DNS
    /// failures cannot pin a host to an obsolete route.
    pub async fn route_resolving(&self, host: &str, port: u16) -> RouteAction {
        self.decision_resolving(host, port).await.action
    }

    pub(crate) async fn decision_resolving(&self, host: &str, port: u16) -> RouteDecision {
        let source = self.client_source();
        // A concrete destination is authoritative, including when its hostname
        // was recovered by the TUN. Its decision can safely outlive a DNS TTL.
        if host.parse::<IpAddr>().is_ok() || crate::proxy::routing::tun_destination().0.is_some() {
            return self.cached_decision(host, port, source);
        }
        let lower = host.to_lowercase();
        let mut answers = None;
        for entry in self.rules.entries_for(source, false) {
            if !source_matches(entry.source, source) {
                continue;
            }
            if entry.rule.needs_ip() && answers.is_none() {
                answers = Some(resolve_host(&lower).await);
            }
            if let Some(decision) =
                self.match_decision(entry, &lower, answers.as_deref().unwrap_or_default(), port)
            {
                return decision;
            }
        }
        RouteDecision::unbound(&self.rules.default_action)
    }

    fn match_decision(
        &self,
        entry: RuleView<'_>,
        host: &str,
        ips: &[IpAddr],
        port: u16,
    ) -> Option<RouteDecision> {
        let mut direct_ips = Vec::new();
        if entry.rule.needs_ip() {
            let mut matches = ips
                .iter()
                .copied()
                .filter(|ip| self.rule_matches(entry.rule, host, Some(*ip), port));
            let first = matches.next()?;
            if entry.action == &RouteAction::Direct && host.parse::<IpAddr>().is_err() {
                direct_ips.push(first);
                direct_ips.extend(matches);
            }
        } else if !self.rule_matches(entry.rule, host, None, port) {
            return None;
        }
        tracing::debug!(%host, port, action = ?entry.action, rule = ?entry.rule, "Route match");
        Some(RouteDecision {
            action: entry.action.clone(),
            direct_ips,
        })
    }

    fn rule_matches(
        &self,
        rule: &MatchRule,
        host_lower: &str,
        ip: Option<IpAddr>,
        port: u16,
    ) -> bool {
        match rule {
            MatchRule::Domain(_) | MatchRule::DomainSuffix(_) | MatchRule::DomainKeyword(_) => {
                rule.matches_domain(host_lower)
            }

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

            MatchRule::DstPort { min, max } => port >= *min && port <= *max,

            MatchRule::MatchAll => true,
            MatchRule::DomesticHint => crate::proxy::routing::tun_destination().1,
        }
    }
}

/// Classification uses real, family-eligible answers. Resolver order controls
/// the dial preference within a matching rule, never rule precedence.
async fn resolve_host(host: &str) -> Vec<IpAddr> {
    use crate::proxy::routing::{TunFamily, tun_family};
    let family = tun_family();
    let eligible = |ip: IpAddr| {
        crate::dns::fakeip::fake_ipv4(ip).is_none()
            && family.is_none_or(|family| family.matches(ip))
    };
    if let Some(result) = crate::dns::scoped_resolve(host).await {
        let mut ips = result.unwrap_or_default();
        ips.retain(|ip| eligible(*ip));
        return ips;
    }
    let lookup = tokio::net::lookup_host((host, 0));
    let Ok(Ok(addrs)) = tokio::time::timeout(std::time::Duration::from_secs(3), lookup).await
    else {
        return Vec::new();
    };
    let mut ips = Vec::new();
    let mut saw_fake = false;
    for addr in addrs {
        let ip = addr.ip();
        saw_fake |= crate::dns::fakeip::fake_ipv4(ip).is_some();
        if eligible(ip) && !ips.contains(&ip) {
            ips.push(ip);
        }
    }
    // Merlin can redirect OS lookups back to the Fake-IP listener. Recover
    // real records for the original family (both for ordinary proxy requests).
    if ips.is_empty() && saw_fake {
        const DOMESTIC_NS: &[&str] = &["223.5.5.5:53", "119.29.29.29:53"];
        for server in DOMESTIC_NS.iter().filter_map(|ns| ns.parse().ok()) {
            let lookup = |qtype| crate::dns::lookup_ips_udp(server, host, qtype);
            let mut real = match family {
                Some(TunFamily::V4) => lookup(1).await.unwrap_or_default(),
                Some(TunFamily::V6) => lookup(28).await.unwrap_or_default(),
                None => {
                    let (v4, v6) = tokio::join!(lookup(1), lookup(28));
                    let mut v4 = v4.unwrap_or_default();
                    v4.extend(v6.unwrap_or_default());
                    v4
                }
            };
            real.retain(|ip| eligible(*ip));
            if !real.is_empty() {
                return real;
            }
        }
    }
    ips
}

/// Parse a CIDR string like "192.168.0.0/16" or "::1/128".
fn parse_port_range(pattern: &str) -> Option<(u16, u16)> {
    let p = pattern.trim();
    if p.is_empty() {
        return None;
    }
    if let Some((a, b)) = p.split_once('-') {
        let min: u16 = a.trim().parse().ok()?;
        let max: u16 = b.trim().parse().ok()?;
        if min <= max {
            Some((min, max))
        } else {
            Some((max, min))
        }
    } else {
        let port: u16 = p.parse().ok()?;
        Some((port, port))
    }
}

/// Canonicalize a client address/network so equivalent scopes share identity.
pub fn normalize_rule_source(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(String::new());
    }
    let (ip, bits) = if value.contains('/') {
        parse_cidr(value)
            .ok_or_else(|| "来源必须是有效的 IPv4/IPv6 地址或 CIDR 网段".to_string())?
    } else {
        let ip: IpAddr = value
            .parse()
            .map_err(|_| "来源必须是 IP 地址或 CIDR 网段，不能填写 MAC / 主机名".to_string())?;
        (ip, if ip.is_ipv4() { 32 } else { 128 })
    };
    let (ip, bits) = match ip {
        IpAddr::V6(v6) if v6.to_ipv4_mapped().is_some() => {
            if bits < 96 {
                return Err("IPv4 映射地址的前缀必须至少为 96，或直接填写 IPv4 网段".into());
            }
            (IpAddr::V4(v6.to_ipv4_mapped().unwrap()), bits - 96)
        }
        _ => (ip, bits),
    };
    let network = match ip {
        IpAddr::V4(ip) => IpAddr::V4(
            (u32::from(ip)
                & if bits == 0 {
                    0
                } else {
                    u32::MAX << (32 - bits)
                })
            .into(),
        ),
        IpAddr::V6(ip) => IpAddr::V6(
            (u128::from(ip)
                & if bits == 0 {
                    0
                } else {
                    u128::MAX << (128 - bits)
                })
            .into(),
        ),
    };
    Ok(format!("{network}/{bits}"))
}
fn source_matches(scope: Option<(IpAddr, u8)>, source: Option<IpAddr>) -> bool {
    match (scope, source) {
        (None, _) => true,
        (Some((network, bits)), Some(ip)) => cidr_match(ip, network, bits),
        _ => false,
    }
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
    async fn resolved_candidates_follow_rule_precedence_and_pin_only_matching_direct_addresses() {
        use crate::dns::{DnsConfig, DnsResolver};
        use crate::proxy::routing::with_tun_destination;

        let host = "mixed.example";
        let subnet = |network: &str| {
            let (addr, prefix_len) = parse_cidr(network).unwrap();
            MatchRule::IpCidr { addr, prefix_len }
        };
        let resolver_for = |ips: &[&str]| {
            let mut config = DnsConfig::default();
            config.hosts.insert(
                host.into(),
                ips.iter().map(|ip| ip.parse().unwrap()).collect(),
            );
            DnsResolver::with_config(config)
        };
        let mut rules = RuleSet::new();
        rules.add_rule(
            MatchRule::Domain("explicit.example".into()),
            RouteAction::Direct,
        );
        rules.add_rule(subnet("203.0.113.0/24"), RouteAction::Reject);
        rules.add_rule(subnet("192.0.2.0/24"), RouteAction::Direct);
        let router = Router::new(rules);
        let preferred: IpAddr = "192.0.2.3".parse().unwrap();
        let other: IpAddr = "192.0.2.4".parse().unwrap();
        let reject: IpAddr = "203.0.113.7".parse().unwrap();
        for addresses in [["192.0.2.3", "203.0.113.7"], ["203.0.113.7", "192.0.2.3"]] {
            resolver_for(&addresses)
                .scope(async {
                    assert_eq!(router.route_resolving(host, 443).await, RouteAction::Reject);
                    let trace = router.trace_route(host, 443, None, None, true).await;
                    assert_eq!(trace.action, RouteAction::Reject);
                    assert_eq!(trace.resolved_ip, Some(reject));
                })
                .await;
        }
        resolver_for(&["198.18.0.2", "198.51.100.10", "192.0.2.3", "192.0.2.4"])
            .scope(async {
                let decision = router.decision_resolving(host, 443).await;
                assert_eq!(decision.action, RouteAction::Direct);
                assert_eq!(decision.direct_ips, [preferred, other]);
                let trace = router.trace_route(host, 443, None, None, true).await;
                assert_eq!(trace.resolved_ip, Some(preferred));

                with_tun_destination(Some(other), false, async {
                    assert_eq!(router.route(host, 443), RouteAction::Direct);
                    assert_eq!(
                        router.decision_resolving(host, 443).await.direct_ips,
                        [other]
                    );
                    let explicit = router.decision_resolving("explicit.example", 443).await;
                    assert_eq!(explicit.action, RouteAction::Direct);
                    assert!(explicit.direct_ips.is_empty());
                })
                .await;
            })
            .await;
        // The earlier named decisions must not survive a changed DNS answer.
        resolver_for(&["198.51.100.10"])
            .scope(async {
                assert_eq!(router.route_resolving(host, 443).await, RouteAction::Proxy);
            })
            .await;
    }

    #[tokio::test]
    async fn cache_preserves_client_destination_hint_and_port_isolation() {
        use crate::proxy::routing::{with_client_source, with_tun_destination};
        let mut rules = RuleSet::new();
        rules.add_rule(MatchRule::DomesticHint, RouteAction::Direct);
        rules.add_rule(
            MatchRule::IpCidr {
                addr: "192.0.2.0".parse().unwrap(),
                prefix_len: 24,
            },
            RouteAction::Reject,
        );
        rules.add_rule(MatchRule::DstPort { min: 80, max: 80 }, RouteAction::Direct);
        let router = Router::new(rules);
        for source in [
            None,
            Some("192.168.1.2".parse().unwrap()),
            Some("2001:db8::2".parse().unwrap()),
        ] {
            with_client_source(source, async {
                for destination in [
                    None,
                    Some("192.0.2.1".parse().unwrap()),
                    Some("2001:db8::1".parse().unwrap()),
                ] {
                    for domestic in [false, true] {
                        with_tun_destination(destination, domestic, async {
                            for port in [80, 443] {
                                let expected = if domestic {
                                    RouteAction::Direct
                                } else if destination == Some("192.0.2.1".parse().unwrap()) {
                                    RouteAction::Reject
                                } else if port == 80 {
                                    RouteAction::Direct
                                } else {
                                    RouteAction::Proxy
                                };
                                assert_eq!(router.route("Example.com", port), expected);
                                assert_eq!(router.route("EXAMPLE.COM", port), expected);
                                if destination.is_some() {
                                    assert_eq!(
                                        router.route_resolving("example.com", port).await,
                                        expected
                                    );
                                }
                            }
                        })
                        .await;
                    }
                }
            })
            .await;
        }
        assert_eq!(router.cache.read().unwrap().len(), 36);
    }

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
        // Repeated domain decisions still evaluate against current DNS.
        assert_eq!(
            router.route_resolving("localhost", 80).await,
            RouteAction::Direct
        );
    }

    #[test]
    fn test_from_config_skips_disabled_rules() {
        let rules = vec![
            RoutingRule {
                source: String::new(),
                name: String::new(),
                group: String::new(),
                rule_type: "domain-suffix".into(),
                pattern: "example.com".into(),
                target: "direct".into(),
                enabled: false,
                priority: 0, // disabled — must not apply
            },
            RoutingRule {
                source: String::new(),
                name: String::new(),
                group: String::new(),
                rule_type: "match".into(),
                pattern: "*".into(),
                target: "proxy".into(),
                enabled: true,
                priority: 0,
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
                source: String::new(),
                name: String::new(),
                group: String::new(),
                rule_type: "domain-suffix".into(),
                pattern: "cn".into(),
                target: "direct".into(),
                enabled: true,
                priority: 0,
            },
            RoutingRule {
                source: String::new(),
                name: String::new(),
                group: String::new(),
                rule_type: "ip-cidr".into(),
                pattern: "192.168.0.0/16".into(),
                target: "direct".into(),
                enabled: true,
                priority: 0,
            },
            RoutingRule {
                source: String::new(),
                name: String::new(),
                group: String::new(),
                rule_type: "domain-keyword".into(),
                pattern: "google".into(),
                target: "proxy".into(),
                enabled: true,
                priority: 0,
            },
            RoutingRule {
                source: String::new(),
                name: String::new(),
                group: String::new(),
                rule_type: "match".into(),
                pattern: "".into(),
                target: "proxy".into(),
                enabled: true,
                priority: 0,
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

        // Cached under lowercased host:port.
        let cache = router.cache.read().unwrap();
        assert!(
            cache
                .peek(&client_cache_key("example.com", 80, None))
                .is_some()
        );
        assert_eq!(cache.iter().next().unwrap().0.host, "example.com");
        assert_eq!(cache.len(), 1);
    }

    #[tokio::test]
    async fn test_cached_sync_route_agrees_with_resolving_domain_rule() {
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

        // Resolving domain routes leave the synchronous cache unchanged.
        let cache = router.cache.read().unwrap();
        assert_eq!(cache.len(), 1);
        assert_eq!(
            cache
                .peek(&client_cache_key("www.example.com", 443, None))
                .map(|decision| &decision.action),
            Some(&RouteAction::Direct)
        );
    }

    #[test]
    fn test_priority_orders_before_list_index() {
        let rules = vec![
            RoutingRule {
                source: String::new(),
                name: String::new(),
                group: String::new(),
                rule_type: "domain-suffix".into(),
                pattern: "example.com".into(),
                target: "direct".into(),
                enabled: true,
                priority: 1,
            },
            RoutingRule {
                source: String::new(),
                name: String::new(),
                group: String::new(),
                rule_type: "domain-suffix".into(),
                pattern: "example.com".into(),
                target: "reject".into(),
                enabled: true,
                priority: 100,
            },
        ];
        let router = Router::new(RuleSet::from_config(&rules));
        assert_eq!(router.route("a.example.com", 443), RouteAction::Reject);
    }

    #[test]
    fn test_dst_port_rule() {
        let rules = vec![
            RoutingRule {
                source: String::new(),
                name: String::new(),
                group: String::new(),
                rule_type: "dst-port".into(),
                pattern: "443".into(),
                target: "direct".into(),
                enabled: true,
                priority: 0,
            },
            RoutingRule {
                source: String::new(),
                name: String::new(),
                group: String::new(),
                rule_type: "dst-port".into(),
                pattern: "1000-2000".into(),
                target: "reject".into(),
                enabled: true,
                priority: 0,
            },
            RoutingRule {
                source: String::new(),
                name: String::new(),
                group: String::new(),
                rule_type: "match".into(),
                pattern: "*".into(),
                target: "proxy".into(),
                enabled: true,
                priority: 0,
            },
        ];
        let router = Router::new(RuleSet::from_config(&rules));
        assert_eq!(router.route("example.com", 443), RouteAction::Direct);
        assert_eq!(router.route("example.com", 1500), RouteAction::Reject);
        assert_eq!(router.route("example.com", 80), RouteAction::Proxy);
    }
}

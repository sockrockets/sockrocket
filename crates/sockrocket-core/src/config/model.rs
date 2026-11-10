use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Proxy routing mode
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyMode {
    /// All traffic goes through the proxy node
    Global,
    /// User rules + built-in China-direct, then final
    #[default]
    Rule,
    /// All traffic connects directly (no proxy)
    Direct,
}

/// Decode a percent-encoded display name.
///
/// Iteratively URL-decodes up to 3 times to handle multi-level encoding
/// commonly seen in subscription share links. Also replaces `+` with space
/// and trims surrounding whitespace.
pub fn decode_display_name(value: &str) -> String {
    let mut decoded = value.replace('+', " ");
    for _ in 0..3 {
        match urlencoding::decode(&decoded) {
            Ok(next) if next.as_ref() != decoded => decoded = next.into_owned(),
            _ => break,
        }
    }
    decoded.trim().to_string()
}

/// Decode and normalize the names of all nodes in-place.
/// Returns `true` if any name was changed.
pub fn normalize_node_names(nodes: &mut [Node]) -> bool {
    let mut changed = false;
    for node in nodes {
        let normalized = decode_display_name(&node.name);
        if normalized != node.name {
            node.name = normalized;
            changed = true;
        }
    }
    changed
}

/// Unified application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Local proxy listen address
    pub listen_addr: String,
    /// Local SOCKS5 proxy port
    pub socks_port: u16,
    /// Local HTTP proxy port
    pub http_port: u16,
    /// Shared DNS policy. None preserves legacy router keys and defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns: Option<super::dns::DnsSettings>,
    #[serde(default)]
    pub ipv6_mode: super::dns::Ipv6Mode,
    /// Proxy routing mode
    #[serde(default)]
    pub proxy_mode: ProxyMode,
    /// Proxy nodes
    pub nodes: Vec<Node>,
    /// Active node index
    pub active_node: Option<usize>,
    /// Subscriptions
    pub subscriptions: Vec<Subscription>,
    /// Routing rules
    pub rules: Vec<RoutingRule>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub device_rule_groups: Vec<DeviceRuleGroup>,
    #[serde(default)]
    pub explicit_group_bindings: bool,
    /// Whether the proxy was running when the app was last closed (auto-reconnect on next start)
    #[serde(default)]
    pub auto_connect: bool,
    /// Whether TUN mode was active when the app was last closed
    #[serde(default)]
    pub tun_was_enabled: bool,
    /// None selects MTU at TUN startup; a value overrides discovery (1280..=9000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tun_mtu: Option<u16>,
    /// Node health check + auto-switch settings
    #[serde(default)]
    pub health_check: HealthCheckConfig,
    /// User-defined proxy groups used by group:name routing targets
    #[serde(default)]
    pub groups: Vec<ProxyGroupConfig>,
    /// UI locale id (`en`, later `zh-CN` / `vi` / `hi`). Unknown values are
    /// interpreted as English by the GUI.
    #[serde(default = "default_locale")]
    pub locale: String,
}

fn default_locale() -> String {
    "en".to_string()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            listen_addr: "127.0.0.1".to_string(),
            socks_port: 1080,
            http_port: 1087,
            dns: None,
            ipv6_mode: super::dns::Ipv6Mode::default(),
            proxy_mode: ProxyMode::default(),
            nodes: Vec::new(),
            active_node: None,
            subscriptions: Vec::new(),
            rules: Vec::new(),
            device_rule_groups: Vec::new(),
            explicit_group_bindings: false,
            auto_connect: false,
            tun_was_enabled: false,
            tun_mtu: None,
            health_check: HealthCheckConfig::default(),
            groups: Vec::new(),
            locale: default_locale(),
        }
    }
}

impl AppConfig {
    /// Parse YAML config. Missing `proxy_mode` defaults to [`ProxyMode::Rule`]
    /// (Merlin installs historically omitted the key).
    pub fn from_yaml_str(content: &str) -> Result<Self, serde_yaml::Error> {
        let raw: serde_yaml::Value = serde_yaml::from_str(content)?;
        Self::from_yaml_value(raw)
    }

    pub(super) fn from_yaml_value(mut raw: serde_yaml::Value) -> Result<Self, serde_yaml::Error> {
        super::node_identity::migrate_references(&mut raw)?;
        let legacy_mode = raw.get("proxy_mode").is_none();
        let mut cfg: Self = serde_yaml::from_value(raw)?;
        if legacy_mode {
            cfg.proxy_mode = ProxyMode::Rule;
        }
        super::group::validate_targets(&cfg.groups, &cfg.rules)
            .map_err(<serde_yaml::Error as serde::de::Error>::custom)?;
        validate_device_rule_groups(&cfg.device_rule_groups, &cfg.rules, &cfg.groups)
            .map_err(<serde_yaml::Error as serde::de::Error>::custom)?;
        Ok(cfg)
    }
}

/// Node health check configuration.
///
/// When enabled, the proxy service periodically probes the active node and,
/// after `failure_threshold` consecutive failures, optionally switches to the
/// lowest-latency reachable node. Disabled by default (conservative).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckConfig {
    /// Master switch for the whole health check mechanism (default: off)
    #[serde(default)]
    pub enabled: bool,
    /// Probe interval in seconds (default: 60, clamped to >= 5 at runtime)
    #[serde(default = "default_health_interval")]
    pub interval_secs: u64,
    /// Consecutive probe failures before the node is considered dead (default: 2)
    #[serde(default = "default_health_failure_threshold")]
    pub failure_threshold: u32,
    /// Automatically switch to the best reachable node on failure (default: true)
    #[serde(default = "default_health_auto_switch")]
    pub auto_switch: bool,
}

fn default_health_interval() -> u64 {
    60
}

fn default_health_failure_threshold() -> u32 {
    2
}

fn default_health_auto_switch() -> bool {
    true
}

impl Default for HealthCheckConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: default_health_interval(),
            failure_threshold: default_health_failure_threshold(),
            auto_switch: default_health_auto_switch(),
        }
    }
}

/// Proxy group behavior type
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GroupType {
    /// Manual selection: the user picks the active member
    #[default]
    Select,
    /// Auto-pick the lowest-latency member after a latency test
    UrlTest,
    /// Ordered: the first reachable member wins
    Fallback,
}

/// A user-defined proxy group.
///
/// Members are keyed by node fingerprint (see `dedup::node_fingerprint`) so
/// they survive renames and subscription refreshes; display names are only
/// cosmetic. Fingerprints that no longer resolve to a node are skipped by
/// the group helpers (see `config::group`) rather than treated as errors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyGroupConfig {
    /// Display name
    pub name: String,
    /// Group behavior type
    #[serde(rename = "type")]
    pub gtype: GroupType,
    /// Member node fingerprints, in order
    #[serde(default)]
    pub members: Vec<String>,
    /// Fingerprint of the current pick (`url-test`/`fallback` update it after a test)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
}

/// A proxy node (server)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    /// Display name
    pub name: String,
    /// Server address (hostname or IP)
    pub server: String,
    /// Server port
    pub port: u16,
    /// Proxy protocol
    pub protocol: ProxyProtocol,
    /// Transport layer config
    pub transport: Option<TransportConfig>,
    /// Measured latency in ms (None if not tested)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u32>,
    /// User-defined tags for grouping/filtering
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Extra metadata
    #[serde(default)]
    pub extra: HashMap<String, String>,
}

/// shadow-tls plugin (v3) settings for Shadowsocks nodes.
///
/// The shadowsocks payload is tunneled through a shadow-tls v3 server that
/// camouflages as a TLS 1.3 endpoint for `host`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShadowTlsConfig {
    /// Camouflage SNI handed to the shadow-tls server.
    pub host: String,
    /// Shared shadow-tls password (plugin-opts.password).
    pub password: String,
    /// Accept any certificate from the shadow-tls server (plugin-opts
    /// `skip-cert-verify`; the camouflage SNI can never be legitimately
    /// certified by the node, so subscriptions set this).
    #[serde(default)]
    pub skip_cert_verify: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TuicRelay {
    #[default]
    Native,
    Quic,
    UotV1,
    UotV2,
}
impl TuicRelay {
    pub(crate) fn parse(mode: Option<&str>, uot: Option<u8>) -> anyhow::Result<Self> {
        match (mode, uot) {
            (None | Some(""), Some(1)) => Ok(Self::UotV1),
            (None | Some(""), Some(2)) => Ok(Self::UotV2),
            (None | Some("" | "native"), None) => Ok(Self::Native),
            (Some("quic"), None) => Ok(Self::Quic),
            _ => anyhow::bail!("Invalid or conflicting TUIC UDP relay options"),
        }
    }
    pub(crate) fn is_native(&self) -> bool {
        *self == Self::Native
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnyTlsPool {
    pub check_interval_ms: Option<u64>,
    pub idle_timeout_ms: Option<u64>,
    #[serde(default)]
    pub min_idle: usize,
    #[serde(default)]
    pub disable_reuse: bool,
}
impl AnyTlsPool {
    pub(crate) fn checked(self) -> anyhow::Result<Self> {
        self.validate()?;
        Ok(self)
    }
    fn is_default(&self) -> bool {
        self == &Self::default()
    }
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.min_idle <= 64,
            "AnyTLS minimum idle sessions exceeds per-node session capacity"
        );
        anyhow::ensure!(
            [self.check_interval_ms, self.idle_timeout_ms]
                .into_iter()
                .flatten()
                .all(|v| v <= (i64::MAX as u64).div_ceil(1_000_000)),
            "AnyTLS duration exceeds range"
        );
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hy2Obfs {
    #[serde(rename = "type")]
    pub kind: String,
    pub password: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_packet_size: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_packet_size: Option<u16>,
}
impl std::fmt::Debug for Hy2Obfs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hy2Obfs")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}
impl Hy2Obfs {
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.password.len() >= 4,
            "Hysteria2 obfuscation needs at least four password bytes"
        );
        let (min, max) = self.gecko_sizes();
        match self.kind.as_str() {
            "salamander" => anyhow::ensure!(
                self.min_packet_size.is_none() && self.max_packet_size.is_none(),
                "Gecko sizes require Gecko obfuscation"
            ),
            "gecko" => {
                anyhow::ensure!(min <= max && max <= 2048, "Invalid Gecko packet size range")
            }
            _ => anyhow::bail!("Unsupported Hysteria2 obfuscation type"),
        }
        Ok(())
    }
    pub(crate) fn gecko_sizes(&self) -> (usize, usize) {
        (
            self.min_packet_size.filter(|v| *v > 0).unwrap_or(512) as usize,
            self.max_packet_size.filter(|v| *v > 0).unwrap_or(1200) as usize,
        )
    }
    pub(crate) fn from_parts(
        kind: Option<&str>,
        password: Option<&str>,
        min_packet_size: Option<u16>,
        max_packet_size: Option<u16>,
    ) -> anyhow::Result<Option<Self>> {
        if kind.is_none()
            && password.is_none()
            && min_packet_size.is_none()
            && max_packet_size.is_none()
        {
            return Ok(None);
        }
        let result = Self {
            kind: kind.unwrap_or("").into(),
            password: password.unwrap_or("").into(),
            min_packet_size,
            max_packet_size,
        };
        result.validate()?;
        Ok(Some(result))
    }
}

/// Supported proxy protocols
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ProxyProtocol {
    /// An in-process, single-peer WireGuard endpoint.
    Wireguard {
        #[serde(flatten)]
        config: WireguardConfig,
    },
    /// AnyTLS v2 with bounded TLS-session reuse and UDP-over-TCP v2.
    AnyTls {
        password: String,
        #[serde(default, skip_serializing_if = "AnyTlsPool::is_default")]
        pool: AnyTlsPool,
        #[serde(default = "default_true")]
        udp: bool,
    },
    /// Shadowsocks
    Shadowsocks {
        cipher: String,
        password: String,
        #[serde(default)]
        udp: bool,
        /// shadow-tls plugin (v3) transport
        #[serde(default)]
        shadow_tls: Option<ShadowTlsConfig>,
    },
    /// VMess (V2Ray)
    VMess {
        uuid: String,
        alter_id: u32,
        cipher: String,
        #[serde(default)]
        udp: bool,
    },
    /// VLESS
    VLess {
        uuid: String,
        #[serde(default)]
        flow: Option<String>,
        #[serde(default)]
        udp: bool,
    },
    /// TUIC v5
    Tuic {
        uuid: String,
        password: String,
        #[serde(default = "default_congestion")]
        congestion_control: String,
        #[serde(default, skip_serializing_if = "TuicRelay::is_native")]
        relay: TuicRelay,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        heartbeat_ms: Option<u64>,
        #[serde(default)]
        udp: bool,
    },
    /// Trojan
    Trojan {
        password: String,
        #[serde(default)]
        udp: bool,
    },
    /// Hysteria2
    Hysteria2 {
        password: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        obfs: Option<Hy2Obfs>,
        #[serde(default)]
        udp: bool,
    },
}

#[derive(Clone, Serialize, Deserialize)]
pub struct WireguardPeer {
    pub server: String,
    pub port: u16,
    pub public_key: String,
    pub preshared_key: Option<String>,
    pub allowed_ips: Vec<String>,
    #[serde(default)]
    pub persistent_keepalive: u16,
    pub reserved: Option<[u8; 3]>,
}

/// WireGuard owns no OS routes or DNS settings. Target DNS travels through
/// the encrypted endpoint; bootstrap DNS resolves only the peer hostname.
#[derive(Clone, Serialize, Deserialize)]
pub struct WireguardConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_peers: Vec<WireguardPeer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listen_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reserved: Option<[u8; 3]>,
    pub private_key: String,
    pub public_key: String,
    #[serde(default)]
    pub preshared_key: Option<String>,
    pub local_addresses: Vec<String>,
    #[serde(default = "default_wg_allowed")]
    pub allowed_ips: Vec<String>,
    #[serde(default = "default_wg_dns")]
    pub dns: Vec<std::net::IpAddr>,
    #[serde(default = "default_wg_mtu")]
    pub mtu: usize,
    #[serde(default)]
    pub persistent_keepalive: u16,
    #[serde(default = "default_true")]
    pub udp: bool,
}
impl Default for WireguardConfig {
    fn default() -> Self {
        Self {
            additional_peers: vec![],
            listen_port: None,
            reserved: None,
            private_key: String::new(),
            public_key: String::new(),
            preshared_key: None,
            local_addresses: vec![],
            allowed_ips: default_wg_allowed(),
            dns: default_wg_dns(),
            mtu: default_wg_mtu(),
            persistent_keepalive: 0,
            udp: true,
        }
    }
}
fn default_wg_dns() -> Vec<std::net::IpAddr> {
    vec![
        "1.1.1.1".parse().unwrap(),
        "2606:4700:4700::1111".parse().unwrap(),
    ]
}

fn default_wg_allowed() -> Vec<String> {
    vec!["0.0.0.0/0".into(), "::/0".into()]
}
fn default_wg_mtu() -> usize {
    1280
}
impl std::fmt::Debug for WireguardConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WireguardConfig")
            .field("keys", &"[redacted]")
            .field("address_count", &self.local_addresses.len())
            .field("mtu", &self.mtu)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for ProxyProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyProtocol")
            .field("type", &self.name())
            .field("udp", &self.supports_udp())
            .finish_non_exhaustive()
    }
}

impl ProxyProtocol {
    /// Configured UDP capability; ShadowTLS currently provides streams only.
    pub fn supports_udp(&self) -> bool {
        match self {
            Self::Wireguard { config } => config.udp,
            Self::Shadowsocks {
                udp, shadow_tls, ..
            } => *udp && shadow_tls.is_none(),
            Self::VMess { udp, .. }
            | Self::VLess { udp, .. }
            | Self::Tuic { udp, .. }
            | Self::Trojan { udp, .. }
            | Self::Hysteria2 { udp, .. }
            | Self::AnyTls { udp, .. } => *udp,
        }
    }

    /// Short protocol label, used for grouping and display.
    pub fn name(&self) -> &'static str {
        match self {
            ProxyProtocol::Shadowsocks { .. } => "ss",
            ProxyProtocol::VMess { .. } => "vmess",
            ProxyProtocol::VLess { .. } => "vless",
            ProxyProtocol::Tuic { .. } => "tuic",
            ProxyProtocol::Trojan { .. } => "trojan",
            ProxyProtocol::Hysteria2 { .. } => "hysteria2",
            ProxyProtocol::AnyTls { .. } => "anytls",
            ProxyProtocol::Wireguard { .. } => "wireguard",
        }
    }
}

fn default_congestion() -> String {
    "bbr".to_string()
}

/// Transport layer configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grpc: Option<GrpcConfig>,
    /// Transport type
    #[serde(rename = "type")]
    pub transport_type: TransportType,
    /// TLS settings
    pub tls: Option<TlsConfig>,
    /// WebSocket settings
    pub ws: Option<WsConfig>,
    /// XHTTP transport settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xhttp: Option<XhttpConfig>,
    /// Reality settings
    pub reality: Option<RealityConfig>,
}

/// Transport types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TransportType {
    Tcp,
    Tls,
    WebSocket,
    HttpUpgrade,
    Grpc,
    Xhttp,
    Quic,
    Reality,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GrpcConfig {
    #[serde(default, alias = "serviceName", alias = "grpc-service-name")]
    pub service_name: String,
    #[serde(default, alias = "multiMode")]
    pub multi_mode: bool,
    pub authority: Option<String>,
}

/// TLS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    pub sni: Option<String>,
    #[serde(default)]
    pub skip_cert_verify: bool,
    pub alpn: Option<Vec<String>>,
    pub fingerprint: Option<String>,
}

/// WebSocket configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsConfig {
    pub path: Option<String>,
    pub host: Option<String>,
    pub headers: Option<HashMap<String, String>>,
}

/// XTLS Reality configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealityConfig {
    pub public_key: String,
    pub short_id: String,
    pub sni: Option<String>,
    /// uTLS client fingerprint (Clash `client-fingerprint`, vless URI `fp=`).
    #[serde(default)]
    pub client_fingerprint: Option<String>,
    /// Skip certificate verification (Clash `skip-cert-verify`).
    #[serde(default)]
    pub skip_cert_verify: bool,
}

/// Subscription source
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    pub name: String,
    pub url: String,
    /// Format hint: "clash", "v2ray", "karing", "auto"
    #[serde(default = "default_format")]
    pub format: String,
    /// Last update timestamp (Unix seconds)
    pub last_updated: Option<u64>,
    /// How often to auto-refresh (hours). 0 = manual only. Default: 24.
    #[serde(default = "default_refresh_hours")]
    pub refresh_interval_hours: u64,
    /// Opaque revision used to reject refreshes fetched from an older source.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub refresh_revision: String,
}

fn default_refresh_hours() -> u64 {
    24
}

fn default_format() -> String {
    "auto".to_string()
}

/// A device uses the current global destination members of this rule group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceRuleGroup {
    #[serde(deserialize_with = "deserialize_rule_source")]
    pub source: String,
    pub group: String,
    /// Higher binding priority is evaluated first within the linked-group stage.
    #[serde(default)]
    pub priority: u16,
    /// Empty means use each member's own action.
    #[serde(default)]
    pub target: String,
}

pub fn validate_device_rule_groups(
    links: &[DeviceRuleGroup],
    rules: &[RoutingRule],
    exits: &[ProxyGroupConfig],
) -> Result<(), &'static str> {
    if links.is_empty() {
        return Ok(());
    }
    if links.iter().filter(|b| !b.source.is_empty()).count() > 256 {
        return Err("最多支持 256 个设备规则组关联");
    }
    let mut seen = std::collections::HashSet::new();
    let mut counts = std::collections::HashMap::new();
    for rule in rules.iter().filter(|r| r.is_global_destination()) {
        *counts.entry(rule.group.trim()).or_insert(0usize) += 1;
    }
    let mut total = 0;
    for link in links {
        if !seen.insert((&link.source, &link.group)) || link.group.trim().is_empty() {
            return Err("设备或规则组缺失，或关联重复");
        }
        let target = link.target.as_str();
        if !matches!(target, "" | "direct" | "proxy" | "reject")
            && super::group::node_target_id(target).is_none()
            && !target
                .strip_prefix("group:")
                .is_some_and(|name| exits.iter().any(|g| g.name == name))
        {
            return Err("设备规则组关联引用了无效出口");
        }
        if !link.source.is_empty() {
            total += counts.get(link.group.as_str()).copied().unwrap_or(0);
        }
        if total > 65536 {
            return Err("设备关联的目的规则总数超过 65536");
        }
    }
    Ok(())
}

/// Routing rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingRule {
    /// Optional client IP/CIDR. Empty means all clients. Not a MAC address.
    #[serde(
        default,
        skip_serializing_if = "String::is_empty",
        deserialize_with = "deserialize_rule_source"
    )]
    pub source: String,
    /// Optional display name (e.g. app or purpose). Empty when unset.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Display group (e.g. "跨境", "国内直连"). Empty = ungrouped.
    /// Merlin devices can link to the global destination rules in this group.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,
    /// Rule type: "domain", "domain-suffix", "domain-keyword", "ip-cidr",
    /// "geoip", "dst-port", "match"/"final"
    pub rule_type: String,
    /// Match pattern
    pub pattern: String,
    /// Target: "direct", "proxy", "reject", "group:Name", or "node:<public id>"
    pub target: String,
    /// Whether this rule is active (defaults to true)
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Evaluation priority. Higher values are tried first. Rules with equal
    /// priority keep their list order (stable). Default `0`.
    #[serde(default)]
    pub priority: i32,
}

impl RoutingRule {
    pub fn is_global_destination(&self) -> bool {
        self.source.is_empty()
            && !self.rule_type.eq_ignore_ascii_case("match")
            && !self.rule_type.eq_ignore_ascii_case("final")
    }
}

fn deserialize_rule_source<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    crate::router::normalize_rule_source(&value).map_err(serde::de::Error::custom)
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_single_encoded() {
        assert_eq!(decode_display_name("%45%78%61%6D%70%6C%65"), "Example");
    }

    #[test]
    fn decode_double_encoded() {
        assert_eq!(
            decode_display_name("%2545%2578%2561%256D%2570%256C%2565"),
            "Example"
        );
    }

    #[test]
    fn decode_plus_as_space() {
        assert_eq!(decode_display_name("Hong+Kong+01"), "Hong Kong 01");
    }

    #[test]
    fn decode_mixed_encoding() {
        assert_eq!(decode_display_name("Node-A"), "Node-A");
    }

    #[test]
    fn decode_trims_whitespace() {
        assert_eq!(decode_display_name("  test  "), "test");
    }

    #[test]
    fn decode_plain_text_unchanged() {
        assert_eq!(decode_display_name("Tokyo 01"), "Tokyo 01");
    }

    #[test]
    fn health_check_defaults_when_section_missing() {
        // Backward compatibility: old config files without `health_check` parse fine.
        let yaml = r#"
listen_addr: 127.0.0.1
socks_port: 1080
http_port: 1087
nodes: []
active_node: null
subscriptions: []
rules: []
"#;
        let config: AppConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!config.health_check.enabled);
        assert_eq!(config.health_check.interval_secs, 60);
        assert_eq!(config.health_check.failure_threshold, 2);
        assert!(config.health_check.auto_switch);
    }

    #[test]
    fn health_check_partial_fields_use_defaults() {
        let yaml = r#"
listen_addr: 127.0.0.1
socks_port: 1080
http_port: 1087
nodes: []
active_node: null
subscriptions: []
rules: []
health_check:
  enabled: true
"#;
        let config: AppConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.health_check.enabled);
        assert_eq!(config.health_check.interval_secs, 60);
        assert_eq!(config.health_check.failure_threshold, 2);
        assert!(config.health_check.auto_switch);
    }

    #[test]
    fn normalize_nodes_decodes_names() {
        let mut nodes = vec![Node {
            name: "%48%6F%6E%67%20%4B%6F%6E%67".to_string(),
            server: "1.2.3.4".to_string(),
            port: 443,
            protocol: ProxyProtocol::Shadowsocks {
                cipher: "aes-256-gcm".to_string(),
                password: "test".to_string(),
                udp: false,
                shadow_tls: None,
            },
            transport: None,
            latency_ms: None,
            tags: vec![],
            extra: Default::default(),
        }];
        assert!(normalize_node_names(&mut nodes));
        assert_eq!(nodes[0].name, "Hong Kong");
    }

    #[test]
    fn groups_default_when_section_missing() {
        // Backward compatibility: old config files without `groups` parse fine.
        let yaml = r#"
listen_addr: 127.0.0.1
socks_port: 1080
http_port: 1087
nodes: []
active_node: null
subscriptions: []
rules: []
"#;
        let config: AppConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.groups.is_empty());
    }

    #[test]
    fn groups_roundtrip_preserves_type_members_and_current() {
        let config = AppConfig {
            groups: vec![
                ProxyGroupConfig {
                    name: "Auto".to_string(),
                    gtype: GroupType::UrlTest,
                    members: vec!["fp-a".to_string(), "fp-b".to_string()],
                    current: Some("fp-b".to_string()),
                },
                ProxyGroupConfig {
                    name: "Manual".to_string(),
                    gtype: GroupType::Select,
                    members: vec![],
                    current: None,
                },
                ProxyGroupConfig {
                    name: "Fallback".to_string(),
                    gtype: GroupType::Fallback,
                    members: vec!["fp-a".to_string()],
                    current: None,
                },
            ],
            ..AppConfig::default()
        };
        let yaml = serde_yaml::to_string(&config).unwrap();
        assert!(
            yaml.contains("type: url-test"),
            "gtype serializes as `type`"
        );
        assert!(yaml.contains("type: fallback"));
        let back: AppConfig = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back.groups.len(), 3);
        assert_eq!(back.groups[0].gtype, GroupType::UrlTest);
        assert_eq!(back.groups[0].members, ["fp-a", "fp-b"]);
        assert_eq!(back.groups[0].current.as_deref(), Some("fp-b"));
        assert_eq!(back.groups[1].gtype, GroupType::Select);
        assert_eq!(back.groups[1].current, None);
        assert_eq!(back.groups[2].gtype, GroupType::Fallback);
    }

    #[test]
    fn group_members_and_current_default_when_omitted() {
        let yaml = r#"
listen_addr: 127.0.0.1
socks_port: 1080
http_port: 1087
nodes: []
active_node: null
subscriptions: []
rules: []
groups:
  - name: G
    type: select
"#;
        let config: AppConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.groups.len(), 1);
        assert_eq!(config.groups[0].gtype, GroupType::Select);
        assert!(config.groups[0].members.is_empty());
        assert_eq!(config.groups[0].current, None);
    }

    #[test]
    fn missing_locale_defaults_to_en() {
        let yaml = r#"
listen_addr: 127.0.0.1
socks_port: 1080
http_port: 1087
nodes: []
active_node: null
subscriptions: []
rules: []
"#;
        let cfg: AppConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.locale, "en");
    }

    #[test]
    fn explicit_locale_round_trips() {
        let yaml = r#"
listen_addr: 127.0.0.1
socks_port: 1080
http_port: 1087
nodes: []
active_node: null
subscriptions: []
rules: []
locale: en
"#;
        let cfg: AppConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.locale, "en");
        let out = serde_yaml::to_string(&cfg).unwrap();
        assert!(out.contains("locale:"));
        assert!(out.contains("en"));
    }
}

/// XHTTP settings shared by native, Clash and Xray imports.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct XhttpConfig {
    pub path: Option<String>,
    pub host: Option<String>,
    #[serde(default = "xhttp_default_mode")]
    pub mode: String,
    #[serde(flatten)]
    pub options: std::collections::BTreeMap<String, serde_json::Value>,
}
impl Default for XhttpConfig {
    fn default() -> Self {
        Self {
            path: None,
            host: None,
            mode: xhttp_default_mode(),
            options: Default::default(),
        }
    }
}
fn xhttp_default_mode() -> String {
    "auto".into()
}
impl XhttpConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        super::xhttp::Settings::parse(self).map(|_| ())
    }
}

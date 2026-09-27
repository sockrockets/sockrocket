use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Proxy routing mode
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyMode {
    /// All traffic goes through the proxy node
    #[default]
    Global,
    /// China-direct / overseas-proxy based on built-in rules
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
    /// Whether the proxy was running when the app was last closed (auto-reconnect on next start)
    #[serde(default)]
    pub auto_connect: bool,
    /// Whether TUN mode was active when the app was last closed
    #[serde(default)]
    pub tun_was_enabled: bool,
    /// Node health check + auto-switch settings
    #[serde(default)]
    pub health_check: HealthCheckConfig,
    /// User-defined proxy groups (persisted; UI-level state, not used by routing)
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
            proxy_mode: ProxyMode::default(),
            nodes: Vec::new(),
            active_node: None,
            subscriptions: Vec::new(),
            rules: Vec::new(),
            auto_connect: false,
            tun_was_enabled: false,
            health_check: HealthCheckConfig::default(),
            groups: Vec::new(),
            locale: default_locale(),
        }
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

/// Supported proxy protocols
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ProxyProtocol {
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
        #[serde(default)]
        udp: bool,
    },
}

impl ProxyProtocol {
    /// Short protocol label, used for grouping and display.
    pub fn name(&self) -> &'static str {
        match self {
            ProxyProtocol::Shadowsocks { .. } => "ss",
            ProxyProtocol::VMess { .. } => "vmess",
            ProxyProtocol::VLess { .. } => "vless",
            ProxyProtocol::Tuic { .. } => "tuic",
            ProxyProtocol::Trojan { .. } => "trojan",
            ProxyProtocol::Hysteria2 { .. } => "hysteria2",
        }
    }
}

fn default_congestion() -> String {
    "bbr".to_string()
}

/// Transport layer configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportConfig {
    /// Transport type
    #[serde(rename = "type")]
    pub transport_type: TransportType,
    /// TLS settings
    pub tls: Option<TlsConfig>,
    /// WebSocket settings
    pub ws: Option<WsConfig>,
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
    Quic,
    Reality,
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
}

fn default_refresh_hours() -> u64 {
    24
}

fn default_format() -> String {
    "auto".to_string()
}

/// Routing rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingRule {
    /// Rule type: "domain", "domain-suffix", "ip-cidr", "geoip"
    pub rule_type: String,
    /// Match pattern
    pub pattern: String,
    /// Target: "direct", "proxy", "reject"
    pub target: String,
    /// Whether this rule is active (defaults to true)
    #[serde(default = "default_true")]
    pub enabled: bool,
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
        let mut config = AppConfig::default();
        config.groups = vec![
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
        ];
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

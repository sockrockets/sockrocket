//! Subscription export: turn a set of [`Node`]s into shareable subscription
//! content (V2Ray base64 blob or Clash YAML).
//!
//! Everything is generated locally from the in-memory node list — no network
//! calls and no third-party subscription converters, so credentials never
//! leave this machine.

use base64::Engine;
use base64::engine::general_purpose;
use serde_yaml::{Mapping, Value};

use super::model::*;
use super::v2ray::node_to_share_uri;

/// Output format for an exported subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionFormat {
    /// Newline-separated share URIs, base64-encoded (v2rayN / Shadowrocket style)
    V2ray,
    /// Minimal but complete Clash / mihomo YAML configuration
    Clash,
}

impl SubscriptionFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::V2ray => "v2ray",
            Self::Clash => "clash",
        }
    }

    /// Parse a format hint (query parameter, CLI flag, ...).
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "v2ray" | "base64" | "txt" => Some(Self::V2ray),
            "clash" | "yaml" | "yml" => Some(Self::Clash),
            _ => None,
        }
    }

    pub fn content_type(&self) -> &'static str {
        match self {
            Self::V2ray => "text/plain; charset=utf-8",
            Self::Clash => "text/yaml; charset=utf-8",
        }
    }

    pub fn file_extension(&self) -> &'static str {
        match self {
            Self::V2ray => "txt",
            Self::Clash => "yaml",
        }
    }

    /// Render the given nodes into subscription content in this format.
    pub fn render(&self, nodes: &[Node]) -> String {
        match self {
            Self::V2ray => export_v2ray_base64(nodes),
            Self::Clash => export_clash_config(nodes),
        }
    }
}

// ---------------------------------------------------------------------------
// Cross-module API contract (fixed signatures — other modules depend on these)
// ---------------------------------------------------------------------------

/// Export nodes as a V2Ray subscription: one share URI per line (via
/// [`node_to_share_uri`]), then standard base64 over the whole text.
pub fn nodes_to_v2ray_subscription(nodes: &[Node]) -> String {
    export_v2ray_base64(nodes)
}

/// Export nodes as a minimal but complete Clash YAML config whose `proxies`
/// section round-trips through [`super::clash::parse_clash_config`].
pub fn nodes_to_clash_yaml(nodes: &[Node]) -> anyhow::Result<String> {
    Ok(export_clash_config(nodes))
}

/// De-duplicate nodes by a credential fingerprint (protocol + server + port +
/// key credentials such as password / UUID; the display name is *not* part of
/// the fingerprint). The first occurrence of each fingerprint wins and the
/// original order is preserved. Returns the deduplicated list together with
/// the number of nodes removed.
pub fn dedupe_nodes(nodes: &[Node]) -> (Vec<Node>, usize) {
    let mut seen = std::collections::HashSet::new();
    let mut kept = Vec::with_capacity(nodes.len());
    let mut removed = 0usize;
    for node in nodes {
        if seen.insert(super::dedup::node_fingerprint(node)) {
            kept.push(node.clone());
        } else {
            removed += 1;
        }
    }
    (kept, removed)
}

/// Encode nodes as a base64 subscription blob: one share URI per line, then
/// standard base64 over the whole text (the de-facto subscription format).
pub fn export_v2ray_base64(nodes: &[Node]) -> String {
    let lines = nodes
        .iter()
        .map(node_to_share_uri)
        .collect::<Vec<_>>()
        .join("\n");
    general_purpose::STANDARD.encode(lines.as_bytes())
}

/// Build a minimal complete Clash config: the proxies plus one `select` group
/// containing all of them and a `MATCH` rule, so the output can be dropped
/// into Clash / mihomo / Clash-Verge / FlClash as-is.
pub fn export_clash_config(nodes: &[Node]) -> String {
    let names = unique_names(nodes);

    let mut proxies = Vec::with_capacity(nodes.len());
    let mut group_names = Vec::with_capacity(nodes.len());
    for (node, name) in nodes.iter().zip(names.iter()) {
        if let Some(mapping) = node_to_clash_mapping(node, name) {
            proxies.push(Value::Mapping(mapping));
            group_names.push(Value::String(name.clone()));
        }
    }

    let mut group = Mapping::new();
    put(&mut group, "name", s("PROXY"));
    put(&mut group, "type", s("select"));
    group.insert(s("proxies"), Value::Sequence(group_names));

    let mut root = Mapping::new();
    put(&mut root, "port", Value::Number(7890.into()));
    put(&mut root, "socks-port", Value::Number(7891.into()));
    put(&mut root, "allow-lan", Value::Bool(false));
    put(&mut root, "mode", s("rule"));
    put(&mut root, "log-level", s("warning"));
    root.insert(s("proxies"), Value::Sequence(proxies));
    root.insert(
        s("proxy-groups"),
        Value::Sequence(vec![Value::Mapping(group)]),
    );
    root.insert(s("rules"), Value::Sequence(vec![s("MATCH,PROXY")]));

    serde_yaml::to_string(&Value::Mapping(root)).unwrap_or_default()
}

/// Clash proxy groups reference proxies by name, so names must be unique.
/// Nodes that share a name get a ` #2`, ` #3`, ... suffix (first wins).
fn unique_names(nodes: &[Node]) -> Vec<String> {
    let mut used = std::collections::HashSet::new();
    let mut names = Vec::with_capacity(nodes.len());
    for node in nodes {
        let base = if node.name.trim().is_empty() {
            "node".to_string()
        } else {
            node.name.clone()
        };
        let mut candidate = base.clone();
        let mut counter = 2;
        while !used.insert(candidate.clone()) {
            candidate = format!("{} #{}", base, counter);
            counter += 1;
        }
        names.push(candidate);
    }
    names
}

fn s(value: &str) -> Value {
    Value::String(value.to_string())
}

fn put(mapping: &mut Mapping, key: &str, value: Value) {
    mapping.insert(Value::String(key.to_string()), value);
}

/// Serialize a node into a Clash `proxies` entry. Field names mirror what
/// [`super::clash::parse_clash_config`] reads, so the output round-trips
/// through our own parser (and is understood by mihomo).
fn node_to_clash_mapping(node: &Node, name: &str) -> Option<Mapping> {
    let mut m = Mapping::new();
    put(&mut m, "name", s(name));
    let type_str = match &node.protocol {
        ProxyProtocol::Shadowsocks { .. } => "ss",
        ProxyProtocol::VMess { .. } => "vmess",
        ProxyProtocol::VLess { .. } => "vless",
        ProxyProtocol::Tuic { .. } => "tuic",
        ProxyProtocol::Trojan { .. } => "trojan",
        ProxyProtocol::Hysteria2 { .. } => "hysteria2",
    };
    put(&mut m, "type", s(type_str));
    put(&mut m, "server", s(&node.server));
    put(&mut m, "port", Value::Number(node.port.into()));

    match &node.protocol {
        ProxyProtocol::Shadowsocks {
            cipher,
            password,
            udp,
            shadow_tls,
        } => {
            put(&mut m, "cipher", s(cipher));
            put(&mut m, "password", s(password));
            // mihomo defaults `udp` to true for most protocols; always write
            // it explicitly so importing clients see the intended value.
            put(&mut m, "udp", Value::Bool(*udp));
            // shadow-tls cannot be expressed in an ss:// URI (which keeps
            // exporting as plain SS), but Clash YAML carries it via the
            // plugin fields — our own parser reads these back.
            if let Some(st) = shadow_tls {
                put(&mut m, "plugin", s("shadow-tls"));
                let mut opts = Mapping::new();
                put(&mut opts, "host", s(&st.host));
                put(&mut opts, "password", s(&st.password));
                put(&mut opts, "version", Value::Number(3.into()));
                if st.skip_cert_verify {
                    put(&mut opts, "skip-cert-verify", Value::Bool(true));
                }
                m.insert(s("plugin-opts"), Value::Mapping(opts));
            }
        }
        ProxyProtocol::VMess {
            uuid,
            alter_id,
            cipher,
            udp,
        } => {
            put(&mut m, "uuid", s(uuid));
            put(&mut m, "alterId", Value::Number((*alter_id).into()));
            put(&mut m, "cipher", s(cipher));
            // mihomo defaults `udp` to true for most protocols; always write
            // it explicitly so importing clients see the intended value.
            put(&mut m, "udp", Value::Bool(*udp));
            apply_ws_and_tls(&mut m, node, "servername");
        }
        ProxyProtocol::VLess { uuid, flow, udp } => {
            put(&mut m, "uuid", s(uuid));
            if let Some(flow) = flow {
                put(&mut m, "flow", s(flow));
            }
            // mihomo defaults `udp` to true for most protocols; always write
            // it explicitly so importing clients see the intended value.
            put(&mut m, "udp", Value::Bool(*udp));
            apply_ws_and_tls(&mut m, node, "servername");
        }
        ProxyProtocol::Tuic {
            uuid,
            password,
            congestion_control,
            udp,
        } => {
            put(&mut m, "uuid", s(uuid));
            put(&mut m, "password", s(password));
            put(&mut m, "congestion-controller", s(congestion_control));
            // mihomo defaults `udp` to true for most protocols; always write
            // it explicitly so importing clients see the intended value.
            put(&mut m, "udp", Value::Bool(*udp));
            if let Some(tls) = node.transport.as_ref().and_then(|tc| tc.tls.as_ref()) {
                apply_tls(&mut m, tls, "sni");
            }
        }
        ProxyProtocol::Trojan { password, udp } => {
            put(&mut m, "password", s(password));
            // mihomo defaults `udp` to true for most protocols; always write
            // it explicitly so importing clients see the intended value.
            put(&mut m, "udp", Value::Bool(*udp));
            apply_ws_and_tls(&mut m, node, "sni");
        }
        ProxyProtocol::Hysteria2 { password, udp } => {
            put(&mut m, "password", s(password));
            // mihomo defaults `udp` to true for most protocols; always write
            // it explicitly so importing clients see the intended value.
            put(&mut m, "udp", Value::Bool(*udp));
            if let Some(tls) = node.transport.as_ref().and_then(|tc| tc.tls.as_ref()) {
                apply_tls(&mut m, tls, "sni");
            }
        }
    }

    Some(m)
}

/// Write `network`/`ws-opts`/`tls`/`reality-opts` fields for TCP-based
/// transports (VMess, VLESS, Trojan).
fn apply_ws_and_tls(m: &mut Mapping, node: &Node, sni_key: &str) {
    let Some(tc) = &node.transport else {
        return;
    };

    if tc.transport_type == TransportType::WebSocket {
        put(m, "network", s("ws"));
        let mut ws = Mapping::new();
        if let Some(wsc) = &tc.ws {
            if let Some(path) = &wsc.path {
                put(&mut ws, "path", s(path));
            }
            // Write back every WS header (not just Host): custom headers are
            // part of the connection contract. `wsc.host` wins for the Host
            // key if both are set.
            let mut headers = wsc.headers.clone().unwrap_or_default();
            if let Some(host) = &wsc.host {
                headers.insert("Host".to_string(), host.clone());
            }
            if !headers.is_empty() {
                let mut hm = Mapping::new();
                // Deterministic key order keeps the YAML output stable.
                let mut keys: Vec<&String> = headers.keys().collect();
                keys.sort();
                for key in keys {
                    put(&mut hm, key, s(&headers[key]));
                }
                ws.insert(s("headers"), Value::Mapping(hm));
            }
        }
        m.insert(s("ws-opts"), Value::Mapping(ws));
    }

    if let Some(reality) = &tc.reality {
        put(m, "tls", Value::Bool(true));
        let mut ropts = Mapping::new();
        put(&mut ropts, "public-key", s(&reality.public_key));
        if !reality.short_id.is_empty() {
            put(&mut ropts, "short-id", s(&reality.short_id));
        }
        m.insert(s("reality-opts"), Value::Mapping(ropts));
        if let Some(sni) = &reality.sni {
            put(m, sni_key, s(sni));
        }
        if let Some(fp) = &reality.client_fingerprint {
            put(m, "client-fingerprint", s(fp));
        }
        if reality.skip_cert_verify {
            put(m, "skip-cert-verify", Value::Bool(true));
        }
    }

    if let Some(tls) = &tc.tls {
        apply_tls(m, tls, sni_key);
    }
}

fn apply_tls(m: &mut Mapping, tls: &TlsConfig, sni_key: &str) {
    put(m, "tls", Value::Bool(true));
    if let Some(sni) = &tls.sni {
        put(m, sni_key, s(sni));
    }
    if tls.skip_cert_verify {
        put(m, "skip-cert-verify", Value::Bool(true));
    }
    if let Some(alpn) = &tls.alpn
        && !alpn.is_empty()
    {
        m.insert(
            s("alpn"),
            Value::Sequence(alpn.iter().map(|v| s(v)).collect()),
        );
    }
    if let Some(fp) = &tls.fingerprint {
        put(m, "client-fingerprint", s(fp));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::clash::parse_clash_config;
    use crate::config::v2ray::parse_proxy_uri;
    use std::collections::HashSet;

    fn ss_node() -> Node {
        Node {
            name: "SS HK #1".to_string(),
            server: "1.2.3.4".to_string(),
            port: 8388,
            protocol: ProxyProtocol::Shadowsocks {
                cipher: "aes-256-gcm".to_string(),
                password: "p@ss word".to_string(),
                udp: true,
                shadow_tls: None,
            },
            transport: None,
            latency_ms: None,
            tags: vec![],
            extra: Default::default(),
        }
    }

    fn vmess_ws_tls_node() -> Node {
        Node {
            name: "VMess JP".to_string(),
            server: "jp.example.com".to_string(),
            port: 443,
            protocol: ProxyProtocol::VMess {
                uuid: "b0e80a62-8a51-47f0-91f1-f0f7faf8d9d4".to_string(),
                alter_id: 0,
                cipher: "auto".to_string(),
                udp: false,
            },
            transport: Some(TransportConfig {
                transport_type: TransportType::WebSocket,
                tls: Some(TlsConfig {
                    sni: Some("cdn.example.com".to_string()),
                    skip_cert_verify: false,
                    alpn: Some(vec!["h2".to_string(), "http/1.1".to_string()]),
                    fingerprint: Some("chrome".to_string()),
                }),
                ws: Some(WsConfig {
                    path: Some("/ray".to_string()),
                    host: Some("cdn.example.com".to_string()),
                    headers: None,
                }),
                reality: None,
            }),
            latency_ms: None,
            tags: vec![],
            extra: Default::default(),
        }
    }

    fn vless_reality_node() -> Node {
        Node {
            name: "VLESS US".to_string(),
            server: "us.example.com".to_string(),
            port: 443,
            protocol: ProxyProtocol::VLess {
                uuid: "b85798ef-e9dc-46a4-9a87-8da4499d36d0".to_string(),
                flow: Some("xtls-rprx-vision".to_string()),
                udp: true,
            },
            transport: Some(TransportConfig {
                transport_type: TransportType::Reality,
                tls: None,
                ws: None,
                reality: Some(RealityConfig {
                    public_key: "PUBKEY123".to_string(),
                    short_id: "01ab".to_string(),
                    sni: Some("www.microsoft.com".to_string()),
                    client_fingerprint: Some("chrome".to_string()),
                    skip_cert_verify: true,
                }),
            }),
            latency_ms: None,
            tags: vec![],
            extra: Default::default(),
        }
    }

    fn tuic_node() -> Node {
        Node {
            name: "TUIC SG".to_string(),
            server: "sg.example.com".to_string(),
            port: 8443,
            protocol: ProxyProtocol::Tuic {
                uuid: "d685aef3-b3c4-4932-9a9d-d0c2f6727dfa".to_string(),
                password: "secret".to_string(),
                congestion_control: "bbr".to_string(),
                udp: true,
            },
            transport: Some(TransportConfig {
                transport_type: TransportType::Quic,
                tls: Some(TlsConfig {
                    sni: Some("sg.example.com".to_string()),
                    skip_cert_verify: true,
                    alpn: Some(vec!["h3".to_string()]),
                    fingerprint: None,
                }),
                ws: None,
                reality: None,
            }),
            latency_ms: None,
            tags: vec![],
            extra: Default::default(),
        }
    }

    fn trojan_node() -> Node {
        Node {
            name: "Trojan UK".to_string(),
            server: "uk.example.com".to_string(),
            port: 443,
            protocol: ProxyProtocol::Trojan {
                password: "trojanpass".to_string(),
                udp: false,
            },
            transport: None,
            latency_ms: None,
            tags: vec![],
            extra: Default::default(),
        }
    }

    fn hy2_node() -> Node {
        Node {
            name: "Hy2 DE".to_string(),
            server: "de.example.com".to_string(),
            port: 1443,
            protocol: ProxyProtocol::Hysteria2 {
                password: "hy2pass".to_string(),
                udp: true,
            },
            transport: Some(TransportConfig {
                transport_type: TransportType::Quic,
                tls: Some(TlsConfig {
                    sni: Some("de.example.com".to_string()),
                    skip_cert_verify: true,
                    alpn: None,
                    fingerprint: None,
                }),
                ws: None,
                reality: None,
            }),
            latency_ms: None,
            tags: vec![],
            extra: Default::default(),
        }
    }

    fn sample_nodes() -> Vec<Node> {
        vec![
            ss_node(),
            vmess_ws_tls_node(),
            vless_reality_node(),
            tuic_node(),
            trojan_node(),
            hy2_node(),
        ]
    }

    #[test]
    fn format_parse_and_metadata() {
        assert_eq!(
            SubscriptionFormat::parse("clash"),
            Some(SubscriptionFormat::Clash)
        );
        assert_eq!(
            SubscriptionFormat::parse("V2RAY"),
            Some(SubscriptionFormat::V2ray)
        );
        assert_eq!(
            SubscriptionFormat::parse("yaml"),
            Some(SubscriptionFormat::Clash)
        );
        assert_eq!(SubscriptionFormat::parse("nope"), None);
        assert_eq!(SubscriptionFormat::V2ray.file_extension(), "txt");
        assert_eq!(SubscriptionFormat::Clash.file_extension(), "yaml");
        assert!(SubscriptionFormat::Clash.content_type().contains("yaml"));
    }

    #[test]
    fn v2ray_base64_decodes_back_to_uri_list() {
        let nodes = sample_nodes();
        let blob = export_v2ray_base64(&nodes);

        let decoded = String::from_utf8(general_purpose::STANDARD.decode(&blob).unwrap()).unwrap();
        let lines: Vec<&str> = decoded.lines().collect();
        assert_eq!(lines.len(), nodes.len());

        for (line, original) in lines.iter().zip(nodes.iter()) {
            let parsed = parse_proxy_uri(line)
                .unwrap_or_else(|e| panic!("failed to parse exported URI '{}': {}", line, e));
            assert_eq!(parsed.server, original.server);
            assert_eq!(parsed.port, original.port);
        }

        // SS password with space/punctuation must survive base64 userinfo.
        let parsed_ss = parse_proxy_uri(lines[0]).unwrap();
        match &parsed_ss.protocol {
            ProxyProtocol::Shadowsocks {
                cipher, password, ..
            } => {
                assert_eq!(cipher, "aes-256-gcm");
                assert_eq!(password, "p@ss word");
            }
            _ => panic!("expected Shadowsocks"),
        }
    }

    #[test]
    fn clash_export_parses_back_through_own_parser() {
        let nodes = sample_nodes();
        let yaml = export_clash_config(&nodes);

        // Minimal usable config skeleton is present.
        assert!(yaml.contains("proxies:"));
        assert!(yaml.contains("proxy-groups:"));
        assert!(yaml.contains("MATCH,PROXY"));

        let parsed = parse_clash_config(&yaml).unwrap();
        assert_eq!(parsed.len(), nodes.len());

        for (original, roundtripped) in nodes.iter().zip(parsed.iter()) {
            assert_eq!(roundtripped.server, original.server);
            assert_eq!(roundtripped.port, original.port);
        }

        // VMess WS+TLS transport survives the round-trip.
        let vmess = &parsed[1];
        let transport = vmess.transport.as_ref().expect("vmess transport");
        assert_eq!(transport.transport_type, TransportType::WebSocket);
        let tls = transport.tls.as_ref().expect("vmess tls");
        assert_eq!(tls.sni.as_deref(), Some("cdn.example.com"));
        assert_eq!(transport.ws.as_ref().unwrap().path.as_deref(), Some("/ray"));
        assert_eq!(tls.fingerprint.as_deref(), Some("chrome"));

        // VLESS Reality survives (including fingerprint + skip-cert-verify).
        let vless = &parsed[2];
        let transport = vless.transport.as_ref().expect("vless transport");
        assert_eq!(transport.transport_type, TransportType::Reality);
        let reality = transport.reality.as_ref().expect("vless reality");
        assert_eq!(reality.public_key, "PUBKEY123");
        assert_eq!(reality.short_id, "01ab");
        assert_eq!(reality.client_fingerprint.as_deref(), Some("chrome"));
        assert!(reality.skip_cert_verify);
        match &vless.protocol {
            ProxyProtocol::VLess { uuid, flow, .. } => {
                assert_eq!(uuid, "b85798ef-e9dc-46a4-9a87-8da4499d36d0");
                assert_eq!(flow.as_deref(), Some("xtls-rprx-vision"));
            }
            _ => panic!("expected VLESS"),
        }

        // TUIC SNI / skip-cert-verify survive.
        let tuic = &parsed[3];
        let tls = tuic
            .transport
            .as_ref()
            .and_then(|tc| tc.tls.as_ref())
            .expect("tuic tls");
        assert_eq!(tls.sni.as_deref(), Some("sg.example.com"));
        assert!(tls.skip_cert_verify);

        // Hysteria2 SNI survives.
        let hy2 = &parsed[5];
        let tls = hy2
            .transport
            .as_ref()
            .and_then(|tc| tc.tls.as_ref())
            .expect("hy2 tls");
        assert_eq!(tls.sni.as_deref(), Some("de.example.com"));
    }

    #[test]
    fn clash_export_dedupes_proxy_names() {
        let mut nodes = vec![ss_node(), vmess_ws_tls_node()];
        nodes.push(nodes[0].clone()); // same name "SS HK #1"
        let yaml = export_clash_config(&nodes);

        let value: Value = serde_yaml::from_str(&yaml).unwrap();
        let proxies = value
            .get(Value::String("proxies".to_string()))
            .and_then(Value::as_sequence)
            .expect("proxies sequence");
        let names: HashSet<String> = proxies
            .iter()
            .filter_map(|p| {
                p.get(Value::String("name".to_string()))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect();
        assert_eq!(names.len(), 3, "all exported proxy names must be unique");

        let parsed = parse_clash_config(&yaml).unwrap();
        assert_eq!(parsed.len(), 3);
    }

    // -- Cross-module API contract tests ------------------------------------

    #[test]
    fn contract_v2ray_subscription_decodes_back_to_uri_list() {
        let nodes = sample_nodes();
        let blob = nodes_to_v2ray_subscription(&nodes);

        let decoded = String::from_utf8(general_purpose::STANDARD.decode(&blob).unwrap()).unwrap();
        let lines: Vec<&str> = decoded.lines().collect();
        assert_eq!(lines.len(), nodes.len());

        // Every line is a parseable share URI pointing at the same server:port.
        for (line, original) in lines.iter().zip(nodes.iter()) {
            let parsed = parse_proxy_uri(line)
                .unwrap_or_else(|e| panic!("failed to parse exported URI '{}': {}", line, e));
            assert_eq!(parsed.server, original.server);
            assert_eq!(parsed.port, original.port);
        }
    }

    #[test]
    fn contract_clash_yaml_parses_back_to_same_node_count() {
        let nodes = sample_nodes();
        let yaml = nodes_to_clash_yaml(&nodes).unwrap();
        assert!(yaml.contains("proxies:"));

        let parsed = parse_clash_config(&yaml).unwrap();
        assert_eq!(parsed.len(), nodes.len());
        for (original, roundtripped) in nodes.iter().zip(parsed.iter()) {
            assert_eq!(roundtripped.server, original.server);
            assert_eq!(roundtripped.port, original.port);
        }
    }

    #[test]
    fn contract_dedupe_same_name_same_credentials() {
        let nodes = vec![ss_node(), ss_node()];
        let (kept, removed) = dedupe_nodes(&nodes);
        assert_eq!(removed, 1);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].name, "SS HK #1", "first occurrence wins");
    }

    #[test]
    fn contract_dedupe_different_name_same_credentials() {
        let mut renamed = ss_node();
        renamed.name = "Totally different name".to_string();
        renamed.latency_ms = Some(12);
        renamed.tags = vec!["tag".to_string()];

        let nodes = vec![ss_node(), renamed];
        let (kept, removed) = dedupe_nodes(&nodes);
        assert_eq!(
            removed, 1,
            "name/latency/tags are not part of the fingerprint"
        );
        assert_eq!(kept.len(), 1);
        assert_eq!(
            kept[0].name, "SS HK #1",
            "original order preserved, first wins"
        );
    }

    #[test]
    fn contract_dedupe_different_credentials_kept() {
        let mut other_password = ss_node();
        other_password.protocol = ProxyProtocol::Shadowsocks {
            cipher: "aes-256-gcm".to_string(),
            password: "different-password".to_string(),
            udp: true,
            shadow_tls: None,
        };
        let mut other_port = ss_node();
        other_port.port = 8389;
        let mut other_protocol = trojan_node();
        other_protocol.server = ss_node().server;
        other_protocol.port = ss_node().port;

        let nodes = vec![ss_node(), other_password, other_port, other_protocol];
        let (kept, removed) = dedupe_nodes(&nodes);
        assert_eq!(removed, 0);
        assert_eq!(kept.len(), 4);
    }

    #[test]
    fn clash_export_writes_shadow_tls_plugin_back() {
        let mut node = ss_node();
        node.protocol = ProxyProtocol::Shadowsocks {
            cipher: "2022-blake3-aes-256-gcm".to_string(),
            password: "pw".to_string(),
            udp: true,
            shadow_tls: Some(ShadowTlsConfig {
                host: "apple.com".to_string(),
                password: "10086".to_string(),
                skip_cert_verify: true,
            }),
        };

        let yaml = export_clash_config(&[node]);
        assert!(yaml.contains("plugin: shadow-tls"), "got:\n{}", yaml);
        assert!(yaml.contains("apple.com"));

        // Round-trips through our own Clash parser.
        let parsed = parse_clash_config(&yaml).unwrap();
        assert_eq!(parsed.len(), 1);
        let ProxyProtocol::Shadowsocks { shadow_tls, .. } = &parsed[0].protocol else {
            panic!("expected Shadowsocks");
        };
        let st = shadow_tls.as_ref().expect("shadow-tls survives round-trip");
        assert_eq!(st.host, "apple.com");
        assert_eq!(st.password, "10086");
        assert!(st.skip_cert_verify);
    }

    #[test]
    fn clash_export_writes_all_ws_headers() {
        let mut node = vmess_ws_tls_node();
        let mut headers = std::collections::HashMap::new();
        headers.insert("Host".to_string(), "cdn.example.com".to_string());
        headers.insert("X-Custom".to_string(), "abc".to_string());
        node.transport
            .as_mut()
            .unwrap()
            .ws
            .as_mut()
            .unwrap()
            .headers = Some(headers);

        let yaml = export_clash_config(&[node]);
        assert!(yaml.contains("X-Custom"), "custom header lost:\n{}", yaml);

        let parsed = parse_clash_config(&yaml).unwrap();
        let ws = parsed[0]
            .transport
            .as_ref()
            .and_then(|t| t.ws.as_ref())
            .expect("ws transport");
        assert_eq!(
            ws.headers
                .as_ref()
                .and_then(|h| h.get("X-Custom"))
                .map(String::as_str),
            Some("abc")
        );
        assert_eq!(ws.host.as_deref(), Some("cdn.example.com"));
    }

    #[test]
    fn clash_export_always_writes_udp() {
        // trojan_node() has udp: false — it must be written explicitly, or
        // mihomo (default udp: true) would flip the semantics on import.
        let yaml = export_clash_config(&[trojan_node()]);
        assert!(yaml.contains("udp: false"), "got:\n{}", yaml);

        let yaml = export_clash_config(&[ss_node()]);
        assert!(yaml.contains("udp: true"), "got:\n{}", yaml);
    }

    #[test]
    fn vless_uri_export_includes_reality_fingerprint() {
        let uri = crate::config::v2ray::node_to_share_uri(&vless_reality_node());
        assert!(uri.contains("security=reality"), "got: {}", uri);
        assert!(uri.contains("fp=chrome"), "got: {}", uri);
        assert!(uri.contains("allowInsecure=1"), "got: {}", uri);

        // And the URI parses back with the fingerprint intact.
        let parsed = parse_proxy_uri(&uri).unwrap();
        let reality = parsed
            .transport
            .as_ref()
            .and_then(|t| t.reality.as_ref())
            .expect("reality transport");
        assert_eq!(reality.client_fingerprint.as_deref(), Some("chrome"));
        assert!(reality.skip_cert_verify);
    }
}

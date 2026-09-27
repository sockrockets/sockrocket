use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;

use super::model::*;

/// Clash YAML configuration top-level structure
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct ClashConfig {
    // Deserialized entry-by-entry as raw values first: one malformed proxy
    // (missing port, wrong scalar type, …) must not fail the whole import.
    #[serde(default)]
    proxies: Vec<serde_yaml::Value>,
    #[serde(default, rename = "proxy-groups")]
    proxy_groups: Vec<ClashProxyGroup>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct ClashProxy {
    name: String,
    #[serde(rename = "type")]
    proxy_type: String,
    server: String,
    // Optional: Hysteria2 port-hopping configs carry `ports: "a-b"` instead.
    #[serde(default)]
    port: Option<u16>,
    /// Hysteria2 port-hopping range, e.g. "40001-50000".
    #[serde(default)]
    ports: Option<String>,
    // Shadowsocks fields
    cipher: Option<String>,
    password: Option<String>,
    // VMess fields
    uuid: Option<String>,
    #[serde(rename = "alterId", default)]
    alter_id: Option<u32>,
    // VLESS fields
    flow: Option<String>,
    // TLS
    #[serde(default)]
    tls: bool,
    servername: Option<String>,
    sni: Option<String>,
    #[serde(rename = "skip-cert-verify", default)]
    skip_cert_verify: bool,
    alpn: Option<Vec<String>>,
    #[serde(rename = "client-fingerprint")]
    client_fingerprint: Option<String>,
    // Network/Transport
    network: Option<String>,
    #[serde(rename = "ws-opts")]
    ws_opts: Option<ClashWsOpts>,
    // TUIC
    #[serde(rename = "congestion-controller")]
    congestion_controller: Option<String>,
    // Reality
    #[serde(rename = "reality-opts")]
    reality_opts: Option<ClashRealityOpts>,
    // UDP
    #[serde(default)]
    udp: Option<bool>,
    // SS plugin (e.g. shadow-tls)
    plugin: Option<String>,
    #[serde(rename = "plugin-opts")]
    plugin_opts: Option<serde_yaml::Value>,
    // Hysteria2 bandwidth
    up: Option<String>,
    down: Option<String>,
    // Hysteria2 obfuscation (salamander). Parsed so we can *reject* obfs
    // nodes explicitly instead of silently importing a node that cannot
    // connect.
    obfs: Option<String>,
    #[serde(rename = "obfs-password")]
    obfs_password: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClashWsOpts {
    path: Option<String>,
    headers: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize)]
struct ClashRealityOpts {
    #[serde(rename = "public-key")]
    public_key: Option<String>,
    #[serde(rename = "short-id")]
    short_id: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct ClashProxyGroup {
    name: String,
    #[serde(rename = "type")]
    group_type: String,
    #[serde(default)]
    proxies: Vec<String>,
}

/// Parse a Clash YAML configuration string into a list of nodes
pub fn parse_clash_config(yaml_str: &str) -> Result<Vec<Node>> {
    let config: ClashConfig =
        serde_yaml::from_str(yaml_str).context("Failed to parse Clash YAML")?;

    let mut nodes = Vec::new();
    for value in config.proxies {
        // Per-entry deserialization: a single malformed proxy is skipped
        // instead of failing the entire subscription.
        let proxy: ClashProxy = match serde_yaml::from_value(value) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("Skipping malformed proxy entry: {}", e);
                continue;
            }
        };
        match convert_clash_proxy(&proxy) {
            Ok(node) => nodes.push(node),
            Err(e) => {
                tracing::warn!("Skipping proxy '{}': {}", proxy.name, e);
            }
        }
    }

    Ok(nodes)
}

/// Resolve the connect port: `port` first, otherwise the first port of a
/// Hysteria2 `ports` hopping range ("40001-50000" / "40001,40002").
fn resolve_port(proxy: &ClashProxy) -> Result<u16> {
    if let Some(p) = proxy.port {
        return Ok(p);
    }
    if let Some(ref range) = proxy.ports {
        let first = range
            .split(['-', ','])
            .next()
            .unwrap_or("")
            .trim()
            .parse::<u16>();
        if let Ok(p) = first {
            return Ok(p);
        }
    }
    anyhow::bail!("missing port (and no usable ports range)")
}

/// Parse the SS `plugin` / `plugin-opts` fields.
///
/// `shadow-tls` v3 is supported and becomes `Some(ShadowTlsConfig)`; no
/// plugin at all is `None`; anything else is rejected.
fn parse_shadow_tls_plugin(proxy: &ClashProxy) -> Result<Option<ShadowTlsConfig>> {
    let Some(plugin) = proxy.plugin.as_deref() else {
        return Ok(None);
    };
    if plugin != "shadow-tls" {
        anyhow::bail!(
            "SS node '{}' uses unsupported plugin '{}'",
            proxy.name,
            plugin
        );
    }

    #[derive(Debug, Deserialize)]
    struct ShadowTlsOpts {
        host: Option<String>,
        password: Option<String>,
        version: Option<u32>,
        #[serde(rename = "skip-cert-verify", default)]
        skip_cert_verify: bool,
    }

    let opts: ShadowTlsOpts = match proxy.plugin_opts {
        Some(ref value) => serde_yaml::from_value(value.clone())
            .context("SS shadow-tls plugin-opts are malformed")?,
        None => anyhow::bail!(
            "SS node '{}' shadow-tls plugin is missing plugin-opts",
            proxy.name
        ),
    };
    if opts.version.unwrap_or(3) != 3 {
        anyhow::bail!(
            "SS node '{}' shadow-tls version {:?} is unsupported (only v3)",
            proxy.name,
            opts.version
        );
    }
    Ok(Some(ShadowTlsConfig {
        host: opts
            .host
            .context("SS shadow-tls plugin-opts missing host")?,
        password: opts
            .password
            .context("SS shadow-tls plugin-opts missing password")?,
        skip_cert_verify: opts.skip_cert_verify,
    }))
}

/// Check if a YAML string looks like Clash config
pub fn is_clash_config(content: &str) -> bool {
    content.contains("proxies:") || content.contains("proxy-groups:")
}

fn convert_clash_proxy(proxy: &ClashProxy) -> Result<Node> {
    let udp = proxy.udp.unwrap_or(false);

    let protocol = match proxy.proxy_type.as_str() {
        "ss" => {
            let cipher = proxy.cipher.as_ref().context("SS missing cipher")?.clone();
            let password = proxy
                .password
                .as_ref()
                .context("SS missing password")?
                .clone();
            let shadow_tls = parse_shadow_tls_plugin(proxy)?;
            ProxyProtocol::Shadowsocks {
                cipher,
                password,
                udp,
                shadow_tls,
            }
        }
        "vmess" => {
            let uuid = proxy.uuid.as_ref().context("VMess missing uuid")?.clone();
            let alter_id = proxy.alter_id.unwrap_or(0);
            let cipher = proxy.cipher.clone().unwrap_or_else(|| "auto".to_string());
            ProxyProtocol::VMess {
                uuid,
                alter_id,
                cipher,
                udp,
            }
        }
        "vless" => {
            let uuid = proxy.uuid.as_ref().context("VLESS missing uuid")?.clone();
            ProxyProtocol::VLess {
                uuid,
                flow: proxy.flow.clone(),
                udp,
            }
        }
        "tuic" => {
            let uuid = proxy.uuid.as_ref().context("TUIC missing uuid")?.clone();
            let password = proxy
                .password
                .as_ref()
                .context("TUIC missing password")?
                .clone();
            let congestion_control = proxy
                .congestion_controller
                .clone()
                .unwrap_or_else(|| "bbr".to_string());
            ProxyProtocol::Tuic {
                uuid,
                password,
                congestion_control,
                udp,
            }
        }
        "trojan" => {
            let password = proxy
                .password
                .as_ref()
                .context("Trojan missing password")?
                .clone();
            ProxyProtocol::Trojan { password, udp }
        }
        "hysteria2" | "hy2" => {
            if let Some(ref obfs) = proxy.obfs {
                anyhow::bail!(
                    "Hysteria2 node '{}' uses obfs '{}' (obfs-password {:?}): obfuscation is not supported",
                    proxy.name,
                    obfs,
                    proxy.obfs_password.as_deref().unwrap_or("")
                );
            }
            let password = proxy
                .password
                .as_ref()
                .context("Hysteria2 missing password")?
                .clone();
            ProxyProtocol::Hysteria2 { password, udp }
        }
        other => anyhow::bail!("Unsupported proxy type: {}", other),
    };

    let transport = build_transport(proxy)?;

    Ok(Node {
        name: super::model::decode_display_name(&proxy.name),
        server: proxy.server.clone(),
        port: resolve_port(proxy)?,
        protocol,
        transport,
        latency_ms: None,
        tags: vec![],
        extra: HashMap::new(),
    })
}

fn build_transport(proxy: &ClashProxy) -> Result<Option<TransportConfig>> {
    let network = proxy.network.as_deref().unwrap_or("tcp");
    let proxy_type = proxy.proxy_type.as_str();

    // Only plain TCP and WebSocket are supported. Anything else
    // (grpc/h2/http/httpupgrade/xhttp/...) must be rejected loudly — silently
    // downgrading to bare TCP/TLS produces a node that imports fine but can
    // never connect.
    if !matches!(network, "tcp" | "ws") {
        anyhow::bail!(
            "node '{}' uses unsupported network '{}' (only tcp/ws are supported)",
            proxy.name,
            network
        );
    }

    // TUIC and Hysteria2 always use QUIC (TLS is implicit)
    let force_tls = matches!(proxy_type, "tuic" | "hysteria2" | "hy2");

    // Reality transport
    if let Some(ref reality) = proxy.reality_opts
        && let Some(pk) = &reality.public_key
    {
        return Ok(Some(TransportConfig {
            transport_type: TransportType::Reality,
            tls: None,
            ws: None,
            reality: Some(RealityConfig {
                public_key: pk.clone(),
                short_id: reality.short_id.clone().unwrap_or_default(),
                sni: proxy.servername.clone().or_else(|| proxy.sni.clone()),
                client_fingerprint: proxy.client_fingerprint.clone(),
                skip_cert_verify: proxy.skip_cert_verify,
            }),
        }));
    }

    let has_tls = proxy.tls || force_tls;
    let has_ws = network == "ws";

    if !has_tls && !has_ws {
        return Ok(None);
    }

    let tls = if has_tls {
        Some(TlsConfig {
            sni: proxy.servername.clone().or_else(|| proxy.sni.clone()),
            skip_cert_verify: proxy.skip_cert_verify,
            alpn: proxy.alpn.clone(),
            fingerprint: proxy.client_fingerprint.clone(),
        })
    } else {
        None
    };

    let ws = if has_ws {
        proxy.ws_opts.as_ref().map(|opts| {
            let host = opts.headers.as_ref().and_then(|h| h.get("Host").cloned());
            WsConfig {
                path: opts.path.clone(),
                host,
                headers: opts.headers.clone(),
            }
        })
    } else {
        None
    };

    let transport_type = match (has_ws, has_tls) {
        (true, _) => TransportType::WebSocket,
        (false, true) => TransportType::Tls,
        _ => TransportType::Tcp,
    };

    Ok(Some(TransportConfig {
        transport_type,
        tls,
        ws,
        reality: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_clash_ss() {
        let yaml = r#"
proxies:
  - name: "SS-HK"
    type: ss
    server: 1.2.3.4
    port: 8388
    cipher: aes-256-gcm
    password: "test123"
    udp: true
"#;
        let nodes = parse_clash_config(yaml).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "SS-HK");
        assert_eq!(nodes[0].server, "1.2.3.4");
        assert_eq!(nodes[0].port, 8388);
        match &nodes[0].protocol {
            ProxyProtocol::Shadowsocks {
                cipher, password, ..
            } => {
                assert_eq!(cipher, "aes-256-gcm");
                assert_eq!(password, "test123");
            }
            _ => panic!("Expected Shadowsocks"),
        }
    }

    #[test]
    fn test_parse_clash_vmess_ws_tls() {
        let yaml = r#"
proxies:
  - name: "VMess-JP"
    type: vmess
    server: 5.6.7.8
    port: 443
    uuid: "b0e80a62-8a51-47f0-91f1-f0f7faf8d9d4"
    alterId: 0
    cipher: auto
    tls: true
    servername: example.com
    network: ws
    ws-opts:
      path: /v2ray
      headers:
        Host: example.com
"#;
        let nodes = parse_clash_config(yaml).unwrap();
        assert_eq!(nodes.len(), 1);
        let node = &nodes[0];
        assert_eq!(node.name, "VMess-JP");
        let transport = node.transport.as_ref().unwrap();
        assert_eq!(transport.transport_type, TransportType::WebSocket);
        assert!(transport.tls.is_some());
        assert!(transport.ws.is_some());
        assert_eq!(
            transport.ws.as_ref().unwrap().path.as_deref(),
            Some("/v2ray")
        );
    }

    #[test]
    fn test_parse_clash_vless_reality() {
        let yaml = r#"
proxies:
  - name: "VLESS-US"
    type: vless
    server: 9.10.11.12
    port: 443
    uuid: "b85798ef-e9dc-46a4-9a87-8da4499d36d0"
    flow: xtls-rprx-vision
    tls: true
    reality-opts:
      public-key: "abc123"
      short-id: "0123456789abcdef"
    servername: www.example.com
"#;
        let nodes = parse_clash_config(yaml).unwrap();
        assert_eq!(nodes.len(), 1);
        let node = &nodes[0];
        let transport = node.transport.as_ref().unwrap();
        assert_eq!(transport.transport_type, TransportType::Reality);
        let reality = transport.reality.as_ref().unwrap();
        assert_eq!(reality.public_key, "abc123");
    }

    #[test]
    fn test_parse_clash_tuic() {
        let yaml = r#"
proxies:
  - name: "TUIC-SG"
    type: tuic
    server: 13.14.15.16
    port: 443
    uuid: "d685aef3-b3c4-4932-9a9d-d0c2f6727dfa"
    password: "supersecret"
    congestion-controller: bbr
    udp: true
"#;
        let nodes = parse_clash_config(yaml).unwrap();
        assert_eq!(nodes.len(), 1);
        match &nodes[0].protocol {
            ProxyProtocol::Tuic {
                uuid,
                password,
                congestion_control,
                ..
            } => {
                assert_eq!(uuid, "d685aef3-b3c4-4932-9a9d-d0c2f6727dfa");
                assert_eq!(password, "supersecret");
                assert_eq!(congestion_control, "bbr");
            }
            _ => panic!("Expected TUIC"),
        }
    }

    #[test]
    fn test_parse_mixed_proxies() {
        let yaml = r#"
proxies:
  - name: "SS"
    type: ss
    server: 1.1.1.1
    port: 8388
    cipher: aes-256-gcm
    password: "pass1"
  - name: "VMess"
    type: vmess
    server: 2.2.2.2
    port: 443
    uuid: "uuid1"
  - name: "Trojan"
    type: trojan
    server: 3.3.3.3
    port: 443
    password: "pass2"
  - name: "Unknown"
    type: snell
    server: 4.4.4.4
    port: 1234
"#;
        let nodes = parse_clash_config(yaml).unwrap();
        // snell is unsupported, should be skipped
        assert_eq!(nodes.len(), 3);
    }

    #[test]
    fn test_parse_malformed_entries_are_skipped() {
        // Real-world converter output quirks:
        // - Hysteria2 port hopping uses `ports: "a-b"` instead of `port`
        // - Entries missing both port and ports must not fail the import
        // - Entries with a wrong scalar type must not fail the import
        let yaml = r#"
proxies:
  - { name: "Hy2-Hop", type: hysteria2, server: hk1.example.com, ports: 40001-50000, password: pw, sni: example.net }
  - { name: "NoPort", type: ss, server: 1.2.3.4, cipher: aes-256-gcm, password: pw }
  - { name: "BadPort", type: ss, server: 1.2.3.4, port: { oops: 1 }, cipher: aes-256-gcm, password: pw }
  - { name: "SS-OK", type: ss, server: 5.6.7.8, port: 8388, cipher: aes-256-gcm, password: pw }
"#;
        let nodes = parse_clash_config(yaml).unwrap();
        assert_eq!(nodes.len(), 2, "malformed entries must be skipped");
        // Port-hopping range resolves to its first port.
        assert_eq!(nodes[0].name, "Hy2-Hop");
        assert_eq!(nodes[0].port, 40001);
        assert_eq!(nodes[1].name, "SS-OK");
        assert_eq!(nodes[1].port, 8388);
    }

    #[test]
    fn test_parse_clash_meta_subscription() {
        // Real-world Clash Meta subscription format with:
        // - VLESS + Reality (no short-id) → should use Reality transport
        // - SS + shadow-tls plugin → should parse with ShadowTlsConfig
        // - Hysteria2 with sni field → SNI should be picked up
        let yaml = r#"
port: 7890
socks-port: 7891
proxies:
  - { name: "US-Xr1",type: vless,server: vless.example.com,port: 443,uuid: 00000000-0000-0000-0000-000000000001,network: tcp,tls: true,udp: false,servername: www.example.com,reality-opts: {public-key: AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA},client-fingerprint: safari}
  - { name: "US-SS-TLS",type: ss,server: ss.example.com,port: 443,password: "testpass",udp: false,cipher: 2022-blake3-aes-256-gcm,plugin: shadow-tls,plugin-opts: {host: www.example.com,password: "st-password",version: 3}}
  - { name: "HK-Hy2-1",type: hysteria2,server: hy2.example.com,port: 443,password: 00000000-0000-0000-0000-000000000001,up: "50 Mbps",down: "100 Mbps",skip-cert-verify: true,sni: www.example.com}
proxy-groups: []
"#;
        let nodes = parse_clash_config(yaml).unwrap();
        assert_eq!(nodes.len(), 3, "all three nodes should parse");

        // VLESS Reality: transport should be Reality, not plain TLS
        let vless = &nodes[0];
        assert_eq!(vless.name, "US-Xr1");
        let transport = vless.transport.as_ref().unwrap();
        assert!(
            matches!(transport.transport_type, TransportType::Reality),
            "VLESS Reality should have Reality transport, got {:?}",
            transport.transport_type
        );
        let reality = transport.reality.as_ref().unwrap();
        assert_eq!(
            reality.public_key,
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        );
        assert_eq!(reality.short_id, ""); // default empty when not specified
        assert_eq!(reality.sni.as_deref(), Some("www.example.com"));

        // SS + shadow-tls: plugin opts should land in ShadowTlsConfig
        let ss = &nodes[1];
        assert_eq!(ss.name, "US-SS-TLS");
        let ProxyProtocol::Shadowsocks {
            cipher,
            password,
            shadow_tls,
            ..
        } = &ss.protocol
        else {
            panic!("expected Shadowsocks, got {:?}", ss.protocol);
        };
        assert_eq!(cipher, "2022-blake3-aes-256-gcm");
        assert_eq!(password, "testpass");
        let st = shadow_tls.as_ref().expect("shadow-tls config parsed");
        assert_eq!(st.host, "www.example.com");
        assert_eq!(st.password, "st-password");
        assert!(!st.skip_cert_verify);

        // Hysteria2: SNI should be picked up from `sni` field
        let hy2 = &nodes[2];
        assert_eq!(hy2.name, "HK-Hy2-1");
        let transport = hy2.transport.as_ref().unwrap();
        let tls = transport.tls.as_ref().unwrap();
        assert_eq!(tls.sni.as_deref(), Some("www.example.com"));
        assert!(tls.skip_cert_verify);
    }

    #[test]
    fn test_parse_clash_ss_shadow_tls_skip_cert_verify() {
        let yaml = r#"
proxies:
  - { name: "SS-ST",type: ss,server: s.example.com,port: 443,password: "p",udp: false,cipher: aes-256-gcm,plugin: shadow-tls,plugin-opts: {host: real.com,password: "pw",version: 3,skip-cert-verify: true}}
"#;
        let nodes = parse_clash_config(yaml).unwrap();
        let ProxyProtocol::Shadowsocks { shadow_tls, .. } = &nodes[0].protocol else {
            panic!("expected Shadowsocks");
        };
        let st = shadow_tls.as_ref().expect("shadow-tls config parsed");
        assert!(st.skip_cert_verify);
    }

    #[test]
    fn test_parse_clash_ss_unsupported_plugin_rejected() {
        let yaml = r#"
proxies:
  - { name: "SS-OBFS",type: ss,server: s.example.com,port: 8388,password: "p",udp: false,cipher: aes-256-gcm,plugin: obfs,plugin-opts: {mode: tls}}
"#;
        let nodes = parse_clash_config(yaml).unwrap();
        assert!(nodes.is_empty(), "unsupported plugin must skip the node");
    }

    #[test]
    fn test_unknown_network_is_rejected_not_downgraded() {
        // grpc/h2/httpupgrade/... used to be silently downgraded to bare
        // TLS/TCP, producing a node that imports but can never connect.
        for network in ["grpc", "h2", "http", "httpupgrade", "xhttp"] {
            let yaml = format!(
                r#"
proxies:
  - {{ name: "OK", type: ss, server: 1.2.3.4, port: 8388, cipher: aes-256-gcm, password: pw }}
  - {{ name: "Bad-Net", type: vmess, server: 5.6.7.8, port: 443, uuid: u, network: {network} }}
"#
            );
            let nodes = parse_clash_config(&yaml).unwrap();
            assert_eq!(
                nodes.len(),
                1,
                "network '{network}' node must be skipped, not downgraded"
            );
            assert_eq!(nodes[0].name, "OK");
        }
    }

    #[test]
    fn test_hysteria2_obfs_is_rejected() {
        // Obfuscated Hy2 nodes cannot work without obfs support; importing
        // them silently would yield a dead node.
        let yaml = r#"
proxies:
  - { name: "Hy2-Plain", type: hysteria2, server: a.example.com, port: 443, password: pw }
  - { name: "Hy2-Obfs", type: hysteria2, server: b.example.com, port: 443, password: pw, obfs: salamander, obfs-password: secret }
"#;
        let nodes = parse_clash_config(yaml).unwrap();
        assert_eq!(nodes.len(), 1, "obfs node must be skipped");
        assert_eq!(nodes[0].name, "Hy2-Plain");
    }

    #[test]
    fn test_reality_keeps_client_fingerprint_and_skip_cert_verify() {
        let yaml = r#"
proxies:
  - { name: "VLESS-R", type: vless, server: 1.2.3.4, port: 443, uuid: u, tls: true, reality-opts: {public-key: pk}, servername: example.com, client-fingerprint: chrome, skip-cert-verify: true }
"#;
        let nodes = parse_clash_config(yaml).unwrap();
        assert_eq!(nodes.len(), 1);
        let reality = nodes[0]
            .transport
            .as_ref()
            .and_then(|t| t.reality.as_ref())
            .expect("reality transport");
        assert_eq!(reality.client_fingerprint.as_deref(), Some("chrome"));
        assert!(reality.skip_cert_verify);
    }
}

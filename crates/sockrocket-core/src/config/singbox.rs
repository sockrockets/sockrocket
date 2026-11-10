//! Sing-box configuration parser.
//!
//! Parses sing-box JSON outbound format into sockrocket Node structs.
//! Supports: TUIC, Hysteria2, VMess, VLESS, Trojan, Shadowsocks.

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::Deserialize;

use super::model::*;

// --- Sing-box config structures ---

#[derive(Debug, Deserialize)]
struct SingBoxConfig {
    #[serde(default)]
    outbounds: Vec<serde_json::Value>,
    #[serde(default)]
    endpoints: Vec<serde_json::Value>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct SingBoxOutbound {
    #[serde(rename = "type")]
    outbound_type: String,
    #[serde(default)]
    tag: String,
    #[serde(default)]
    server: String,
    #[serde(default)]
    server_port: u16,

    // Auth fields
    uuid: Option<String>,
    password: Option<String>,
    method: Option<String>, // SS cipher
    flow: Option<String>,   // VLESS

    #[serde(default)]
    global_padding: bool,
    #[serde(default)]
    authenticated_length: bool,
    security: Option<String>,
    alter_id: Option<u32>,
    obfs: Option<serde_json::Value>,
    server_ports: Option<serde_json::Value>,
    hop_interval: Option<String>,
    hop_interval_max: Option<String>,
    up_mbps: Option<u64>,
    down_mbps: Option<u64>,

    // TUIC
    congestion_control: Option<String>,
    udp_relay_mode: Option<String>,
    #[serde(default)]
    udp_over_stream: bool,
    #[serde(default)]
    zero_rtt_handshake: bool,
    heartbeat: Option<String>,
    idle_session_check_interval: Option<String>,
    idle_session_timeout: Option<String>,
    #[serde(default)]
    min_idle_session: usize,

    // TLS
    tls: Option<SingBoxTls>,

    // Transport
    transport: Option<SingBoxTransport>,

    network: Option<String>,

    // UDP
    #[serde(default)]
    udp: Option<bool>,

    // Multiplex
    multiplex: Option<serde_json::Value>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct SingBoxTls {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    insecure: bool,
    server_name: Option<String>,
    alpn: Option<Vec<String>>,
    #[serde(default)]
    disable_sni: bool,
    utls: Option<SingBoxUtls>,
    reality: Option<SingBoxReality>,
    #[serde(default, flatten)]
    extensions: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct SingBoxReality {
    #[serde(default)]
    enabled: bool,
    public_key: Option<String>,
    short_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SingBoxUtls {
    enabled: Option<bool>,
    fingerprint: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SingBoxTransport {
    host: Option<String>,
    #[serde(rename = "type")]
    transport_type: Option<String>,
    path: Option<String>,
    headers: Option<HashMap<String, String>>,
}

/// Check if JSON content looks like a sing-box config.
pub fn is_singbox_config(content: &str) -> bool {
    (content.contains("\"outbounds\"")
        && (content.contains("\"server_port\"") || content.contains("\"server_ports\"")))
        || content.contains("\"endpoints\"")
}

/// Parse sing-box JSON config into Node list.
pub fn parse_singbox_config(json_str: &str) -> Result<Vec<Node>> {
    let config: SingBoxConfig =
        serde_json::from_str(json_str).context("Failed to parse sing-box JSON")?;

    let mut nodes = Vec::new();
    for value in config.outbounds.iter().chain(config.endpoints.iter()) {
        if value.get("type").and_then(serde_json::Value::as_str) == Some("wireguard") {
            match super::wireguard::from_singbox(value) {
                Ok(node) => nodes.push(node),
                Err(_) => tracing::warn!("Skipping invalid or unsupported WireGuard endpoint"),
            }
            continue;
        }
        let outbound: SingBoxOutbound = match serde_json::from_value(value.clone()) {
            Ok(v) => v,
            Err(_) => {
                tracing::warn!("Skipping malformed sing-box outbound");
                continue;
            }
        };
        match convert_singbox_outbound(&outbound) {
            Ok(Some(node)) => nodes.push(node),
            Ok(None) => {}
            Err(_) => tracing::warn!("Skipping unsupported or invalid sing-box outbound"),
        }
    }
    Ok(nodes)
}

/// Parse a single sing-box outbound JSON object.
pub fn parse_singbox_outbound(json_str: &str) -> Result<Option<Node>> {
    let value: serde_json::Value =
        serde_json::from_str(json_str).context("Failed to parse sing-box outbound")?;
    if value.get("type").and_then(serde_json::Value::as_str) == Some("wireguard") {
        return super::wireguard::from_singbox(&value).map(Some);
    }
    let outbound: SingBoxOutbound =
        serde_json::from_value(value).context("Failed to parse sing-box outbound")?;
    convert_singbox_outbound(&outbound)
}

// sing-box accepts Go duration strings, including compound and fractional units.
// The timer and Mihomo's corresponding field have millisecond resolution.
fn duration_millis(value: &str) -> Result<u64> {
    let mut rest = value.strip_prefix('+').unwrap_or(value);
    if rest == "0" {
        return Ok(0);
    }
    anyhow::ensure!(!rest.is_empty(), "Empty duration");
    let mut nanos = 0.0_f64;
    while !rest.is_empty() {
        let end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .context("Missing heartbeat time unit")?;
        let number: f64 = rest[..end].parse().context("Invalid duration")?;
        rest = &rest[end..];
        let (unit, scale) = [
            ("ns", 1.),
            ("us", 1e3),
            ("µs", 1e3),
            ("μs", 1e3),
            ("ms", 1e6),
            ("s", 1e9),
            ("m", 60e9),
            ("h", 3600e9),
        ]
        .into_iter()
        .find(|(u, _)| rest.starts_with(u))
        .context("Invalid heartbeat time unit")?;
        nanos += number * scale;
        rest = &rest[unit.len()..];
    }
    anyhow::ensure!(nanos <= i64::MAX as f64, "duration exceeds duration range");
    Ok((nanos / 1e6).ceil() as u64)
}

fn convert_singbox_outbound(ob: &SingBoxOutbound) -> Result<Option<Node>> {
    let udp = match ob.network.as_deref() {
        None | Some("") => ob.udp.unwrap_or(true),
        Some("tcp") => false,
        Some("udp") if ob.outbound_type == "trojan" && ob.udp != Some(false) => true,
        Some(_) => anyhow::bail!("Unsupported outbound network restriction"),
    };

    let protocol = match ob.outbound_type.as_str() {
        "anytls" => ProxyProtocol::AnyTls {
            password: ob.password.clone().context("AnyTLS missing password")?,
            pool: AnyTlsPool {
                check_interval_ms: ob
                    .idle_session_check_interval
                    .as_deref()
                    .map(duration_millis)
                    .transpose()?,
                idle_timeout_ms: ob
                    .idle_session_timeout
                    .as_deref()
                    .map(duration_millis)
                    .transpose()?,
                min_idle: ob.min_idle_session,
                disable_reuse: false,
            }
            .checked()?,
            udp,
        },
        "tuic" => {
            let uuid = ob.uuid.as_ref().context("TUIC missing uuid")?.clone();
            let password = ob
                .password
                .as_ref()
                .context("TUIC missing password")?
                .clone();
            let congestion_control = ob
                .congestion_control
                .clone()
                .unwrap_or_else(|| "cubic".to_string());
            ProxyProtocol::Tuic {
                uuid,
                password,
                congestion_control,
                heartbeat_ms: ob.heartbeat.as_deref().map(duration_millis).transpose()?,
                relay: TuicRelay::parse(
                    ob.udp_relay_mode.as_deref(),
                    ob.udp_over_stream.then_some(2),
                )?,
                udp,
            }
        }
        "hysteria2" | "hy2" => {
            let password = ob
                .password
                .as_ref()
                .context("Hysteria2 missing password")?
                .clone();
            let obfs: Option<Hy2Obfs> = ob.obfs.clone().map(serde_json::from_value).transpose()?;
            if let Some(o) = &obfs {
                o.validate()?;
            }
            ProxyProtocol::Hysteria2 {
                password,
                udp,
                obfs,
            }
        }
        "vless" => {
            let uuid = ob.uuid.as_ref().context("VLESS missing uuid")?.clone();
            ProxyProtocol::VLess {
                uuid,
                flow: ob.flow.clone(),
                udp,
            }
        }
        "vmess" => {
            let uuid = ob.uuid.as_ref().context("VMess missing uuid")?.clone();
            ProxyProtocol::VMess {
                uuid,
                alter_id: ob.alter_id.unwrap_or(0),
                cipher: ob.security.clone().unwrap_or_else(|| "auto".into()),
                udp,
            }
        }
        "trojan" => {
            let password = ob
                .password
                .as_ref()
                .context("Trojan missing password")?
                .clone();
            ProxyProtocol::Trojan { password, udp }
        }
        "shadowsocks" | "ss" => {
            let cipher = ob
                .method
                .as_ref()
                .context("Shadowsocks missing method")?
                .clone();
            let password = ob
                .password
                .as_ref()
                .context("Shadowsocks missing password")?
                .clone();
            ProxyProtocol::Shadowsocks {
                cipher,
                password,
                udp,
                shadow_tls: None,
            }
        }
        // Non-proxy types: skip
        "direct" | "block" | "dns" | "selector" | "urltest" => return Ok(None),
        other => anyhow::bail!("Unsupported sing-box outbound type: {}", other),
    };

    if let Some(tls) = &ob.tls {
        // Unknown TLS controls can strengthen authentication (pinning, custom
        // trust, mTLS, ECH, version bounds). Never turn them into weaker TLS.
        anyhow::ensure!(
            tls.extensions.is_empty(),
            "Unsupported sing-box TLS option; security constraints were not discarded"
        );
    }
    let transport = build_singbox_transport(ob)?;
    let mut extra = HashMap::new();
    if ob.network.as_deref() == Some("udp") {
        extra.insert(super::v2ray::UDP_ONLY.into(), "true".into());
    }
    if ob.outbound_type == "vmess" {
        super::v2ray::vmess_framing_options(&mut extra, ob.global_padding, ob.authenticated_length);
    }
    let mut port = ob.server_port;
    if matches!(protocol, ProxyProtocol::Hysteria2 { .. }) {
        if let Some(list) = &ob.server_ports {
            let items = match list {
                serde_json::Value::Array(a) => a.as_slice(),
                v => std::slice::from_ref(v),
            };
            let ports = items
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(|p| p.replace(':', "-"))
                        .context("Invalid Hysteria2 server port range")
                })
                .collect::<Result<Vec<_>>>()?
                .join(",");
            port = ports
                .split(['-', ','])
                .next()
                .context("Missing Hysteria2 port")?
                .parse()?;
            super::v2ray::validate_hysteria2_mport(&ports, port)?;
            extra.insert(super::v2ray::HY2_MPORT.into(), ports);
        }
        for (value, field) in [
            (&ob.hop_interval, super::v2ray::HY2_HOP_INTERVAL),
            (&ob.hop_interval_max, super::v2ray::HY2_HOP_MAX),
        ] {
            if let Some(v) = value {
                extra.insert(field.into(), duration_millis(v)?.to_string());
            }
        }
        for (rate, key) in [
            (ob.up_mbps, super::v2ray::HY2_UP),
            (ob.down_mbps, super::v2ray::HY2_DOWN),
        ] {
            if let Some(rate) = rate {
                extra.insert(
                    key.into(),
                    super::bandwidth::canonical_mbps(&rate.to_string())?,
                );
            }
        }
        super::v2ray::validate_hysteria2_profile(transport.as_ref().and_then(|t| t.tls.as_ref()))?;
    }

    let name = if ob.tag.is_empty() {
        format!("{}-{}:{}", ob.outbound_type, ob.server, ob.server_port)
    } else {
        decode_display_name(&ob.tag)
    };

    Ok(Some(Node {
        name,
        server: ob.server.clone(),
        port,
        protocol,
        transport,
        latency_ms: None,
        tags: vec![],
        extra,
    }))
}

fn build_singbox_transport(ob: &SingBoxOutbound) -> Result<Option<TransportConfig>> {
    if let Some(transport) = &ob.transport {
        anyhow::ensure!(
            matches!(
                transport.transport_type.as_deref(),
                Some("ws" | "httpupgrade")
            ),
            "Unsupported sing-box transport; configuration was not downgraded"
        );
    }
    anyhow::ensure!(
        !ob.tls.as_ref().is_some_and(|tls| tls.disable_sni),
        "Disabling TLS SNI is not supported"
    );
    anyhow::ensure!(
        !ob.multiplex
            .as_ref()
            .and_then(|mux| mux.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        "Multiplex is not supported"
    );
    if let Some(tls) = &ob.tls
        && let Some(reality) = tls.reality.as_ref().filter(|r| r.enabled)
    {
        anyhow::ensure!(
            tls.enabled
                && ob.transport.is_none()
                && matches!(ob.outbound_type.as_str(), "vless" | "trojan"),
            "REALITY requires VLESS or Trojan over raw TCP with TLS enabled"
        );
        let public_key = reality
            .public_key
            .clone()
            .filter(|s| !s.is_empty())
            .context("REALITY public key is required")?;
        return Ok(Some(TransportConfig {
            xhttp: None,
            transport_type: TransportType::Reality,
            tls: None,
            ws: None,
            reality: Some(RealityConfig {
                public_key,
                short_id: reality.short_id.clone().unwrap_or_default(),
                sni: tls.server_name.clone(),
                client_fingerprint: tls
                    .utls
                    .as_ref()
                    .filter(|u| u.enabled != Some(false))
                    .and_then(|u| u.fingerprint.clone()),
                skip_cert_verify: tls.insecure,
            }),
        }));
    }
    let tls_cfg = ob.tls.as_ref();
    let proxy_type = ob.outbound_type.as_str();

    // TUIC and Hysteria2 always need TLS even if not explicitly enabled
    let force_tls = matches!(proxy_type, "tuic" | "hysteria2" | "hy2" | "anytls");
    let has_tls = tls_cfg.map(|t| t.enabled).unwrap_or(false) || force_tls;

    let has_ws = ob
        .transport
        .as_ref()
        .and_then(|t| t.transport_type.as_deref())
        .map(|t| matches!(t, "ws" | "httpupgrade"))
        .unwrap_or(false);

    if !has_tls && !has_ws {
        return Ok(None);
    }

    let tls = if has_tls {
        Some(TlsConfig {
            sni: tls_cfg.and_then(|t| t.server_name.clone()),
            skip_cert_verify: tls_cfg.map(|t| t.insecure).unwrap_or(false),
            alpn: tls_cfg.and_then(|t| t.alpn.clone()),
            fingerprint: tls_cfg.and_then(|t| {
                t.utls
                    .as_ref()
                    .filter(|u| u.enabled != Some(false))
                    .and_then(|u| u.fingerprint.clone())
            }),
        })
    } else {
        None
    };

    let ws = if has_ws {
        ob.transport.as_ref().map(|t| WsConfig {
            path: t.path.clone(),
            host: t
                .host
                .clone()
                .or_else(|| t.headers.as_ref().and_then(|h| h.get("Host").cloned())),
            headers: t.headers.clone(),
        })
    } else {
        None
    };

    let transport_type = match (has_ws, has_tls) {
        (true, _)
            if ob
                .transport
                .as_ref()
                .and_then(|t| t.transport_type.as_deref())
                == Some("httpupgrade") =>
        {
            TransportType::HttpUpgrade
        }
        (true, _) => TransportType::WebSocket,
        (false, true) => TransportType::Tls,
        _ => TransportType::Tcp,
    };

    Ok(Some(TransportConfig {
        xhttp: None,
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
    fn disabled_utls_does_not_activate_a_quic_profile() {
        for enabled in [false, true] {
            let content = serde_json::json!({"outbounds":[{"type":"hysteria2","tag":"synthetic","server":"example.invalid","server_port":443,"password":"synthetic","tls":{"enabled":true,"utls":{"enabled":enabled,"fingerprint":"chrome"}}}]}).to_string();
            let nodes = parse_singbox_config(&content).unwrap();
            assert_eq!(
                nodes[0]
                    .transport
                    .as_ref()
                    .unwrap()
                    .tls
                    .as_ref()
                    .unwrap()
                    .fingerprint
                    .as_deref(),
                enabled.then_some("chrome")
            );
        }
    }

    #[test]
    fn incompatible_quic_profile_is_skipped_before_saving() {
        let content = serde_json::json!({"outbounds":[{"type":"hysteria2","tag":"synthetic","server":"example.invalid","server_port":443,"password":"synthetic","tls":{"enabled":true,"alpn":["hysteria"],"utls":{"enabled":true,"fingerprint":"chrome"}}}]}).to_string();
        assert!(parse_singbox_config(&content).unwrap().is_empty());
    }

    #[test]
    fn unsupported_tls_security_constraints_fail_closed_even_when_insecure() {
        for key in [
            "certificate_public_key_sha256",
            "certificate_sha256",
            "certificate",
            "certificate_path",
            "client_certificate",
            "client_key",
            "min_version",
            "max_version",
            "ech",
        ] {
            let mut tls = serde_json::json!({"enabled":true,"insecure":true});
            tls[key] = serde_json::json!(["synthetic-value"]);
            let value = serde_json::json!({"type":"anytls","server":"vpn.example.invalid","server_port":443,"password":"synthetic-only","tls":tls});
            let result = parse_singbox_outbound(&value.to_string());
            assert!(
                result.is_err(),
                "security constraint {key} must not be dropped"
            );
        }
    }

    #[test]
    fn anytls_idle_options_survive_import_save_and_export() {
        let json = serde_json::json!({"type":"anytls","server":"example.invalid","server_port":443,"password":"local","idle_session_check_interval":"1m2s","idle_session_timeout":"8s","min_idle_session":6});
        let node = parse_singbox_outbound(&json.to_string()).unwrap().unwrap();
        let ProxyProtocol::AnyTls { pool, .. } = &node.protocol else {
            panic!("expected AnyTLS")
        };
        assert_eq!(pool.check_interval_ms, Some(62000));
        assert_eq!(pool.idle_timeout_ms, Some(8000));
        assert_eq!(pool.min_idle, 6);
        let yaml = crate::config::export::nodes_to_clash_yaml(std::slice::from_ref(&node)).unwrap();
        let restored = crate::parse_clash_config(&yaml).unwrap().pop().unwrap();
        let uri = crate::node_to_share_uri(&restored);
        let from_uri = super::super::v2ray::parse_anytls_uri(&uri).unwrap();
        assert_eq!(
            serde_json::to_value(node.protocol).unwrap(),
            serde_json::to_value(from_uri.protocol).unwrap()
        );
    }
    #[test]
    fn tuic_relay_modes_and_heartbeat_validate_on_import() {
        assert_eq!(duration_millis("1m0.125s").unwrap(), 60125);
        assert_eq!(duration_millis("250us").unwrap(), 1);
        for invalid in ["", "30", "1hour", "-2s", "1.2.3s"] {
            assert!(duration_millis(invalid).is_err());
        }
        let mut value = serde_json::json!({"type":"tuic","server":"example.invalid","server_port":443,"uuid":"00000000-0000-4000-8000-000000000001","password":"local","udp_relay_mode":"quic","udp_over_stream":true});
        assert!(parse_singbox_outbound(&value.to_string()).is_err());
        value["udp_over_stream"] = false.into();
        value["udp_relay_mode"] = "invalid".into();
        assert!(parse_singbox_outbound(&value.to_string()).is_err());
    }

    #[test]
    fn test_parse_singbox_tuic() {
        let json = r#"{
            "outbounds": [{
                "type": "tuic",
                "tag": "MY-TUIC",
                "server": "tuic.example.com",
                "server_port": 60000,
                "congestion_control": "bbr",
                "udp_relay_mode": "native",
                "zero_rtt_handshake": true,
                "heartbeat": "10s",
                "tls": {
                    "enabled": true,
                    "insecure": true,
                    "alpn": ["h3"],
                    "server_name": "www.example.com"
                },
                "uuid": "00000000-0000-0000-0000-000000000001",
                "password": "00000000-0000-0000-0000-000000000001"
            }]
        }"#;

        let nodes = parse_singbox_config(json).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "MY-TUIC");
        assert_eq!(nodes[0].server, "tuic.example.com");
        assert_eq!(nodes[0].port, 60000);

        // Check TLS config preserved
        let transport = nodes[0].transport.as_ref().unwrap();
        let tls = transport.tls.as_ref().unwrap();
        assert_eq!(tls.sni.as_deref(), Some("www.example.com"));
        assert!(tls.skip_cert_verify);
        assert_eq!(tls.alpn.as_ref().unwrap(), &vec!["h3".to_string()]);

        match &nodes[0].protocol {
            ProxyProtocol::Tuic {
                uuid,
                password,
                congestion_control,
                ..
            } => {
                assert_eq!(uuid, "00000000-0000-0000-0000-000000000001");
                assert_eq!(password, "00000000-0000-0000-0000-000000000001");
                assert_eq!(congestion_control, "bbr");
            }
            _ => panic!("Expected TUIC"),
        }
    }

    #[test]
    fn test_parse_singbox_hysteria2() {
        let json = r#"{
            "outbounds": [{
                "type": "hysteria2",
                "tag": "HY2-SG",
                "up_mbps": 17,
                "down_mbps": 83,
                "server": "1.2.3.4",
                "server_port": 443,
                "password": "secret",
                "tls": {
                    "enabled": true,
                    "insecure": false,
                    "server_name": "example.com",
                    "alpn": ["h3"]
                }
            }]
        }"#;

        let nodes = parse_singbox_config(json).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "HY2-SG");
        assert_eq!(nodes[0].extra[super::super::v2ray::HY2_UP], "17");
        assert_eq!(nodes[0].extra[super::super::v2ray::HY2_DOWN], "83");
        let tls = nodes[0].transport.as_ref().unwrap().tls.as_ref().unwrap();
        assert_eq!(tls.sni.as_deref(), Some("example.com"));
        assert!(!tls.skip_cert_verify);
    }

    #[test]
    fn test_skip_non_proxy_outbounds() {
        let json = r#"{
            "outbounds": [
                {"type": "direct", "tag": "direct"},
                {"type": "block", "tag": "block"},
                {"type": "dns", "tag": "dns"},
                {"type": "selector", "tag": "select"}
            ]
        }"#;
        let nodes = parse_singbox_config(json).unwrap();
        assert_eq!(nodes.len(), 0);
    }

    #[test]
    fn test_is_singbox_config() {
        assert!(is_singbox_config(r#"{"outbounds":[{"server_port":443}]}"#));
        assert!(!is_singbox_config(r#"{"inbounds":[]}"#));
        assert!(!is_singbox_config("proxies:"));
    }

    #[test]
    fn test_parse_single_outbound() {
        let json = r#"{
            "type": "tuic",
            "tag": "MY-1",
            "server": "1.2.3.4",
            "server_port": 443,
            "uuid": "550e8400-e29b-41d4-a716-446655440000",
            "password": "pass",
            "tls": {"enabled": true, "insecure": true, "server_name": "sni.test.com"}
        }"#;
        let node = parse_singbox_outbound(json).unwrap().unwrap();
        assert_eq!(node.name, "MY-1");
        assert!(
            matches!(&node.protocol, ProxyProtocol::Tuic { congestion_control, .. } if congestion_control == "cubic")
        );
        let tls = node.transport.as_ref().unwrap().tls.as_ref().unwrap();
        assert_eq!(tls.sni.as_deref(), Some("sni.test.com"));
    }
}

#[cfg(test)]
mod capability_regressions {
    use super::*;
    #[test]
    fn reality_is_preserved_and_unsupported_transports_are_rejected() {
        let raw = serde_json::json!({"type":"vless","server":"vpn.example.invalid","server_port":443,"uuid":"00000000-0000-0000-0000-000000000001","tls":{"enabled":true,"server_name":"tls.example.invalid","reality":{"enabled":true,"public_key":"synthetic-public-key","short_id":"aabb"}}});
        let node = parse_singbox_outbound(&raw.to_string()).unwrap().unwrap();
        let transport = node.transport.unwrap();
        assert!(matches!(transport.transport_type, TransportType::Reality));
        assert_eq!(transport.reality.unwrap().short_id, "aabb");
        let mut raw = raw;
        raw["transport"] = serde_json::json!({"type":"grpc"});
        assert!(parse_singbox_outbound(&raw.to_string()).is_err());
    }
}

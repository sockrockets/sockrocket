//! Node de-duplication by credential fingerprint.
//!
//! Two entries are considered duplicates when they describe the same server
//! with the same credentials — protocol, address, port, the protocol's key
//! credential fields (password / UUID / cipher), and the transport summary
//! (transport type + WS path/host + Reality public key + shadow-tls host).
//! Display name, latency, and tags are intentionally excluded: a renamed
//! copy of the same server is still the same server. The first occurrence
//! wins.

use std::collections::HashSet;

use super::model::*;

/// Stable fingerprint for a node, covering protocol + address + port +
/// credentials + transport identity (but not the display name or other
/// cosmetic metadata).
pub fn node_fingerprint(node: &Node) -> String {
    let credentials = match &node.protocol {
        ProxyProtocol::Shadowsocks {
            cipher,
            password,
            shadow_tls,
            ..
        } => {
            // The shadow-tls host is part of the connection identity: an
            // "SS + shadow-tls" node and a plain SS node on the same
            // server/port/cipher/password are *not* interchangeable.
            let st_host = shadow_tls
                .as_ref()
                .map(|st| st.host.to_ascii_lowercase())
                .unwrap_or_default();
            format!(
                "ss|{}|{}|st:{}",
                cipher.to_ascii_lowercase(),
                password,
                st_host
            )
        }
        ProxyProtocol::VMess {
            uuid,
            alter_id,
            cipher,
            ..
        } => format!(
            "vmess|{}|{}|{}",
            uuid.to_ascii_lowercase(),
            alter_id,
            cipher.to_ascii_lowercase()
        ),
        ProxyProtocol::VLess { uuid, flow, .. } => format!(
            "vless|{}|{}",
            uuid.to_ascii_lowercase(),
            flow.as_deref().unwrap_or("")
        ),
        ProxyProtocol::Tuic {
            uuid,
            password,
            congestion_control,
            ..
        } => format!(
            "tuic|{}|{}|{}",
            uuid.to_ascii_lowercase(),
            password,
            congestion_control
        ),
        ProxyProtocol::Trojan { password, .. } => format!("trojan|{}", password),
        ProxyProtocol::Hysteria2 { password, .. } => format!("hy2|{}", password),
    };

    format!(
        "{}|{}|{}|{}",
        node.server.to_ascii_lowercase(),
        node.port,
        credentials,
        transport_summary(node.transport.as_ref())
    )
}

/// One-line summary of how the node is reached: transport type plus the
/// fields that distinguish two nodes sharing the same credentials
/// (WS path/host, Reality public key).
fn transport_summary(transport: Option<&TransportConfig>) -> String {
    let Some(tc) = transport else {
        return "tcp".to_string();
    };
    let ttype = match tc.transport_type {
        TransportType::Tcp => "tcp",
        TransportType::Tls => "tls",
        TransportType::WebSocket => "ws",
        TransportType::Quic => "quic",
        TransportType::Reality => "reality",
    };
    let mut summary = ttype.to_string();
    if let Some(ws) = &tc.ws {
        summary.push_str(&format!(
            "|{}|{}",
            ws.path.as_deref().unwrap_or(""),
            ws.host.as_deref().unwrap_or("").to_ascii_lowercase()
        ));
    }
    if let Some(reality) = &tc.reality {
        summary.push_str(&format!("|pbk:{}", reality.public_key));
    }
    summary
}

/// Remove duplicate nodes in-place, keeping the first occurrence of each
/// fingerprint. Returns the number of nodes removed.
pub fn dedup_nodes(nodes: &mut Vec<Node>) -> usize {
    let mut seen = HashSet::new();
    let before = nodes.len();
    nodes.retain(|node| seen.insert(node_fingerprint(node)));
    before - nodes.len()
}

/// Make display names unique within the list by appending " #2", " #3", …
/// to later duplicates.
///
/// Names are used as identity keys in several places (proxy-group membership,
/// selection restore after a subscription refresh), and subscriptions
/// routinely reuse the same display names, so collisions must be
/// disambiguated after every merge. The first occurrence keeps the base
/// name. Returns `true` if any name was changed.
pub fn uniquify_node_names(nodes: &mut [Node]) -> bool {
    let mut seen: HashSet<String> = HashSet::new();
    let mut changed = false;
    for node in nodes {
        if seen.insert(node.name.clone()) {
            continue;
        }
        let base = std::mem::take(&mut node.name);
        let mut n = 2u32;
        loop {
            let candidate = format!("{} #{}", base, n);
            if seen.insert(candidate.clone()) {
                node.name = candidate;
                break;
            }
            n += 1;
        }
        changed = true;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ss(name: &str, server: &str, port: u16, password: &str) -> Node {
        Node {
            name: name.to_string(),
            server: server.to_string(),
            port,
            protocol: ProxyProtocol::Shadowsocks {
                cipher: "aes-256-gcm".to_string(),
                password: password.to_string(),
                udp: true,
                shadow_tls: None,
            },
            transport: None,
            latency_ms: None,
            tags: vec![],
            extra: Default::default(),
        }
    }

    fn vmess(name: &str, server: &str, port: u16, uuid: &str) -> Node {
        Node {
            name: name.to_string(),
            server: server.to_string(),
            port,
            protocol: ProxyProtocol::VMess {
                uuid: uuid.to_string(),
                alter_id: 0,
                cipher: "auto".to_string(),
                udp: false,
            },
            transport: None,
            latency_ms: None,
            tags: vec![],
            extra: Default::default(),
        }
    }

    fn trojan(name: &str, server: &str, port: u16, password: &str) -> Node {
        Node {
            name: name.to_string(),
            server: server.to_string(),
            port,
            protocol: ProxyProtocol::Trojan {
                password: password.to_string(),
                udp: true,
            },
            transport: None,
            latency_ms: None,
            tags: vec![],
            extra: Default::default(),
        }
    }

    #[test]
    fn uniquify_appends_numeric_suffix_to_later_duplicates() {
        let mut nodes = vec![
            ss("Tokyo", "1.1.1.1", 8388, "pw"),
            ss("Tokyo", "2.2.2.2", 8388, "pw"),
            ss("Tokyo", "3.3.3.3", 8388, "pw"),
            ss("Other", "4.4.4.4", 8388, "pw"),
        ];
        assert!(uniquify_node_names(&mut nodes));
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["Tokyo", "Tokyo #2", "Tokyo #3", "Other"]);
        // Idempotent: a second pass changes nothing.
        assert!(!uniquify_node_names(&mut nodes));
    }

    #[test]
    fn removes_duplicates_keeping_first_occurrence() {
        let mut nodes = vec![
            ss("HK-A", "1.1.1.1", 8388, "pw"),
            ss("HK-B (renamed copy)", "1.1.1.1", 8388, "pw"),
            ss("HK-C other port", "1.1.1.1", 8389, "pw"),
            ss("HK-D other password", "1.1.1.1", 8388, "different"),
        ];
        let removed = dedup_nodes(&mut nodes);
        assert_eq!(removed, 1);
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].name, "HK-A", "first occurrence wins");
        assert_eq!(nodes[1].name, "HK-C other port");
        assert_eq!(nodes[2].name, "HK-D other password");
    }

    #[test]
    fn different_credentials_are_not_duplicates() {
        let mut nodes = vec![
            vmess("a", "2.2.2.2", 443, "uuid-1"),
            vmess("b", "2.2.2.2", 443, "uuid-2"),
        ];
        assert_eq!(dedup_nodes(&mut nodes), 0);
        assert_eq!(nodes.len(), 2);
    }

    #[test]
    fn different_protocols_are_not_duplicates() {
        // Same server/port/password but different protocols must be kept.
        let mut nodes = vec![
            trojan("trojan", "3.3.3.3", 443, "shared-pw"),
            Node {
                protocol: ProxyProtocol::Hysteria2 {
                    password: "shared-pw".to_string(),
                    udp: true,
                },
                ..trojan("hy2", "3.3.3.3", 443, "shared-pw")
            },
        ];
        assert_eq!(dedup_nodes(&mut nodes), 0);
        assert_eq!(nodes.len(), 2);
    }

    #[test]
    fn fingerprint_ignores_name_latency_and_tags() {
        let a = ss("name one", "4.4.4.4", 8388, "pw");
        let mut b = ss("name two", "4.4.4.4", 8388, "pw");
        b.latency_ms = Some(42);
        b.tags = vec!["tag".to_string()];
        assert_eq!(node_fingerprint(&a), node_fingerprint(&b));
    }

    #[test]
    fn fingerprint_is_case_insensitive_for_host_and_uuid() {
        let a = vmess("a", "EXAMPLE.com", 443, "ABC-uuid");
        let b = vmess("b", "example.COM", 443, "abc-uuid");
        assert_eq!(node_fingerprint(&a), node_fingerprint(&b));
    }

    #[test]
    fn shadow_tls_nodes_are_not_merged_with_plain_ss() {
        let plain = ss("plain", "1.1.1.1", 8388, "pw");
        let mut with_st = ss("st", "1.1.1.1", 8388, "pw");
        with_st.protocol = ProxyProtocol::Shadowsocks {
            cipher: "aes-256-gcm".to_string(),
            password: "pw".to_string(),
            udp: true,
            shadow_tls: Some(ShadowTlsConfig {
                host: "apple.com".to_string(),
                password: "st-pw".to_string(),
                skip_cert_verify: false,
            }),
        };
        assert_ne!(node_fingerprint(&plain), node_fingerprint(&with_st));
        let mut nodes = vec![plain, with_st];
        assert_eq!(
            dedup_nodes(&mut nodes),
            0,
            "SS+shadow-tls must not merge with bare SS"
        );
    }

    #[test]
    fn vmess_ws_and_tcp_transports_are_not_merged() {
        let tcp = vmess("tcp", "2.2.2.2", 443, "uuid-1");
        let mut ws = vmess("ws", "2.2.2.2", 443, "uuid-1");
        ws.transport = Some(TransportConfig {
            transport_type: TransportType::WebSocket,
            tls: None,
            ws: Some(WsConfig {
                path: Some("/ray".to_string()),
                host: Some("cdn.example.com".to_string()),
                headers: None,
            }),
            reality: None,
        });
        assert_ne!(node_fingerprint(&tcp), node_fingerprint(&ws));
        let mut nodes = vec![tcp, ws];
        assert_eq!(
            dedup_nodes(&mut nodes),
            0,
            "VMess WS and TCP must not merge"
        );
    }

    #[test]
    fn ws_path_and_reality_key_distinguish_fingerprints() {
        let mut a = vmess("a", "2.2.2.2", 443, "uuid-1");
        a.transport = Some(TransportConfig {
            transport_type: TransportType::WebSocket,
            tls: None,
            ws: Some(WsConfig {
                path: Some("/a".to_string()),
                host: None,
                headers: None,
            }),
            reality: None,
        });
        let mut b = a.clone();
        b.transport.as_mut().unwrap().ws.as_mut().unwrap().path = Some("/b".to_string());
        assert_ne!(node_fingerprint(&a), node_fingerprint(&b));

        let mut r1 = vmess("r1", "2.2.2.2", 443, "uuid-1");
        r1.transport = Some(TransportConfig {
            transport_type: TransportType::Reality,
            tls: None,
            ws: None,
            reality: Some(RealityConfig {
                public_key: "key-one".to_string(),
                short_id: String::new(),
                sni: None,
                client_fingerprint: None,
                skip_cert_verify: false,
            }),
        });
        let mut r2 = r1.clone();
        r2.transport
            .as_mut()
            .unwrap()
            .reality
            .as_mut()
            .unwrap()
            .public_key = "key-two".to_string();
        assert_ne!(node_fingerprint(&r1), node_fingerprint(&r2));
    }
}

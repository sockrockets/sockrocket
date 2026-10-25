//! Node de-duplication by credential fingerprint.
//!
//! Two entries are considered duplicates when they describe the same server
//! with the same credentials — protocol, address, port, the protocol's key
//! credential fields (password / UUID / cipher), and the transport summary
//! (transport type + WS path/host + Reality public key + shadow-tls host).
//! Display name, latency, and tags are intentionally excluded: a renamed
//! copy of the same server is still the same server. The first occurrence
//! wins. Importing a subscription is the exception: the provider's own
//! remarks stay, even when several of them share one endpoint.

use std::collections::{BTreeSet, HashMap, HashSet};

use super::model::*;

/// `Node.extra` key that records which subscription URL produced the node.
pub const SUB_URL_EXTRA_KEY: &str = "sub_url";

const SOURCES_KEY: &str = "subscription_sources";
const MANUAL_KEY: &str = "manual_copy";

/// All source memberships, including the legacy single-source format.
pub fn subscription_sources(node: &Node) -> BTreeSet<String> {
    let mut sources: BTreeSet<String> = node
        .extra
        .get(SOURCES_KEY)
        .and_then(|value| serde_json::from_str(value).ok())
        .unwrap_or_default();
    if let Some(url) = node.extra.get(SUB_URL_EXTRA_KEY) {
        sources.insert(url.clone());
    }
    sources
}
fn set_sources(node: &mut Node, sources: &BTreeSet<String>) {
    node.extra.remove(SUB_URL_EXTRA_KEY);
    node.extra.remove(SOURCES_KEY);
    if let Some(first) = sources.first() {
        node.extra.insert(SUB_URL_EXTRA_KEY.into(), first.clone());
        node.extra.insert(
            SOURCES_KEY.into(),
            serde_json::to_string(sources).expect("string sources"),
        );
    }
}
fn is_manual(node: &Node) -> bool {
    subscription_sources(node).is_empty()
        || node
            .extra
            .get(MANUAL_KEY)
            .is_some_and(|value| value == "true")
}
/// Stamp fresh results with exactly their producing source.
pub fn tag_subscription_nodes(nodes: &mut [Node], url: &str) {
    for node in nodes {
        node.extra.remove(MANUAL_KEY);
        set_sources(node, &BTreeSet::from([url.to_owned()]));
    }
}
fn detach_sources(nodes: &mut Vec<Node>, removed: &HashSet<String>) {
    nodes.retain_mut(|node| {
        let manual = is_manual(node);
        let mut sources = subscription_sources(node);
        sources.retain(|source| !removed.contains(source));
        set_sources(node, &sources);
        if manual {
            node.extra.insert(MANUAL_KEY.into(), "true".into());
        }
        manual || !sources.is_empty()
    });
}

/// Replace nodes belonging to subscriptions that just refreshed.
///
/// Fresh nodes win fingerprint ties, so an updated password replaces the
/// saved copy. Hand-added nodes (no subscription tag) and nodes from a
/// subscription that failed to refresh stay. Returns whether anything was
/// applied. Also records `last_updated`, rotates the refresh revision so older
/// downloads are rejected, and keeps `active_node` pointing at the same server
/// when it still exists.
pub fn apply_subscription_refresh(
    config: &mut AppConfig,
    refreshed: Vec<(String, Vec<Node>)>,
) -> bool {
    // An empty/failed source is not an instruction to delete saved nodes.
    let refreshed: Vec<_> = refreshed
        .into_iter()
        .filter(|(url, nodes)| {
            !nodes.is_empty() && config.subscriptions.iter().any(|source| source.url == *url)
        })
        .collect();
    if refreshed.is_empty() {
        return false;
    }
    let active = config.active_node.and_then(|index| config.nodes.get(index));
    let active_fp = active.map(node_fingerprint);
    let active_name = active.map(|node| node.name.clone());
    let urls: HashSet<String> = refreshed.iter().map(|(url, _)| url.clone()).collect();
    let mut kept = std::mem::take(&mut config.nodes);
    detach_sources(&mut kept, &urls);
    let mut merged = Vec::new();
    for (url, mut nodes) in refreshed {
        tag_subscription_nodes(&mut nodes, &url);
        merged.append(&mut nodes);
    }
    let fresh_len = merged.len();
    merged.append(&mut kept);
    // Distinct remarks in this download stay. A previous copy of the same
    // endpoint, or a hand-added duplicate, still folds into the fresh list.
    dedup_subscription_import(&mut merged, fresh_len);
    config.active_node = active_fp.and_then(|fingerprint| {
        let named = active_name.as_ref().and_then(|name| {
            merged
                .iter()
                .position(|node| node.name == *name && node_fingerprint(node) == fingerprint)
        });
        named.or_else(|| {
            merged
                .iter()
                .position(|node| node_fingerprint(node) == fingerprint)
        })
    });
    uniquify_node_names(&mut merged);
    config.nodes = merged;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    for subscription in &mut config.subscriptions {
        if urls.contains(&subscription.url) {
            subscription.last_updated = Some(now);
            subscription.refresh_revision = super::storage::new_subscription_revision();
        }
    }
    true
}

/// Drop one subscription and the nodes it produced. Untagged nodes stay.
pub fn remove_subscription_nodes(config: &mut AppConfig, url: &str) {
    let active_fp = config
        .active_node
        .and_then(|index| config.nodes.get(index))
        .map(node_fingerprint);
    detach_sources(&mut config.nodes, &HashSet::from([url.to_owned()]));
    config
        .subscriptions
        .retain(|subscription| subscription.url != url);
    config.active_node = active_fp.and_then(|fingerprint| {
        config
            .nodes
            .iter()
            .position(|node| node_fingerprint(node) == fingerprint)
    });
}

/// Opaque id safe to show in the UI and in saved rule targets.
/// The raw fingerprint includes protocol credentials and stays in the config file.
pub fn node_public_id(fingerprint: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(fingerprint.as_bytes()))
}

/// Stable fingerprint for a node, covering protocol + address + port +
/// credentials + transport identity (but not the display name or other
/// cosmetic metadata).
pub fn node_fingerprint(node: &Node) -> String {
    let credentials = match &node.protocol {
        ProxyProtocol::Wireguard { config } => format!(
            "wireguard|{}",
            serde_json::to_string(config).unwrap_or_default()
        ),
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
            "tuic|{}|{}|{}|{}|{}",
            uuid.to_ascii_lowercase(),
            password,
            congestion_control,
            node.extra
                .get(super::v2ray::TUIC_CERT_PIN)
                .map(String::as_str)
                .unwrap_or(""),
            serde_json::to_string(&node.transport).unwrap_or_default()
        ),
        ProxyProtocol::Trojan { password, .. } => format!("trojan|{}", password),
        ProxyProtocol::Hysteria2 { password, .. } => format!(
            "hy2|{}|{}|{}|{}|{}|{}|{}",
            password,
            node.extra
                .get(super::v2ray::TLS_CERT_PIN)
                .map(String::as_str)
                .unwrap_or(""),
            node.extra
                .get(super::v2ray::HY2_UP)
                .map(String::as_str)
                .unwrap_or(""),
            node.extra
                .get(super::v2ray::HY2_DOWN)
                .map(String::as_str)
                .unwrap_or(""),
            node.extra
                .get(super::v2ray::HY2_FAST_OPEN)
                .map(String::as_str)
                .unwrap_or(""),
            node.extra
                .get(super::v2ray::HY2_MPORT)
                .map(String::as_str)
                .unwrap_or(""),
            serde_json::to_string(&node.transport).unwrap_or_default()
        ),
        ProxyProtocol::AnyTls { password, .. } => format!(
            "anytls|{}|{}|{}",
            password,
            node.extra
                .get(super::v2ray::TLS_CERT_PIN)
                .map(String::as_str)
                .unwrap_or(""),
            serde_json::to_string(&node.transport).unwrap_or_default()
        ),
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

fn merge_duplicate(retained: &mut Node, incoming: &Node) {
    let manual = is_manual(retained) || is_manual(incoming);
    let mut sources = subscription_sources(retained);
    sources.extend(subscription_sources(incoming));
    set_sources(retained, &sources);
    if manual {
        retained.extra.insert(MANUAL_KEY.into(), "true".into());
    }
}

/// Remove duplicate nodes in-place, keeping the first occurrence of each
/// fingerprint. Display name is not part of that identity: a renamed copy of
/// the same server is still the same server. Returns the number removed.
pub fn dedup_nodes(nodes: &mut Vec<Node>) -> usize {
    let before = nodes.len();
    let mut seen = HashMap::<String, usize>::new();
    let mut merged: Vec<Node> = Vec::with_capacity(before);
    for node in std::mem::take(nodes) {
        let fingerprint = node_fingerprint(&node);
        if let Some(&index) = seen.get(&fingerprint) {
            merge_duplicate(&mut merged[index], &node);
        } else {
            seen.insert(fingerprint, merged.len());
            merged.push(node);
        }
    }
    *nodes = merged;
    before - nodes.len()
}

/// Keep every distinct remark in a freshly imported subscription.
///
/// `nodes[..fresh_len]` is the new list. Later entries are what remained
/// after the previous copy of those subscriptions was detached. The same
/// endpoint under a different name stays. A leftover with the same endpoint
/// and no other subscription folds into the first fresh remark, so refreshing
/// cannot leave the previous names behind.
fn dedup_subscription_import(nodes: &mut Vec<Node>, fresh_len: usize) -> usize {
    let before = nodes.len();
    let fresh_len = fresh_len.min(before);
    let mut seen_named = HashMap::<String, usize>::new();
    let mut fresh_by_fp = HashMap::<String, usize>::new();
    let mut merged: Vec<Node> = Vec::with_capacity(before);
    let mut incoming = std::mem::take(nodes);
    for node in incoming.drain(..fresh_len) {
        let fingerprint = node_fingerprint(&node);
        let key = format!("{fingerprint}\n{}", node.name);
        if let Some(&index) = seen_named.get(&key) {
            merge_duplicate(&mut merged[index], &node);
        } else {
            fresh_by_fp.entry(fingerprint).or_insert(merged.len());
            seen_named.insert(key, merged.len());
            merged.push(node);
        }
    }
    for node in incoming {
        let fingerprint = node_fingerprint(&node);
        let key = format!("{fingerprint}\n{}", node.name);
        if let Some(&index) = seen_named.get(&key) {
            merge_duplicate(&mut merged[index], &node);
        } else if fresh_by_fp.contains_key(&fingerprint)
            && (is_manual(&node) || subscription_sources(&node).is_empty())
        {
            let index = fresh_by_fp[&fingerprint];
            merge_duplicate(&mut merged[index], &node);
        } else {
            seen_named.insert(key, merged.len());
            merged.push(node);
        }
    }
    *nodes = merged;
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

    #[test]
    fn refresh_keeps_manual_nodes_and_replaces_that_subscription() {
        let mut manual = ss("manual", "192.0.2.9", 1, "manual");
        manual.extra.insert("note".into(), "kept".into());
        let mut old = ss("old", "192.0.2.1", 443, "old-pw");
        old.extra
            .insert(SUB_URL_EXTRA_KEY.into(), "https://example.com/a".into());
        let mut other = ss("other", "192.0.2.2", 443, "other");
        other
            .extra
            .insert(SUB_URL_EXTRA_KEY.into(), "https://example.com/b".into());
        let duplicate = ss("stale-copy", "192.0.2.3", 8443, "new-pw");
        let mut config = AppConfig {
            nodes: vec![manual, old, other, duplicate],
            active_node: Some(3),
            subscriptions: vec![
                Subscription {
                    name: "A".into(),
                    url: "https://example.com/a".into(),
                    format: "auto".into(),
                    last_updated: None,
                    refresh_interval_hours: 24,
                    refresh_revision: String::new(),
                },
                Subscription {
                    name: "B".into(),
                    url: "https://example.com/b".into(),
                    format: "auto".into(),
                    last_updated: None,
                    refresh_interval_hours: 24,
                    refresh_revision: String::new(),
                },
            ],
            ..AppConfig::default()
        };
        let fresh = ss("fresh", "192.0.2.3", 8443, "new-pw");
        assert!(apply_subscription_refresh(
            &mut config,
            vec![("https://example.com/a".into(), vec![fresh])]
        ));
        assert_eq!(config.nodes.len(), 3);
        assert_eq!(config.nodes[0].name, "fresh");
        assert_eq!(
            config.nodes[0]
                .extra
                .get(SUB_URL_EXTRA_KEY)
                .map(String::as_str),
            Some("https://example.com/a")
        );
        assert!(config.nodes.iter().any(|node| node.name == "manual"));
        assert!(config.nodes.iter().any(|node| node.name == "other"));
        assert!(config.nodes.iter().all(|node| node.name != "old"));
        assert!(config.nodes.iter().all(|node| node.name != "stale-copy"));
        let active = config.active_node.and_then(|index| config.nodes.get(index));
        assert_eq!(active.map(|node| node.name.as_str()), Some("fresh"));
        assert!(config.subscriptions[0].last_updated.is_some());
        assert!(config.subscriptions[1].last_updated.is_none());

        remove_subscription_nodes(&mut config, "https://example.com/a");
        assert!(
            config.nodes.iter().any(|node| node.name == "fresh"),
            "manual duplicate survives source deletion"
        );
        assert!(config.nodes.iter().any(|node| node.name == "manual"));
        assert_eq!(config.subscriptions.len(), 1);
        assert_eq!(config.subscriptions[0].url, "https://example.com/b");
    }

    #[test]
    fn refresh_keeps_distinct_remarks_for_one_endpoint() {
        let mut config = AppConfig {
            subscriptions: vec![Subscription {
                name: "A".into(),
                url: "https://example.com/a".into(),
                format: "auto".into(),
                last_updated: None,
                refresh_interval_hours: 24,
                refresh_revision: String::new(),
            }],
            ..AppConfig::default()
        };
        let fresh = vec![
            ss("剩余流量", "192.0.2.8", 443, "same"),
            ss("套餐到期", "192.0.2.8", 443, "same"),
            ss("东京01", "192.0.2.8", 443, "same"),
            ss("新加坡", "192.0.2.9", 443, "hy2"),
            ss("新加坡2", "192.0.2.9", 443, "hy2"),
        ];
        assert!(apply_subscription_refresh(
            &mut config,
            vec![("https://example.com/a".into(), fresh)]
        ));
        let names: Vec<&str> = config.nodes.iter().map(|node| node.name.as_str()).collect();
        assert_eq!(
            names,
            ["剩余流量", "套餐到期", "东京01", "新加坡", "新加坡2"]
        );

        let again = vec![
            ss("剩余流量", "192.0.2.8", 443, "same"),
            ss("套餐到期", "192.0.2.8", 443, "same"),
            ss("东京01", "192.0.2.8", 443, "same"),
            ss("新加坡", "192.0.2.9", 443, "hy2"),
            ss("新加坡2", "192.0.2.9", 443, "hy2"),
        ];
        assert!(apply_subscription_refresh(
            &mut config,
            vec![("https://example.com/a".into(), again)]
        ));
        assert_eq!(
            config.nodes.len(),
            5,
            "a second refresh must not double remarks"
        );
    }
}

#[cfg(test)]
mod provenance_review {
    use super::*;
    fn node() -> Node {
        Node {
            name: "synthetic".into(),
            server: "192.0.2.1".into(),
            port: 443,
            protocol: ProxyProtocol::Trojan {
                password: "test-only".into(),
                udp: true,
            },
            transport: None,
            latency_ms: None,
            tags: vec![],
            extra: Default::default(),
        }
    }
    fn source(url: &str) -> Subscription {
        Subscription {
            name: "synthetic".into(),
            url: url.into(),
            format: "auto".into(),
            last_updated: None,
            refresh_interval_hours: 24,
            refresh_revision: String::new(),
        }
    }
    #[test]
    fn memberships_survive_dedup_and_independent_source_removal() {
        let mut config = AppConfig {
            subscriptions: vec![
                source("https://example.org/a"),
                source("https://example.org/b"),
            ],
            ..Default::default()
        };
        let batches = vec![
            (config.subscriptions[0].url.clone(), vec![node()]),
            (config.subscriptions[1].url.clone(), vec![node()]),
        ];
        assert!(apply_subscription_refresh(&mut config, batches));
        assert_eq!(config.nodes.len(), 1);
        assert_eq!(subscription_sources(&config.nodes[0]).len(), 2);
        remove_subscription_nodes(&mut config, "https://example.org/a");
        assert_eq!(config.nodes.len(), 1);
        remove_subscription_nodes(&mut config, "https://example.org/b");
        assert!(config.nodes.is_empty());
    }
    #[test]
    fn results_for_a_deleted_source_are_discarded() {
        let mut config = AppConfig::default();
        assert!(!apply_subscription_refresh(
            &mut config,
            vec![("https://example.org/deleted".into(), vec![node()])]
        ));
        assert!(config.nodes.is_empty());
    }
}

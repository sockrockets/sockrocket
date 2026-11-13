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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use super::model::*;

/// `Node.extra` key that records which subscription URL produced the node.
pub const SUB_URL_EXTRA_KEY: &str = "sub_url";

const SOURCES_KEY: &str = "subscription_sources";
const NAMES_KEY: &str = "subscription_names";
const MANUAL_KEY: &str = "manual_copy";

/// All source memberships, including the legacy single-source format.
pub fn subscription_sources(node: &Node) -> BTreeSet<String> {
    let mut sources: BTreeSet<String> = node
        .extra
        .get(SOURCES_KEY)
        .and_then(|value| serde_json::from_slice(value.as_bytes()).ok())
        .unwrap_or_default();
    if let Some(url) = node.extra.get(SUB_URL_EXTRA_KEY) {
        sources.insert(url.clone());
    }
    sources
}
// Provider remarks survive display-name suffixes and configuration round trips.
fn subscription_names(node: &Node) -> BTreeMap<String, String> {
    let mut names: BTreeMap<String, String> = node
        .extra
        .get(NAMES_KEY)
        .and_then(|value| serde_json::from_slice(value.as_bytes()).ok())
        .unwrap_or_default();
    let sources = subscription_sources(node);
    names.retain(|source, _| sources.contains(source));
    for source in sources {
        names.entry(source).or_insert_with(|| node.name.clone());
    }
    names
}
fn set_names(node: &mut Node, names: &BTreeMap<String, String>) {
    if names.is_empty() {
        node.extra.remove(NAMES_KEY);
    } else {
        node.extra.insert(
            NAMES_KEY.into(),
            serde_json::to_string(names).expect("string names"),
        );
    }
}
// subscription_names normalizes legacy metadata and gives every source a remark.
fn set_membership(node: &mut Node, names: &BTreeMap<String, String>) {
    set_names(node, names);
    if let Some((first, _)) = names.first_key_value() {
        node.extra.insert(SUB_URL_EXTRA_KEY.into(), first.clone());
        node.extra.insert(
            SOURCES_KEY.into(),
            serde_json::to_string(&names.keys().collect::<Vec<_>>()).expect("string sources"),
        );
    } else {
        node.extra.remove(SUB_URL_EXTRA_KEY);
        node.extra.remove(SOURCES_KEY);
    }
}
fn has_manual_copy(node: &Node) -> bool {
    node.extra
        .get(MANUAL_KEY)
        .is_some_and(|value| value == "true")
}
/// Stamp fresh results with exactly their producing source.
pub fn tag_subscription_nodes(nodes: &mut [Node], url: &str) {
    for node in nodes {
        let name = subscription_names(node)
            .remove(url)
            .unwrap_or_else(|| node.name.clone());
        node.extra.remove(MANUAL_KEY);
        set_membership(node, &BTreeMap::from([(url.to_owned(), name)]));
    }
}
fn detach_sources(nodes: &mut Vec<Node>, removed: &HashSet<String>) {
    nodes.retain_mut(|node| {
        let mut names = subscription_names(node);
        let manual = names.is_empty() || has_manual_copy(node);
        names.retain(|source, _| !removed.contains(source));
        set_membership(node, &names);
        if manual {
            node.extra.insert(MANUAL_KEY.into(), "true".into());
        }
        manual || !names.is_empty()
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
    refresh_with_dns_migrations(config, refreshed).is_some()
}

/// Return changed fingerprints so persisted and live selections can follow a
/// subscription that changes the resolver policy for an existing endpoint.
pub fn refresh_with_dns_migrations(
    config: &mut AppConfig,
    refreshed: Vec<(String, Vec<Node>)>,
) -> Option<HashMap<String, String>> {
    // An empty/failed source is not an instruction to delete saved nodes.
    let refreshed: Vec<_> = refreshed
        .into_iter()
        .filter(|(url, nodes)| {
            !nodes.is_empty() && config.subscriptions.iter().any(|source| source.url == *url)
        })
        .collect();
    if refreshed.is_empty() {
        return None;
    }
    let active = config.active_node.and_then(|index| config.nodes.get(index));
    let active_fp = active.map(node_fingerprint);
    let active_name = active.map(|node| node.name.clone());
    let active_names = active.map(subscription_names).unwrap_or_default();
    let urls: HashSet<String> = refreshed.iter().map(|(url, _)| url.clone()).collect();
    let mut migrations = dns_migrations(&config.nodes, &refreshed);
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
    let mut merged = dedup_subscription_import(merged, fresh_len);
    // Shared and manual copies keep their existing references while present.
    if !migrations.is_empty() {
        for node in &merged {
            migrations.remove(&node_fingerprint(node));
        }
        for group in &mut config.groups {
            for member in group.members.iter_mut().chain(group.current.iter_mut()) {
                if let Some(new) = migrations.get(member) {
                    member.clone_from(new);
                }
            }
        }
        let targets = super::node_identity::rule_target_migrations(&migrations);
        for target in config
            .rules
            .iter_mut()
            .map(|r| &mut r.target)
            .chain(config.device_rule_groups.iter_mut().map(|r| &mut r.target))
        {
            if let Some(new) = targets.get(target) {
                target.clone_from(new);
            }
        }
    }
    config.active_node = active_fp.and_then(|fingerprint| {
        let fingerprint = migrations.get(&fingerprint).unwrap_or(&fingerprint);
        let named = active_name.as_ref().and_then(|name| {
            merged
                .iter()
                .position(|node| node.name == *name && node_fingerprint(node) == *fingerprint)
        });
        named
            .or_else(|| {
                merged
                    .iter()
                    .position(|node| node_fingerprint(node) == *fingerprint)
            })
            .or_else(|| {
                // Providers can rotate credentials or endpoints while retaining
                // the selected remark. Only accept an unambiguous match from the
                // same subscription; another provider's equal name is unrelated.
                let mut matches = merged.iter().enumerate().filter(|(_, node)| {
                    subscription_names(node)
                        .iter()
                        .any(|(source, name)| active_names.get(source) == Some(name))
                });
                let (index, _) = matches.next()?;
                matches.next().is_none().then_some(index)
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
    Some(migrations)
}

// The full identity is used by saved references; the DNS-independent identity
// is used only to match the same endpoint within its subscription.
fn dns_fingerprints(node: &Node) -> (String, String) {
    let id = node_fingerprint(node);
    if !node.extra.contains_key(super::server_dns::KEY) {
        return (id.clone(), id);
    }
    let mut endpoint = node.clone();
    endpoint.extra.remove(super::server_dns::KEY);
    (node_fingerprint(&endpoint), id)
}

fn dns_migrations(old: &[Node], refreshed: &[(String, Vec<Node>)]) -> HashMap<String, String> {
    if !old
        .iter()
        .chain(refreshed.iter().flat_map(|(_, nodes)| nodes))
        .any(|node| node.extra.contains_key(super::server_dns::KEY))
    {
        return HashMap::new();
    }

    let mut fresh_by_source = HashMap::<&str, HashMap<String, BTreeSet<String>>>::new();
    for (url, nodes) in refreshed {
        let endpoints = fresh_by_source.entry(url).or_default();
        for node in nodes {
            let (endpoint, id) = dns_fingerprints(node);
            endpoints.entry(endpoint).or_default().insert(id);
        }
    }
    // A shared node can be published by several sources. All matching
    // sources must agree on a single replacement before its references move.
    let mut replacements = HashMap::<String, BTreeSet<String>>::new();
    for node in old {
        let (endpoint, id) = dns_fingerprints(node);
        for url in subscription_sources(node) {
            if let Some(found) = fresh_by_source
                .get(url.as_str())
                .and_then(|endpoints| endpoints.get(&endpoint))
            {
                replacements
                    .entry(id.clone())
                    .or_default()
                    .extend(found.iter().cloned());
            }
        }
    }
    replacements
        .into_iter()
        .filter_map(|(old, mut ids)| {
            if ids.len() != 1 {
                return None;
            }
            let new = ids.pop_first()?;
            (old != new).then_some((old, new))
        })
        .collect()
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
    super::node_identity::fingerprint(node)
}

/// Pre-v0.0.18 identity, used only to migrate existing saved references.
pub(crate) fn legacy_node_fingerprint(node: &Node) -> String {
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

    let mut fingerprint = format!(
        "{}|{}|{}|{}",
        node.server.to_ascii_lowercase(),
        node.port,
        credentials,
        transport_summary(node.transport.as_ref())
    );
    if let Some(policy) = node.extra.get(super::server_dns::KEY) {
        fingerprint.push_str("|server-dns:");
        fingerprint.push_str(policy);
    }
    fingerprint
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
        TransportType::Xhttp => "xhttp",
        TransportType::Quic => "quic",
        TransportType::Grpc => "gRPC",
        TransportType::HttpUpgrade => "httpupgrade",
        TransportType::Reality => "reality",
    };
    let mut summary = ttype.to_string();
    if let Some(xhttp) = &tc.xhttp {
        summary.push_str(&format!(
            "|{}",
            serde_json::to_string(xhttp).unwrap_or_default()
        ));
    }
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
    let mut names = subscription_names(retained);
    let incoming_names = subscription_names(incoming);
    let manual = names.is_empty()
        || incoming_names.is_empty()
        || has_manual_copy(retained)
        || has_manual_copy(incoming);
    for (source, name) in incoming_names {
        names.entry(source).or_insert(name);
    }
    set_membership(retained, &names);
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
fn dedup_subscription_import(incoming: Vec<Node>, fresh_len: usize) -> Vec<Node> {
    let mut seen_named = HashMap::<String, usize>::new();
    let mut fresh_by_fp = HashMap::<String, usize>::new();
    let mut merged: Vec<Node> = Vec::with_capacity(incoming.len());
    let mut incoming = incoming.into_iter();
    for node in incoming.by_ref().take(fresh_len) {
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
        } else if let Some(&index) = fresh_by_fp.get(&fingerprint)
            && (has_manual_copy(&node) || subscription_sources(&node).is_empty())
        {
            merge_duplicate(&mut merged[index], &node);
        } else {
            seen_named.insert(key, merged.len());
            merged.push(node);
        }
    }
    merged
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
        // Backfill older nodes before adding a display-only suffix.
        let names = subscription_names(node);
        set_names(node, &names);
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
                    obfs: None,
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
            grpc: None,
            xhttp: None,
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
            grpc: None,
            xhttp: None,
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
            grpc: None,
            xhttp: None,
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
    fn refresh_restores_selected_remark_after_endpoint_rotation() {
        let url = "https://example.com/a";
        let mut old = ss("Tokyo", "192.0.2.1", 443, "old-password");
        tag_subscription_nodes(std::slice::from_mut(&mut old), url);
        let mut config = AppConfig {
            nodes: vec![old],
            active_node: Some(0),
            subscriptions: vec![Subscription {
                name: "A".into(),
                url: url.into(),
                format: "auto".into(),
                last_updated: None,
                refresh_interval_hours: 24,
                refresh_revision: String::new(),
            }],
            ..AppConfig::default()
        };
        let mut unrelated = ss("Tokyo", "192.0.2.7", 443, "other");
        tag_subscription_nodes(
            std::slice::from_mut(&mut unrelated),
            "https://example.com/b",
        );
        config.nodes.push(unrelated);
        assert!(apply_subscription_refresh(
            &mut config,
            vec![(
                url.into(),
                vec![
                    ss("Singapore", "192.0.2.2", 443, "new-password"),
                    ss("Tokyo", "192.0.2.3", 8443, "new-password"),
                ]
            )]
        ));
        let selected = &config.nodes[config.active_node.expect("selection survives refresh")];
        assert_eq!(selected.server, "192.0.2.3");
        assert_eq!(selected.port, 8443);
        // The same remark in a different subscription must never be selected.
        assert!(apply_subscription_refresh(
            &mut config,
            vec![(
                url.into(),
                vec![ss("Singapore", "192.0.2.2", 443, "new-password"),]
            )]
        ));
        assert_eq!(config.active_node, None);
    }

    #[test]
    fn dns_migration_requires_same_source_and_unique_replacement() {
        let url = "https://source.invalid/a";
        let mut node = ss("HK", "node.invalid", 443, "pw");
        tag_subscription_nodes(std::slice::from_mut(&mut node), url);
        let mut a = node.clone();
        a.extra
            .insert(super::super::server_dns::KEY.into(), "a".into());
        let mut b = a.clone();
        b.extra
            .insert(super::super::server_dns::KEY.into(), "b".into());
        for (source, nodes) in [
            ("https://source.invalid/b", vec![a.clone()]),
            (url, vec![a, b]),
        ] {
            assert!(
                dns_migrations(std::slice::from_ref(&node), &[(source.into(), nodes)]).is_empty()
            );
        }
    }

    #[test]
    fn refresh_does_not_guess_between_duplicate_rotated_remarks() {
        let url = "https://example.com/a";
        let mut old = ss("Tokyo", "192.0.2.1", 443, "old");
        tag_subscription_nodes(std::slice::from_mut(&mut old), url);
        let mut config = AppConfig {
            nodes: vec![old],
            active_node: Some(0),
            subscriptions: vec![Subscription {
                name: "A".into(),
                url: url.into(),
                format: "auto".into(),
                last_updated: None,
                refresh_interval_hours: 24,
                refresh_revision: String::new(),
            }],
            ..AppConfig::default()
        };
        apply_subscription_refresh(
            &mut config,
            vec![(
                url.into(),
                vec![
                    ss("Tokyo", "192.0.2.2", 443, "new"),
                    ss("Tokyo", "192.0.2.3", 443, "new"),
                ],
            )],
        );
        assert_eq!(config.active_node, None);
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
    fn merged_legacy_memberships_preserve_names_and_manual_copy() {
        let mut retained = node();
        retained.extra.insert(SUB_URL_EXTRA_KEY.into(), "b".into());
        retained.extra.insert(
            NAMES_KEY.into(),
            r#"{"b":"old remark","stale":"removed"}"#.into(),
        );
        let mut incoming = node();
        incoming
            .extra
            .insert(SOURCES_KEY.into(), r#"["b","a"]"#.into());
        incoming.extra.insert(
            NAMES_KEY.into(),
            r#"{"a":"new remark","b":"ignored duplicate"}"#.into(),
        );
        incoming.extra.insert(MANUAL_KEY.into(), "true".into());
        merge_duplicate(&mut retained, &incoming);
        assert_eq!(retained.extra[SUB_URL_EXTRA_KEY], "a");
        assert_eq!(retained.extra[SOURCES_KEY], r#"["a","b"]"#);
        assert_eq!(
            retained.extra[NAMES_KEY],
            r#"{"a":"new remark","b":"old remark"}"#
        );
        let mut nodes = vec![retained];
        detach_sources(&mut nodes, &HashSet::from(["a".into(), "b".into()]));
        assert_eq!(nodes.len(), 1);
        assert_eq!(
            nodes[0].extra,
            HashMap::from([(MANUAL_KEY.into(), "true".into())])
        );
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
    fn dns_refresh_keeps_references_to_surviving_shared_or_manual_nodes() {
        let a = "https://example.org/a";
        let b = "https://example.org/b";
        for shared in [true, false] {
            let mut old = node();
            old.extra
                .insert(super::super::server_dns::KEY.into(), "old".into());
            tag_subscription_nodes(std::slice::from_mut(&mut old), a);
            if shared {
                set_sources(&mut old, &BTreeSet::from([a.into(), b.into()]));
            } else {
                old.extra.insert(MANUAL_KEY.into(), "true".into());
            }
            let old_id = node_fingerprint(&old);
            let fresh = node();
            let mut config = AppConfig {
                subscriptions: vec![source(a), source(b)],
                nodes: vec![old],
                active_node: Some(0),
                groups: vec![ProxyGroupConfig {
                    name: "selected".into(),
                    gtype: GroupType::Select,
                    members: vec![old_id.clone()],
                    current: Some(old_id.clone()),
                }],
                ..Default::default()
            };
            let migrations =
                refresh_with_dns_migrations(&mut config, vec![(a.into(), vec![fresh])]).unwrap();
            assert!(migrations.is_empty());
            assert_eq!(config.nodes.len(), 2);
            assert_eq!(
                node_fingerprint(&config.nodes[config.active_node.unwrap()]),
                old_id
            );
            assert_eq!(config.groups[0].members, [old_id.as_str()]);
            assert_eq!(config.groups[0].current.as_deref(), Some(old_id.as_str()));
        }
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

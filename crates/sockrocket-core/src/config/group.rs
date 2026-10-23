//! Proxy-group behavior helpers.
//!
//! Groups store members as node fingerprints (see `dedup::node_fingerprint`)
//! so membership survives renames and subscription refreshes. These helpers
//! are pure functions: they map fingerprints to node indices and turn probe
//! results into a new pick. The running proxy also races url-test members and
//! remembers a fallback success in memory. Fingerprints that no longer resolve to a node are
//! skipped, never an error.

use super::dedup::node_fingerprint;
use super::model::{AppConfig, GroupType, Node, ProxyGroupConfig, ProxyMode};

/// A probe result for one group member, in member order (as returned by
/// [`resolve_members`]): `Some(latency_ms)` for a successful probe,
/// `None` for a failure / unreachable node.
pub type ProbeResult = (usize, Option<u32>);

/// Resolve a group's member fingerprints to node indices, in member order.
/// Fingerprints that no longer match any node are silently skipped, and
/// duplicate fingerprints resolve to the same first match.
pub fn resolve_members(group: &ProxyGroupConfig, nodes: &[Node]) -> Vec<usize> {
    group
        .members
        .iter()
        .filter_map(|fp| nodes.iter().position(|n| node_fingerprint(n) == *fp))
        .collect()
}

/// `url-test` pick: the member with the lowest successful probe latency.
/// Returns the node's fingerprint (to store in `group.current`), or `None`
/// when no member probed successfully.
pub fn url_test_pick(nodes: &[Node], probes: &[ProbeResult]) -> Option<String> {
    probes
        .iter()
        .filter_map(|&(idx, latency)| latency.map(|ms| (idx, ms)))
        .min_by_key(|&(_, ms)| ms)
        .and_then(|(idx, _)| nodes.get(idx))
        .map(node_fingerprint)
}

/// `fallback` pick: the first member (in order) whose probe succeeded.
/// Returns the node's fingerprint (to store in `group.current`), or `None`
/// when every member failed.
pub fn fallback_pick(nodes: &[Node], probes: &[ProbeResult]) -> Option<String> {
    probes
        .iter()
        .find(|&&(_, latency)| latency.is_some())
        .map(|&(idx, _)| idx)
        .and_then(|idx| nodes.get(idx))
        .map(node_fingerprint)
}

/// `node:` plus the lowercase SHA-256 from [`super::dedup::node_public_id`].
pub fn node_target_id(target: &str) -> Option<&str> {
    let id = target.strip_prefix("node:")?;
    if id.len() == 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Some(id)
    } else {
        None
    }
}

/// A rule that dials some proxy, rather than direct or reject.
pub fn is_proxy_target(target: &str) -> bool {
    target == "proxy" || target.starts_with("group:") || node_target_id(target).is_some()
}

/// Names are portable through native CSV, JSON and YAML rule editors.
pub fn valid_group_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 96
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_'))
}

/// Whether the active node, or an unsourced group's datagram member, can
/// carry UDP. The firewall does not use this to open or close QUIC: each
/// datagram is decided in userspace. The flag remains for diagnostics.
pub fn lan_wide_udp(config: &AppConfig) -> Result<bool, &'static str> {
    if config.proxy_mode == ProxyMode::Direct {
        return Ok(true);
    }
    if let Some(index) = active_index(config)?
        && config
            .nodes
            .get(index)
            .is_some_and(|node| node.protocol.supports_udp())
    {
        return Ok(true);
    }
    Ok(config.rules.iter().any(|rule| {
        rule.enabled
            && rule.source.is_empty()
            && (rule_group_supports_udp(rule.target.as_str(), config)
                || rule_node_supports_udp(rule.target.as_str(), config))
    }))
}

fn rule_node_supports_udp(target: &str, config: &AppConfig) -> bool {
    let Some(id) = node_target_id(target) else {
        return false;
    };
    config.nodes.iter().any(|node| {
        super::dedup::node_public_id(&super::dedup::node_fingerprint(node)) == id
            && node.protocol.supports_udp()
    })
}

fn active_index(config: &AppConfig) -> Result<Option<usize>, &'static str> {
    if config.nodes.is_empty() {
        return Ok(None);
    }
    match config.active_node {
        Some(index) if index < config.nodes.len() => Ok(Some(index)),
        Some(_) => Err("active_node is out of range"),
        None => Ok(Some(0)),
    }
}

/// UDP stays on a fallback group's first member. Select uses the stored
/// pick. url-test's live choice can move after a handshake race; this check
/// uses the stored pick, or the first member when nothing is stored.
fn group_udp_member(group: &ProxyGroupConfig, nodes: &[Node]) -> Option<usize> {
    if group.gtype == GroupType::Fallback {
        resolve_members(group, nodes).first().copied()
    } else {
        selected_member(group, nodes)
    }
}

fn rule_group_supports_udp(target: &str, config: &AppConfig) -> bool {
    let Some(name) = target.strip_prefix("group:") else {
        return false;
    };
    let Some(group) = config.groups.iter().find(|group| group.name == name) else {
        return false;
    };
    group_udp_member(group, &config.nodes)
        .and_then(|index| config.nodes.get(index))
        .is_some_and(|node| node.protocol.supports_udp())
}

/// Resolve the selected member without silently replacing a deleted selection.
pub fn selected_member(group: &ProxyGroupConfig, nodes: &[Node]) -> Option<usize> {
    let members = resolve_members(group, nodes);
    if let Some(current) = &group.current {
        return members
            .into_iter()
            .find(|i| node_fingerprint(&nodes[*i]) == *current);
    }
    members.first().copied()
}

pub fn validate_targets(
    groups: &[ProxyGroupConfig],
    rules: &[super::model::RoutingRule],
) -> Result<(), &'static str> {
    let mut names = std::collections::HashSet::new();
    if groups.len() > 64 {
        return Err("Too many proxy groups (maximum 64)");
    }
    for group in groups {
        if !names.insert(group.name.as_str()) {
            return Err("Duplicate proxy group name");
        }
        if group.members.len() > 256 {
            return Err("Too many group members (maximum 256)");
        }
    }
    for rule in rules {
        if let Some(name) = rule.target.strip_prefix("group:")
            && !names.contains(name)
        {
            return Err("Rule refers to a missing proxy group");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::{GroupType, ProxyProtocol, RoutingRule};

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

    fn group(members: Vec<String>) -> ProxyGroupConfig {
        ProxyGroupConfig {
            name: "test".to_string(),
            gtype: GroupType::Select,
            members,
            current: None,
        }
    }

    fn fps(nodes: &[Node]) -> Vec<String> {
        nodes.iter().map(node_fingerprint).collect()
    }

    #[test]
    fn resolve_members_maps_fingerprints_in_order() {
        let nodes = vec![
            ss("a", "1.1.1.1", 8388, "pw1"),
            ss("b", "2.2.2.2", 8388, "pw2"),
            ss("c", "3.3.3.3", 8388, "pw3"),
        ];
        let fp = fps(&nodes);
        // Reverse order + one stale fingerprint that no longer resolves.
        let g = group(vec![
            fp[2].clone(),
            "stale-fingerprint".to_string(),
            fp[0].clone(),
        ]);
        assert_eq!(resolve_members(&g, &nodes), vec![2, 0]);
    }

    #[test]
    fn resolve_members_survives_renames() {
        let nodes = vec![ss("new name", "1.1.1.1", 8388, "pw")];
        let g = group(vec![node_fingerprint(&ss(
            "old name", "1.1.1.1", 8388, "pw",
        ))]);
        assert_eq!(resolve_members(&g, &nodes), vec![0]);
    }

    #[test]
    fn resolve_members_empty_group() {
        let nodes = vec![ss("a", "1.1.1.1", 8388, "pw")];
        assert!(resolve_members(&group(vec![]), &nodes).is_empty());
    }

    #[test]
    fn url_test_pick_chooses_lowest_successful_latency() {
        let nodes = vec![
            ss("a", "1.1.1.1", 8388, "pw1"),
            ss("b", "2.2.2.2", 8388, "pw2"),
            ss("c", "3.3.3.3", 8388, "pw3"),
        ];
        let probes: Vec<ProbeResult> = vec![(0, Some(120)), (1, None), (2, Some(45))];
        assert_eq!(url_test_pick(&nodes, &probes), Some(fps(&nodes)[2].clone()));
    }

    #[test]
    fn url_test_pick_returns_none_when_all_failed() {
        let nodes = vec![ss("a", "1.1.1.1", 8388, "pw")];
        let probes: Vec<ProbeResult> = vec![(0, None)];
        assert_eq!(url_test_pick(&nodes, &probes), None);
        assert_eq!(url_test_pick(&nodes, &[]), None);
    }

    #[test]
    fn url_test_pick_ignores_out_of_range_indices() {
        let nodes = vec![ss("a", "1.1.1.1", 8388, "pw")];
        let probes: Vec<ProbeResult> = vec![(99, Some(10))];
        assert_eq!(url_test_pick(&nodes, &probes), None);
    }

    #[test]
    fn fallback_pick_chooses_first_reachable_member() {
        let nodes = vec![
            ss("a", "1.1.1.1", 8388, "pw1"),
            ss("b", "2.2.2.2", 8388, "pw2"),
            ss("c", "3.3.3.3", 8388, "pw3"),
        ];
        // First member dead: second wins even though third has lower latency.
        let probes: Vec<ProbeResult> = vec![(0, None), (1, Some(200)), (2, Some(45))];
        assert_eq!(fallback_pick(&nodes, &probes), Some(fps(&nodes)[1].clone()));
        // First member alive: always wins regardless of latency.
        let probes: Vec<ProbeResult> = vec![(0, Some(500)), (1, Some(10))];
        assert_eq!(fallback_pick(&nodes, &probes), Some(fps(&nodes)[0].clone()));
    }

    #[test]
    fn fallback_pick_returns_none_when_all_failed() {
        let nodes = vec![ss("a", "1.1.1.1", 8388, "pw")];
        let probes: Vec<ProbeResult> = vec![(0, None)];
        assert_eq!(fallback_pick(&nodes, &probes), None);
        assert_eq!(fallback_pick(&nodes, &[]), None);
    }

    fn stream_only(name: &str, server: &str) -> Node {
        let mut node = ss(name, server, 8388, name);
        if let ProxyProtocol::Shadowsocks { udp, .. } = &mut node.protocol {
            *udp = false;
        }
        node
    }

    fn domain_rule(source: &str, target: &str, enabled: bool) -> RoutingRule {
        RoutingRule {
            source: source.to_string(),
            name: String::new(),
            group: String::new(),
            rule_type: "domain".to_string(),
            pattern: "example.com".to_string(),
            target: target.to_string(),
            enabled,
            priority: 0,
        }
    }

    fn config_with(
        nodes: Vec<Node>,
        groups: Vec<ProxyGroupConfig>,
        rules: Vec<RoutingRule>,
    ) -> AppConfig {
        AppConfig {
            nodes,
            active_node: Some(0),
            groups,
            rules,
            ..AppConfig::default()
        }
    }

    #[test]
    fn lan_wide_udp_follows_active_node_or_unsourced_group() {
        let stream = stream_only("primary", "1.1.1.1");
        let udp = ss("stream", "2.2.2.2", 8388, "pw");
        let nodes = vec![stream, udp];
        let fingerprints = fps(&nodes);
        let mut media = group(vec![fingerprints[1].clone()]);
        media.name = "media".to_string();
        let config = config_with(
            nodes,
            vec![media],
            vec![domain_rule("", "group:media", true)],
        );
        assert!(lan_wide_udp(&config).unwrap());

        let mut only_device = config;
        only_device.rules = vec![domain_rule("192.0.2.10/32", "group:media", true)];
        assert!(!lan_wide_udp(&only_device).unwrap());

        only_device.rules = vec![domain_rule("", "group:media", false)];
        assert!(!lan_wide_udp(&only_device).unwrap());

        only_device.rules = vec![domain_rule("", "proxy", true)];
        assert!(!lan_wide_udp(&only_device).unwrap());
    }

    #[test]
    fn lan_wide_udp_uses_fallback_first_member_not_a_later_pick() {
        let nodes = vec![
            stream_only("primary", "1.1.1.1"),
            stream_only("first", "2.2.2.2"),
            ss("later", "3.3.3.3", 8388, "pw"),
        ];
        let fingerprints = fps(&nodes);
        let mut media = group(vec![fingerprints[1].clone(), fingerprints[2].clone()]);
        media.name = "media".to_string();
        media.gtype = GroupType::Fallback;
        media.current = Some(fingerprints[2].clone());
        let config = config_with(
            nodes,
            vec![media],
            vec![domain_rule("", "group:media", true)],
        );
        assert!(!lan_wide_udp(&config).unwrap());
    }

    #[test]
    fn lan_wide_udp_direct_mode_and_rejects_a_bad_index() {
        let mut config = AppConfig::default();
        config.proxy_mode = ProxyMode::Direct;
        assert!(lan_wide_udp(&config).unwrap());
        config.proxy_mode = ProxyMode::Rule;
        config.nodes = vec![stream_only("primary", "1.1.1.1")];
        config.active_node = Some(4);
        assert!(lan_wide_udp(&config).is_err());
    }
}

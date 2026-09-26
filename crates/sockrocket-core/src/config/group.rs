//! Proxy-group behavior helpers.
//!
//! Groups store members as node fingerprints (see `dedup::node_fingerprint`)
//! so membership survives renames and subscription refreshes. These helpers
//! are pure functions: they map fingerprints to node indices and turn probe
//! results into a new pick, leaving the actual probing and persistence to
//! the caller (the GUI). Fingerprints that no longer resolve to a node are
//! skipped, never an error.

use super::dedup::node_fingerprint;
use super::model::{Node, ProxyGroupConfig};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::{GroupType, ProxyProtocol};

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
}

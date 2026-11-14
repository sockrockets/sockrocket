//! Search metadata belongs to one view instance. Latency changes invalidate
//! ordering, without rebuilding normalized text, tags, or protocol chips.
use std::collections::BTreeSet;
use std::rc::Rc;

use sockrocket_core::config::model::{Node, ProxyProtocol};

pub fn protocol_short_name(protocol: &ProxyProtocol) -> &'static str {
    match protocol {
        ProxyProtocol::Shadowsocks { .. } => "ss",
        ProxyProtocol::VMess { .. } => "vmess",
        ProxyProtocol::VLess { .. } => "vless",
        ProxyProtocol::Tuic { .. } => "tuic",
        ProxyProtocol::Trojan { .. } => "trojan",
        ProxyProtocol::Hysteria2 { .. } => "hy2",
        ProxyProtocol::Mieru { .. } => "mieru",
        ProxyProtocol::AnyTls { .. } => "anytls",
        ProxyProtocol::Wireguard { .. } => "wireguard",
    }
}

#[derive(Clone, Default)]
pub struct NodeListSnapshot {
    pub indices: Rc<Vec<usize>>,
    pub protocols: Rc<Vec<&'static str>>,
    pub tags: Rc<Vec<String>>,
}

struct SearchDocument {
    protocol: &'static str,
    fields: Vec<String>,
}

#[derive(Default)]
pub struct NodeListCache {
    content_generation: Option<u64>,
    latency_generation: Option<u64>,
    documents: Vec<SearchDocument>,
    filter: Option<(String, Option<&'static str>, Option<String>)>,
    matches: Vec<usize>,
    snapshot: NodeListSnapshot,
}

impl NodeListCache {
    /// Callers bump content_generation on edits/import/reorder, and
    /// latency_generation on probe completion. The query is already lowercase.
    pub fn prepare(
        &mut self,
        nodes: &[Node],
        content_generation: u64,
        latency_generation: u64,
        query: &str,
        protocol: Option<&'static str>,
        tag: Option<&str>,
    ) -> NodeListSnapshot {
        if self.content_generation != Some(content_generation) {
            self.documents = nodes
                .iter()
                .map(|node| {
                    let mut fields = Vec::with_capacity(2 + node.tags.len());
                    fields.push(node.name.to_lowercase());
                    fields.push(node.server.to_lowercase());
                    fields.extend(node.tags.iter().map(|t| t.to_lowercase()));
                    SearchDocument {
                        protocol: protocol_short_name(&node.protocol),
                        fields,
                    }
                })
                .collect();
            let protocols: BTreeSet<_> = self.documents.iter().map(|d| d.protocol).collect();
            let order = [
                "ss",
                "vmess",
                "vless",
                "trojan",
                "tuic",
                "hy2",
                "anytls",
                "mieru",
                "wireguard",
            ];
            self.snapshot.protocols = Rc::new(
                order
                    .into_iter()
                    .filter(|p| protocols.contains(p))
                    .collect(),
            );
            self.snapshot.tags = Rc::new(
                nodes
                    .iter()
                    .flat_map(|n| n.tags.iter().cloned())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
            );
            self.content_generation = Some(content_generation);
            self.filter = None;
        }
        let filter_changed = self
            .filter
            .as_ref()
            .is_none_or(|(q, p, t)| q != query || *p != protocol || t.as_deref() != tag);
        if filter_changed {
            self.matches = self
                .documents
                .iter()
                .enumerate()
                .filter_map(|(i, doc)| {
                    let matches = protocol.is_none_or(|p| doc.protocol == p)
                        && tag.is_none_or(|t| nodes[i].tags.iter().any(|nt| nt == t))
                        && (query.is_empty()
                            || doc.protocol.contains(query)
                            || doc.fields.iter().any(|f| f.contains(query)));
                    matches.then_some(i)
                })
                .collect();
            self.filter = Some((query.into(), protocol, tag.map(str::to_owned)));
        }
        if filter_changed || self.latency_generation != Some(latency_generation) {
            let mut indices = self.matches.clone();
            indices.sort_unstable_by_key(|&i| {
                let latency = nodes[i].latency_ms;
                (latency.is_none(), latency.unwrap_or_default(), i)
            });
            self.snapshot.indices = Rc::new(indices);
            self.latency_generation = Some(latency_generation);
        }
        self.snapshot.clone()
    }
}

/// Return old-index -> new-index while moving nodes only once. UI selection,
/// probe state and active identity can all use this map, even with duplicate names.
pub fn sort_by_latency(nodes: &mut Vec<Node>) -> Vec<usize> {
    let mut indexed: Vec<_> = std::mem::take(nodes).into_iter().enumerate().collect();
    indexed.sort_by_key(|(_, node)| {
        (
            node.latency_ms.is_none(),
            node.latency_ms.unwrap_or_default(),
        )
    });
    let mut remap = vec![0; indexed.len()];
    for (new, (old, _)) in indexed.iter().enumerate() {
        remap[*old] = new;
    }
    *nodes = indexed.into_iter().map(|(_, node)| node).collect();
    remap
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, latency: Option<u32>) -> Node {
        Node {
            name: name.into(),
            server: "EXAMPLE.com".into(),
            port: 443,
            protocol: ProxyProtocol::Trojan {
                password: String::new(),
                udp: false,
            },
            transport: None,
            latency_ms: latency,
            tags: vec!["Việt Nam".into()],
            extra: Default::default(),
        }
    }

    #[test]
    fn search_matches_each_field_and_combines_filters() {
        let nodes = vec![node("香港 A", Some(20)), node("Other", None)];
        let mut cache = NodeListCache::default();
        for query in ["香港", "example", "trojan", "việt"] {
            let view = cache.prepare(&nodes, 0, 0, query, Some("trojan"), Some("Việt Nam"));
            assert!(!view.indices.is_empty());
        }
        assert!(
            cache
                .prepare(&nodes, 0, 0, "a\nexample", None, None)
                .indices
                .is_empty()
        );
        assert!(
            cache
                .prepare(&nodes, 0, 0, "", Some("ss"), None)
                .indices
                .is_empty()
        );
        assert!(
            cache
                .prepare(&nodes, 0, 0, "", None, Some("việt nam"))
                .indices
                .is_empty()
        );
    }

    #[test]
    fn latency_updates_reuse_search_metadata_and_unchanged_frames_reuse_rows() {
        let mut nodes: Vec<_> = (0..10_000)
            .map(|i| node(&format!("Node {i}"), None))
            .collect();
        let mut cache = NodeListCache::default();
        let first = cache.prepare(&nodes, 0, 0, "node", None, None);
        let doc = cache.documents[0].fields[0].as_ptr();
        let again = cache.prepare(&nodes, 0, 0, "node", None, None);
        assert!(Rc::ptr_eq(&first.indices, &again.indices));
        nodes[9999].latency_ms = Some(1);
        let updated = cache.prepare(&nodes, 0, 1, "node", None, None);
        assert_eq!(updated.indices[0], 9999);
        assert_eq!(doc, cache.documents[0].fields[0].as_ptr());
        assert!(Rc::ptr_eq(&first.tags, &updated.tags));
        assert!(Rc::ptr_eq(&first.protocols, &updated.protocols));
        nodes.truncate(1);
        nodes[0].name = "Renamed".into();
        assert!(
            cache
                .prepare(&nodes, 1, 2, "node", None, None)
                .indices
                .is_empty()
        );
        assert_eq!(
            &*cache.prepare(&nodes, 1, 2, "renamed", None, None).indices,
            &[0]
        );
    }

    #[test]
    fn sort_map_preserves_distinct_nodes_with_duplicate_names_and_latency_ties() {
        let mut nodes = vec![
            node("Same", None),
            node("Same", Some(20)),
            node("Same", Some(10)),
            node("Same", Some(20)),
        ];
        let remap = sort_by_latency(&mut nodes);
        assert_eq!(remap, vec![3, 1, 0, 2]);
        assert_eq!(
            nodes.iter().map(|n| n.latency_ms).collect::<Vec<_>>(),
            vec![Some(10), Some(20), Some(20), None]
        );
    }

    #[test]
    fn separate_windows_do_not_share_cache_generations() {
        let mut a = NodeListCache::default();
        let mut b = NodeListCache::default();
        a.prepare(&[node("Window-A", None)], 0, 0, "window-a", None, None);
        assert!(
            b.prepare(&[node("Window-B", None)], 0, 0, "window-a", None, None)
                .indices
                .is_empty()
        );
    }
}

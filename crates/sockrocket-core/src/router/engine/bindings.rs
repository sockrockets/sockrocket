//! Linked device groups share their compiled members. Merge only the bindings
//! applicable to a connection in the same order as the expanded rules.
use super::*;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

#[derive(Clone, Copy)]
pub(super) struct RuleView<'a> {
    pub(super) config_index: Option<usize>,
    pub(super) source: Option<(IpAddr, u8)>,
    pub(super) rule: &'a MatchRule,
    pub(super) action: &'a RouteAction,
}

impl<'a> From<&'a RuleEntry> for RuleView<'a> {
    fn from(entry: &'a RuleEntry) -> Self {
        Self {
            config_index: entry.config_index,
            source: entry.source,
            rule: &entry.rule,
            action: &entry.action,
        }
    }
}

#[derive(Debug, Clone)]
struct Member {
    rank: usize,
    entry: RuleEntry,
}

#[derive(Debug, Clone)]
struct Binding {
    scope: (IpAddr, u8),
    priority: u16,
    members: Arc<[Member]>,
    prefix_priority: u8,
    action: Option<RouteAction>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct DeviceBindings {
    links: Vec<Binding>,
}

// BinaryHeap chooses the greatest key first. Field order mirrors the saved rule precedence.
#[derive(Ord, PartialOrd, Eq, PartialEq)]
struct Cursor {
    priority: u16,
    member_rank: Reverse<usize>,
    prefix: u8,
    binding: Reverse<usize>,
    offset: usize,
}

impl DeviceBindings {
    pub(super) fn compile(
        rules: &[(usize, &RoutingRule)],
        links: &[crate::config::model::DeviceRuleGroup],
    ) -> Self {
        let mut groups: HashMap<&str, Vec<Member>> = links
            .iter()
            .filter(|link| !link.source.is_empty())
            .map(|link| (link.group.as_str(), Vec::new()))
            .collect();
        if groups.is_empty() {
            return Self::default();
        }
        let mut members: Vec<_> = rules
            .iter()
            .copied()
            .filter(|(_, r)| groups.contains_key(r.group.trim()))
            .collect();
        members.sort_by_key(|(index, r)| (Reverse(r.priority), *index));
        for (rank, (index, rule)) in members.into_iter().enumerate() {
            if let Some(entry) = RuleEntry::compile(index, rule) {
                groups
                    .get_mut(rule.group.trim())
                    .unwrap()
                    .push(Member { rank, entry });
            }
        }
        let groups: HashMap<_, Arc<[Member]>> = groups
            .into_iter()
            .filter(|(_, members)| !members.is_empty())
            .map(|(name, members)| (name, members.into()))
            .collect();
        let links = links
            .iter()
            .filter_map(|link| {
                let scope = parse_cidr(&normalize_rule_source(&link.source).ok()?)?;
                Some(Binding {
                    scope,
                    prefix_priority: link
                        .source
                        .split_once('/')
                        .and_then(|(_, n)| n.parse().ok())
                        .unwrap_or(0),
                    priority: link.priority,
                    members: groups.get(link.group.as_str())?.clone(),
                    action: (!link.target.is_empty())
                        .then(|| RouteAction::from_target(&link.target)),
                })
            })
            .collect();
        Self { links }
    }

    pub(super) fn entries(
        &self,
        source: Option<IpAddr>,
        include_other_sources: bool,
        paused: bool,
    ) -> impl Iterator<Item = RuleView<'_>> {
        // Binding priority, member rank, then longest source prefix. The
        // original binding index breaks exact ties just as the stable sort did.
        let mut heap = BinaryHeap::new();
        if !paused && (source.is_some() || include_other_sources) {
            for (i, link) in self.links.iter().enumerate() {
                if include_other_sources || source_matches(Some(link.scope), source) {
                    heap.push(Cursor {
                        priority: link.priority,
                        member_rank: Reverse(link.members[0].rank),
                        prefix: link.prefix_priority,
                        binding: Reverse(i),
                        offset: 0,
                    });
                }
            }
        }
        std::iter::from_fn(move || {
            let mut head = heap.peek_mut()?;
            let i = head.binding.0;
            let offset = head.offset;
            let link = &self.links[i];
            if let Some(next) = link.members.get(offset + 1) {
                head.member_rank = Reverse(next.rank);
                head.offset += 1;
            } else {
                std::collections::binary_heap::PeekMut::pop(head);
            }
            let entry = &link.members[offset].entry;
            Some(RuleView {
                source: Some(link.scope),
                action: link.action.as_ref().unwrap_or(&entry.action),
                ..RuleView::from(entry)
            })
        })
    }
    pub(super) fn is_empty(&self) -> bool {
        self.links.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::DeviceRuleGroup;

    #[test]
    fn connections_only_iterate_their_own_shared_group_members() {
        let rule = RoutingRule {
            source: String::new(),
            group: "video".into(),
            name: String::new(),
            rule_type: "domain".into(),
            pattern: "video.test".into(),
            target: "direct".into(),
            priority: 0,
            enabled: true,
        };
        let links: Vec<_> = (1..=16)
            .map(|n| DeviceRuleGroup {
                source: format!("192.0.2.{n}/32"),
                group: "video".into(),
                priority: 0,
                target: String::new(),
            })
            .collect();
        let bindings = DeviceBindings::compile(&[(0, &rule)], &links);
        assert_eq!(bindings.links.len(), 16);
        assert!(
            bindings
                .links
                .iter()
                .all(|link| Arc::ptr_eq(&link.members, &bindings.links[0].members))
        );
        assert_eq!(
            bindings
                .entries(Some("192.0.2.3".parse().unwrap()), false, false)
                .count(),
            1
        );
        assert_eq!(bindings.entries(None, false, false).count(), 0);
        assert_eq!(bindings.entries(None, true, false).count(), 16);
    }
}

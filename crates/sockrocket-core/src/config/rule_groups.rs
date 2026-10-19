//! Group rules for UI display (matching order stays global by priority).

use crate::config::model::RoutingRule;

/// One display bucket: group label + indices into the flat `rules` vec.
#[derive(Debug, Clone)]
pub struct RuleGroupView {
    /// Empty string means ungrouped.
    pub name: String,
    pub indices: Vec<usize>,
}

/// Partition rules into groups in first-seen order; ungrouped last.
pub fn group_rules_for_display(rules: &[RoutingRule]) -> Vec<RuleGroupView> {
    let mut named: Vec<RuleGroupView> = Vec::new();
    let mut ungrouped: Vec<usize> = Vec::new();
    for (i, r) in rules.iter().enumerate() {
        let g = r.group.trim();
        if g.is_empty() {
            ungrouped.push(i);
            continue;
        }
        if let Some(bucket) = named.iter_mut().find(|b| b.name == g) {
            bucket.indices.push(i);
        } else {
            named.push(RuleGroupView {
                name: g.to_string(),
                indices: vec![i],
            });
        }
    }
    if !ungrouped.is_empty() {
        named.push(RuleGroupView {
            name: String::new(),
            indices: ungrouped,
        });
    }
    named
}

/// True when `rule_group` belongs to the display bucket `want` (empty = ungrouped).
pub fn same_rule_group(rule_group: &str, want: &str) -> bool {
    rule_group.trim() == want.trim()
}

/// Indices of every rule in `group` (empty name = ungrouped).
pub fn group_member_indices(rules: &[RoutingRule], group: &str) -> Vec<usize> {
    rules
        .iter()
        .enumerate()
        .filter(|(_, r)| same_rule_group(&r.group, group))
        .map(|(i, _)| i)
        .collect()
}

/// Shared fields of a group, taken from its members.
#[derive(Debug, Clone)]
pub struct RuleGroupSnapshot {
    pub rule_type: String,
    pub target: String,
    pub priority: i32,
    /// True only when every member is enabled.
    pub enabled: bool,
    pub patterns: Vec<String>,
    /// Members do not all share `rule_type`.
    pub mixed_type: bool,
}

/// Snapshot a group for editing it as one unit. `None` when the group is empty.
pub fn snapshot_rule_group(rules: &[RoutingRule], group: &str) -> Option<RuleGroupSnapshot> {
    let members: Vec<&RoutingRule> = rules
        .iter()
        .filter(|r| same_rule_group(&r.group, group))
        .collect();
    let first = *members.first()?;
    let mixed_type = members.iter().any(|r| r.rule_type != first.rule_type);
    Some(RuleGroupSnapshot {
        rule_type: first.rule_type.clone(),
        target: first.target.clone(),
        priority: first.priority,
        enabled: members.iter().all(|r| r.enabled),
        patterns: members.iter().map(|r| r.pattern.clone()).collect(),
        mixed_type,
    })
}

/// Replace every member of `old_group` with `incoming`, keeping the block's
/// position. Fails when the new group name already belongs to a different block.
pub fn replace_rule_group(
    rules: &mut Vec<RoutingRule>,
    old_group: &str,
    incoming: Vec<RoutingRule>,
) -> Result<usize, String> {
    if incoming.is_empty() {
        return Err("分组里至少要有一条规则".into());
    }
    let new_name = incoming[0].group.trim().to_string();
    if !same_rule_group(&new_name, old_group)
        && rules.iter().any(|r| same_rule_group(&r.group, &new_name))
    {
        return Err(format!("分组「{new_name}」已存在"));
    }
    let insert_at = rules
        .iter()
        .position(|r| same_rule_group(&r.group, old_group))
        .unwrap_or(rules.len());
    rules.retain(|r| !same_rule_group(&r.group, old_group));
    let at = insert_at.min(rules.len());
    let n = incoming.len();
    for (i, rule) in incoming.into_iter().enumerate() {
        rules.insert(at + i, rule);
    }
    Ok(n)
}

/// Distinct non-empty group names in first-seen order (for pickers).
pub fn existing_rule_groups(rules: &[RoutingRule]) -> Vec<String> {
    let mut out = Vec::new();
    for r in rules {
        let g = r.group.trim();
        if g.is_empty() {
            continue;
        }
        if !out.iter().any(|x| x == g) {
            out.push(g.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(group: &str, pattern: &str) -> RoutingRule {
        RoutingRule {
            name: String::new(),
            group: group.into(),
            rule_type: "domain-suffix".into(),
            pattern: pattern.into(),
            target: "proxy".into(),
            enabled: true,
            priority: 0,
        }
    }

    #[test]
    fn groups_preserve_order_ungrouped_last() {
        let rules = vec![
            r("A", "a.com"),
            r("", "x.com"),
            r("B", "b.com"),
            r("A", "a2.com"),
        ];
        let g = group_rules_for_display(&rules);
        assert_eq!(g.len(), 3);
        assert_eq!(g[0].name, "A");
        assert_eq!(g[0].indices, vec![0, 3]);
        assert_eq!(g[1].name, "B");
        assert_eq!(g[1].indices, vec![2]);
        assert!(g[2].name.is_empty());
        assert_eq!(g[2].indices, vec![1]);
    }

    #[test]
    fn replace_group_keeps_position_and_rejects_name_clash() {
        let mut rules = vec![r("A", "a.com"), r("B", "b.com"), r("A", "a2.com")];
        let snap = snapshot_rule_group(&rules, "A").unwrap();
        assert_eq!(snap.patterns, vec!["a.com", "a2.com"]);
        assert!(!snap.mixed_type);

        let incoming = vec![r("A", "new.com")];
        assert_eq!(replace_rule_group(&mut rules, "A", incoming).unwrap(), 1);
        assert_eq!(rules[0].pattern, "new.com");
        assert_eq!(rules[1].group, "B");

        let clash = vec![r("B", "nope.com")];
        assert!(replace_rule_group(&mut rules, "A", clash).is_err());
        assert_eq!(rules.len(), 2);
    }
}

//! The desktop editor and proxy use global rules. Preserve router-specific rows
//! at their original positions when saving a shared configuration.
use sockrocket_core::RoutingRule;

pub fn split(rules: Vec<RoutingRule>) -> (Vec<RoutingRule>, Vec<(usize, RoutingRule)>) {
    let mut global = Vec::new();
    let mut scoped = Vec::new();
    for (index, rule) in rules.into_iter().enumerate() {
        if rule.source.is_empty() {
            global.push(rule);
        } else {
            scoped.push((index, rule));
        }
    }
    (global, scoped)
}

pub fn for_storage(global: &[RoutingRule], scoped: &[(usize, RoutingRule)]) -> Vec<RoutingRule> {
    let mut result = Vec::with_capacity(global.len() + scoped.len());
    let mut remaining = global.iter();
    for (index, rule) in scoped {
        result.extend(
            remaining
                .by_ref()
                .take(index.saturating_sub(result.len()))
                .cloned(),
        );
        result.push(rule.clone());
    }
    result.extend(remaining.cloned());
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use sockrocket_core::parse_batch_rules_in_group;

    #[test]
    fn only_global_rules_are_active_and_router_rows_survive_desktop_edits() {
        let input = "DOMAIN,a.invalid,DIRECT\nSOURCE,192.0.2.5,DOMAIN,b.invalid,REJECT\nDOMAIN,c.invalid,PROXY\nSOURCE,192.0.2.6,DOMAIN,d.invalid,DIRECT";
        let parsed = parse_batch_rules_in_group(input, "proxy", "Media");
        assert!(parsed.errors.is_empty());
        let original = serde_yaml::to_string(&parsed.rules).unwrap();
        let (mut global, scoped) = split(parsed.rules);
        assert_eq!(global.len(), 2);
        assert!(global.iter().all(|r| r.source.is_empty()));
        assert_eq!(
            serde_yaml::to_string(&for_storage(&global, &scoped)).unwrap(),
            original
        );
        global.clear();
        let saved = for_storage(&global, &scoped);
        assert_eq!(saved.len(), 2);
        assert_eq!(saved[0].pattern, "b.invalid");
        assert_eq!(saved[1].pattern, "d.invalid");
    }
}

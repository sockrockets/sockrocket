//! Rule-mode ruleset assembly: user-configured routing rules, falling back
//! to the built-in China-direct ruleset when no custom rule is enabled.
//!
//! When custom rules are present they are evaluated first (by priority), then
//! the built-in China-direct set is appended so domestic direct still works
//! unless the user already provided a `match`/`final` catch-all.

use crate::config::model::RoutingRule;
use crate::router::{MatchRule, RuleSet, china_direct_ruleset};

/// Build the effective ruleset for rule mode.
///
/// Enabled custom rules win (sorted by [`RoutingRule::priority`]); when none
/// are enabled the built-in China-direct ruleset is used so rule mode still
/// behaves sanely. When custom rules exist, China-direct entries are appended
/// after them (skipping a duplicate catch-all if the user already has one).
pub fn rule_mode_ruleset(rules: &[RoutingRule]) -> RuleSet {
    let enabled: Vec<RoutingRule> = rules.iter().filter(|r| r.enabled).cloned().collect();
    if enabled.is_empty() {
        return china_direct_ruleset();
    }

    let mut set = RuleSet::from_config(&enabled);
    let has_final = set
        .rules()
        .iter()
        .any(|e| matches!(e.rule, MatchRule::MatchAll));
    for entry in china_direct_ruleset().rules() {
        if matches!(entry.rule, MatchRule::MatchAll) && has_final {
            continue;
        }
        set.add_rule(entry.rule.clone(), entry.action.clone());
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::{RouteAction, Router};
    use std::sync::Arc;

    #[test]
    fn rule_mode_ruleset_uses_builtin_rules_when_empty() {
        let router = Router::new(rule_mode_ruleset(&[]));
        assert_eq!(router.route("www.baidu.com", 443), RouteAction::Direct);
    }

    #[test]
    fn rule_mode_ruleset_prefers_custom_rules_when_present() {
        let rules = vec![
            RoutingRule {
                rule_type: "domain-suffix".to_string(),
                pattern: "example.com".to_string(),
                target: "reject".to_string(),
                enabled: true,
                priority: 0,
            },
            RoutingRule {
                rule_type: "match".to_string(),
                pattern: "*".to_string(),
                target: "proxy".to_string(),
                enabled: true,
                priority: 0,
            },
        ];

        let router = Router::new(rule_mode_ruleset(&rules));
        assert_eq!(router.route("api.example.com", 443), RouteAction::Reject);
    }

    #[test]
    fn rule_mode_priority_orders_before_list_index() {
        let rules = vec![
            RoutingRule {
                rule_type: "domain-suffix".to_string(),
                pattern: "example.com".to_string(),
                target: "direct".to_string(),
                enabled: true,
                priority: 1,
            },
            RoutingRule {
                rule_type: "domain-suffix".to_string(),
                pattern: "example.com".to_string(),
                target: "reject".to_string(),
                enabled: true,
                priority: 10,
            },
        ];
        let router = Router::new(rule_mode_ruleset(&rules));
        assert_eq!(router.route("www.example.com", 443), RouteAction::Reject);
    }

    #[test]
    fn rule_mode_custom_without_final_keeps_china_domains() {
        let rules = vec![RoutingRule {
            rule_type: "domain-suffix".to_string(),
            pattern: "example.com".to_string(),
            target: "reject".to_string(),
            enabled: true,
            priority: 0,
        }];
        let router = Router::new(rule_mode_ruleset(&rules))
            .with_geoip(Arc::new(crate::router::china_geoip_db()));
        assert_eq!(router.route("www.baidu.com", 443), RouteAction::Direct);
        assert_eq!(router.route("api.example.com", 443), RouteAction::Reject);
    }
}

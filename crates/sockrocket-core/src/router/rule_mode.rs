//! Rule-mode ruleset assembly: user-configured routing rules, falling back
//! to the built-in China-direct ruleset when no custom rule is enabled.

use crate::config::model::RoutingRule;
use crate::router::{RuleSet, china_direct_ruleset};

/// Build the effective ruleset for rule mode.
///
/// Enabled custom rules win; when none are enabled the built-in
/// China-direct ruleset is used so rule mode still behaves sanely.
pub fn rule_mode_ruleset(rules: &[RoutingRule]) -> RuleSet {
    let enabled: Vec<RoutingRule> = rules.iter().filter(|r| r.enabled).cloned().collect();
    if enabled.is_empty() {
        china_direct_ruleset()
    } else {
        RuleSet::from_config(&enabled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::{RouteAction, Router};

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
            },
            RoutingRule {
                rule_type: "match".to_string(),
                pattern: "*".to_string(),
                target: "proxy".to_string(),
                enabled: true,
            },
        ];

        let router = Router::new(rule_mode_ruleset(&rules));
        assert_eq!(router.route("api.example.com", 443), RouteAction::Reject);
    }
}

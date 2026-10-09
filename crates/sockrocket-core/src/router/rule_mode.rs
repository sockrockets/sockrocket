//! Rule-mode ruleset assembly: user-configured routing rules, falling back
//! to the built-in China-direct ruleset when no custom rule is enabled.
//!
//! Order when custom rules exist:
//! 1. User non-`final`/`match` rules (by priority)
//! 2. Built-in China-direct (private / CN domains / `geoip:CN`) — always
//!    before any catch-all, otherwise a user `final → proxy` would shadow
//!    domestic direct and push CN IPs through the proxy
//! 3. User `final`/`match` catch-alls, or a built-in `MatchAll → Proxy`

use crate::config::model::RoutingRule;
use crate::router::{MatchRule, RouteAction, RuleSet, china_direct_ruleset};

fn is_catch_all(rule_type: &str) -> bool {
    matches!(rule_type.to_lowercase().as_str(), "final" | "match")
}

/// Build the effective ruleset for rule mode.
///
/// Enabled custom rules win (sorted by [`RoutingRule::priority`]); when none
/// are enabled the built-in China-direct ruleset is used so rule mode still
/// behaves sanely. Catch-all rules are deferred so built-in `geoip:CN` is
/// never stuck behind `final → proxy`.
pub fn rule_mode_ruleset(rules: &[RoutingRule]) -> RuleSet {
    let enabled: Vec<RoutingRule> = rules.iter().filter(|r| r.enabled).cloned().collect();
    if enabled.is_empty() {
        return china_direct_ruleset();
    }

    let (non_final, finals): (Vec<_>, Vec<_>) = enabled
        .into_iter()
        .partition(|r| !is_catch_all(&r.rule_type));

    let mut set = RuleSet::from_config(&non_final);

    // Built-in China-direct without its catch-all. Placed after user
    // non-final rules so an explicit user `geoip:CN → proxy` (or a CN
    // ip-cidr override) still wins, but before any final.
    for entry in china_direct_ruleset().rules() {
        if matches!(entry.rule, MatchRule::MatchAll) {
            continue;
        }
        set.add_rule(entry.rule.clone(), entry.action.clone());
    }

    if finals.is_empty() {
        set.add_rule(MatchRule::MatchAll, RouteAction::Proxy);
    } else {
        for entry in RuleSet::from_config(&finals).rules() {
            set.add_rule(entry.rule.clone(), entry.action.clone());
        }
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
            RoutingRule { name: String::new(),group: String::new(), rule_type: "domain-suffix".to_string(),
                pattern: "example.com".to_string(),
                target: "reject".to_string(),
                enabled: true,
                priority: 0,
            },
            RoutingRule { name: String::new(),group: String::new(), rule_type: "match".to_string(),
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
            RoutingRule { name: String::new(),group: String::new(), rule_type: "domain-suffix".to_string(),
                pattern: "example.com".to_string(),
                target: "direct".to_string(),
                enabled: true,
                priority: 1,
            },
            RoutingRule { name: String::new(),group: String::new(), rule_type: "domain-suffix".to_string(),
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
        let rules = vec![RoutingRule { name: String::new(),group: String::new(), rule_type: "domain-suffix".to_string(),
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

    /// Regression: a lone `final → proxy` used to be compiled *before* the
    /// built-in geoip:CN append, so every CN IP fell through to proxy.
    #[test]
    fn rule_mode_user_final_does_not_shadow_geoip_cn() {
        let rules = vec![RoutingRule {
            name: String::new(),
            group: String::new(),
            rule_type: "final".to_string(),
            pattern: "*".to_string(),
            target: "proxy".to_string(),
            enabled: true,
            priority: 0,
        }];
        let router = Router::new(rule_mode_ruleset(&rules))
            .with_geoip(Arc::new(crate::router::china_geoip_db()));
        assert_eq!(router.route("114.114.114.114", 443), RouteAction::Direct);
        assert_eq!(router.route("8.8.8.8", 443), RouteAction::Proxy);
        assert_eq!(router.route("www.google.com", 443), RouteAction::Proxy);
    }

    #[test]
    fn rule_mode_user_can_still_force_proxy_a_cn_cidr() {
        let rules = vec![
            RoutingRule {
                name: String::new(),
                group: String::new(),
                rule_type: "ip-cidr".to_string(),
                pattern: "114.114.114.114/32".to_string(),
                target: "proxy".to_string(),
                enabled: true,
                priority: 100,
            },
            RoutingRule {
                name: String::new(),
                group: String::new(),
                rule_type: "final".to_string(),
                pattern: "*".to_string(),
                target: "proxy".to_string(),
                enabled: true,
                priority: 0,
            },
        ];
        let router = Router::new(rule_mode_ruleset(&rules))
            .with_geoip(Arc::new(crate::router::china_geoip_db()));
        assert_eq!(router.route("114.114.114.114", 443), RouteAction::Proxy);
        assert_eq!(router.route("223.5.5.5", 443), RouteAction::Direct);
    }
}

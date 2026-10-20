//! Rule-mode ruleset assembly: user-configured routing rules, falling back
//! to the built-in China-direct ruleset when no custom rule is enabled.
//!
//! Order when custom rules exist:
//! 1. Device non-final rules, then device final rules (source match required).
//! 2. Global non-final rules.
//! 3. Built-in China-direct (including known domestic TUN destinations).
//! 4. Global final rules, or a built-in MatchAll → Proxy fallback.
//!
//! Priorities descend within each phase; ties keep configuration order.

use crate::config::model::RoutingRule;
use crate::router::{MatchRule, RouteAction, RuleSet, china_direct_ruleset};

fn is_catch_all(rule_type: &str) -> bool {
    matches!(rule_type.to_lowercase().as_str(), "final" | "match")
}

/// Build the effective ruleset for rule mode.
///
/// Enabled custom rules win (sorted by [`RoutingRule::priority`]); when none
/// are enabled the built-in China-direct ruleset is used so rule mode still
/// behaves sanely. Global catch-all rules are deferred until after China-direct;
/// device catch-alls precede global rules but only match their own clients.
pub fn rule_mode_ruleset(rules: &[RoutingRule]) -> RuleSet {
    let enabled: Vec<RoutingRule> = rules.iter().filter(|r| r.enabled).cloned().collect();
    if enabled.is_empty() {
        return china_direct_ruleset();
    }

    let (devices, enabled): (Vec<_>, Vec<_>) =
        enabled.into_iter().partition(|r| !r.source.is_empty());
    let (device_rules, device_finals): (Vec<_>, Vec<_>) = devices
        .into_iter()
        .partition(|r| !is_catch_all(&r.rule_type));
    let (non_final, finals): (Vec<_>, Vec<_>) = enabled
        .into_iter()
        .partition(|r| !is_catch_all(&r.rule_type));

    let mut set = RuleSet::from_config(&device_rules);
    set.extend(RuleSet::from_config(&device_finals));
    set.extend(RuleSet::from_config(&non_final));

    // Built-in China-direct without its catch-all. Placed after user
    // non-final rules so an explicit user `geoip:CN → proxy` (or a CN
    // ip-cidr override) still wins, but before any final.
    set.add_rule(MatchRule::DomesticHint, RouteAction::Direct);
    for entry in china_direct_ruleset().rules() {
        if matches!(entry.rule, MatchRule::MatchAll) {
            continue;
        }
        set.add_rule(entry.rule.clone(), entry.action.clone());
    }

    if finals.is_empty() {
        set.add_rule(MatchRule::MatchAll, RouteAction::Proxy);
    } else {
        set.extend(RuleSet::from_config(&finals));
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
                source: String::new(),
                name: String::new(),
                group: String::new(),
                rule_type: "domain-suffix".to_string(),
                pattern: "example.com".to_string(),
                target: "reject".to_string(),
                enabled: true,
                priority: 0,
            },
            RoutingRule {
                source: String::new(),
                name: String::new(),
                group: String::new(),
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
                source: String::new(),
                name: String::new(),
                group: String::new(),
                rule_type: "domain-suffix".to_string(),
                pattern: "example.com".to_string(),
                target: "direct".to_string(),
                enabled: true,
                priority: 1,
            },
            RoutingRule {
                source: String::new(),
                name: String::new(),
                group: String::new(),
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
            source: String::new(),
            name: String::new(),
            group: String::new(),
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

    /// Regression: a lone `final → proxy` used to be compiled *before* the
    /// built-in geoip:CN append, so every CN IP fell through to proxy.
    #[test]
    fn rule_mode_user_final_does_not_shadow_geoip_cn() {
        let rules = vec![RoutingRule {
            source: String::new(),
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
                source: String::new(),
                name: String::new(),
                group: String::new(),
                rule_type: "ip-cidr".to_string(),
                pattern: "114.114.114.114/32".to_string(),
                target: "proxy".to_string(),
                enabled: true,
                priority: 100,
            },
            RoutingRule {
                source: String::new(),
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

#[cfg(test)]
mod device_tests {
    use super::*;
    use crate::proxy::routing::{client_source, with_client_source, with_tun_destination};
    use crate::router::{Router, normalize_rule_source};
    use std::sync::Arc;
    fn rule(source: &str, typ: &str, pattern: &str, target: &str, priority: i32) -> RoutingRule {
        RoutingRule {
            source: normalize_rule_source(source).unwrap(),
            name: String::new(),
            group: String::new(),
            rule_type: typ.into(),
            pattern: pattern.into(),
            target: target.into(),
            priority,
            enabled: true,
        }
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn same_destination_is_isolated_between_concurrent_devices_and_global_fallback() {
        let router = Arc::new(Router::new(rule_mode_ruleset(&[
            rule(
                "192.0.2.10",
                "domain-suffix",
                "example.invalid",
                "reject",
                0,
            ),
            rule(
                "192.0.2.20",
                "domain-suffix",
                "example.invalid",
                "direct",
                0,
            ),
            rule("", "final", "*", "proxy", 999),
        ])));
        let mut tasks = Vec::new();
        for (source, want) in [
            ("192.0.2.10", RouteAction::Reject),
            ("192.0.2.20", RouteAction::Direct),
            ("192.0.2.30", RouteAction::Proxy),
        ] {
            let router = router.clone();
            tasks.push(tokio::spawn(async move {
                with_client_source(Some(source.parse().unwrap()), async move {
                    for _ in 0..64 {
                        assert_eq!(router.route("video.example.invalid", 443), want);
                        tokio::task::yield_now().await;
                        assert_eq!(
                            router.route_resolving("video.example.invalid", 443).await,
                            want
                        );
                    }
                })
                .await;
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        assert_eq!(client_source(), None);
        assert_eq!(
            router.route("video.example.invalid", 443),
            RouteAction::Proxy
        );
    }
    #[tokio::test]
    async fn device_catch_all_overrides_domestic_defaults_without_affecting_other_devices() {
        let router = Router::new(rule_mode_ruleset(&[
            rule(
                "2001:db8:1::/64",
                "domain-suffix",
                "safe.invalid",
                "direct",
                20,
            ),
            rule("2001:db8:1::/64", "final", "*", "reject", 0),
            rule("192.0.2.0/24", "final", "*", "proxy", 0),
        ]));
        with_client_source(Some("2001:db8:1::10".parse().unwrap()), async {
            assert_eq!(router.route("safe.invalid", 443), RouteAction::Direct);
            assert_eq!(router.route("www.baidu.com", 443), RouteAction::Reject);
        })
        .await;
        with_client_source(Some("192.0.2.10".parse().unwrap()), async {
            assert_eq!(router.route("www.baidu.com", 443), RouteAction::Proxy);
        })
        .await;
        assert_eq!(router.route("www.baidu.com", 443), RouteAction::Direct);
    }
    #[tokio::test]
    async fn tun_original_destination_and_domestic_hint_survive_device_routing() {
        let router = Router::new(rule_mode_ruleset(&[
            rule("192.0.2.10", "ip-cidr", "203.0.113.0/24", "reject", 10),
            rule("192.0.2.10", "domain-suffix", "foreign.invalid", "proxy", 0),
        ]));
        with_client_source(Some("192.0.2.10".parse().unwrap()), async {
            with_tun_destination(Some("203.0.113.9".parse().unwrap()), true, async {
                assert_eq!(
                    router.route_resolving("cdn.invalid", 443).await,
                    RouteAction::Reject
                );
            })
            .await;
            with_tun_destination(Some("198.51.100.9".parse().unwrap()), true, async {
                assert_eq!(
                    router.route_resolving("cdn.invalid", 443).await,
                    RouteAction::Direct
                );
                assert_eq!(
                    router.route_resolving("foreign.invalid", 443).await,
                    RouteAction::Proxy
                );
            })
            .await;
        })
        .await;
    }
    #[test]
    fn source_persists_canonically_and_invalid_configuration_is_rejected() {
        let row = rule("192.0.2.199/24", "final", "*", "direct", 0);
        let yaml = serde_yaml::to_string(&row).unwrap();
        assert_eq!(
            serde_yaml::from_str::<RoutingRule>(&yaml).unwrap().source,
            "192.0.2.0/24"
        );
        for bad in [
            "192.0.2.1/33",
            "2001:db8::/129",
            "tv.local",
            "aa:bb:cc:dd:ee:ff",
        ] {
            let mut value = serde_json::to_value(&row).unwrap();
            value["source"] = bad.into();
            assert!(serde_json::from_value::<RoutingRule>(value).is_err());
        }
        assert_eq!(
            normalize_rule_source("::ffff:192.0.2.10").unwrap(),
            "192.0.2.10/32"
        );
        assert_eq!(
            normalize_rule_source("2001:DB8:1::1234/64").unwrap(),
            "2001:db8:1::/64"
        );
        let mut value = serde_json::to_value(&row).unwrap();
        value.as_object_mut().unwrap().remove("source");
        assert!(
            serde_json::from_value::<RoutingRule>(value)
                .unwrap()
                .source
                .is_empty()
        );
    }
    #[test]
    fn native_export_and_batch_merge_never_drop_device_scope() {
        let original = vec![
            rule(
                "192.0.2.10",
                "domain-suffix",
                "example.invalid",
                "reject",
                0,
            ),
            rule(
                "192.0.2.20",
                "domain-suffix",
                "example.invalid",
                "direct",
                0,
            ),
        ];
        let text = crate::format_rules_export(&original);
        let parsed = crate::parse_batch_rules(&text, "proxy");
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let mut merged = Vec::new();
        crate::merge_batch_rules(&mut merged, parsed.rules, crate::BatchConflict::Skip);
        assert_eq!(merged.len(), 2);
        assert_ne!(merged[0].source, merged[1].source);
        assert!(text.contains("SOURCE,192.0.2.10/32,"));
    }
}

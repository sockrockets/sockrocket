//! On-demand explanation of saved routing policy. Does not use/update the
//! live route cache, open proxy connections, change rules, or log queries.
use super::*;

#[derive(Debug, serde::Serialize)]
pub struct RouteTraceStep {
    pub config_index: Option<usize>,
    pub rule_type: &'static str,
    pub pattern: String,
    pub source: Option<String>,
    pub action: RouteAction,
    pub outcome: &'static str,
    pub checked: usize,
}

#[derive(Debug, serde::Serialize)]
pub struct RouteTrace {
    pub action: RouteAction,
    pub matched_rule: Option<usize>,
    pub matched_builtin: bool,
    pub resolved_ip: Option<IpAddr>,
    pub dns: &'static str,
    pub definitive: bool,
    pub steps: Vec<RouteTraceStep>,
    pub omitted_steps: usize,
    pub checked_rules: usize,
}

impl Router {
    /// Same compiled order, source matching, destination matcher and lazy DNS
    /// resolver as routing. Optional IP reproduces a caller-observed address;
    /// no TUN-only domestic mapping hint or live-connection cache is assumed.
    pub async fn trace_route(
        &self,
        host: &str,
        port: u16,
        source: Option<IpAddr>,
        destination_ip: Option<IpAddr>,
        resolve_dns: bool,
    ) -> RouteTrace {
        let source = source.map(|ip| match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
            _ => ip,
        });
        let literal = host.parse::<IpAddr>().ok();
        let mut ip = literal.or(destination_ip);
        let mut attempted = ip.is_some();
        let mut trace = RouteTrace {
            action: self.rules.default_action.clone(),
            matched_rule: None,
            matched_builtin: false,
            resolved_ip: ip,
            dns: if literal.is_some() {
                "literal"
            } else if ip.is_some() {
                "provided"
            } else {
                "not_needed"
            },
            definitive: true,
            steps: Vec::new(),
            omitted_steps: 0,
            checked_rules: 0,
        };
        let lower = host.to_lowercase();
        let mut builtin_misses = 0;
        let mut builtin_needs_ip = false;
        for entry in &self.rules.rules {
            let source_ok = source_matches(entry.source, source);
            let needs_ip = matches!(entry.rule, MatchRule::IpCidr { .. } | MatchRule::GeoIp(_));
            if source_ok && needs_ip && !attempted {
                attempted = true;
                if resolve_dns {
                    ip = resolve_host(&lower).await;
                    trace.resolved_ip = ip;
                    trace.dns = if ip.is_some() { "resolved" } else { "failed" };
                } else {
                    trace.dns = "skipped";
                }
            }
            let outcome = if !source_ok {
                "source_mismatch"
            } else if needs_ip && ip.is_none() {
                trace.definitive = false;
                "needs_ip"
            } else if !matches!(entry.rule, MatchRule::DomesticHint)
                && self.rule_matches(&entry.rule, &lower, ip, port)
            {
                "matched"
            } else {
                "not_matched"
            };
            trace.checked_rules += 1;
            // Summarize contiguous built-in misses instead of returning the
            // entire China-domain database to a small router/browser.
            if entry.config_index.is_none() && outcome != "matched" {
                builtin_misses += 1;
                builtin_needs_ip |= outcome == "needs_ip";
                continue;
            }
            if builtin_misses > 0 {
                push_builtin(&mut trace, builtin_misses, builtin_needs_ip);
                builtin_misses = 0;
                builtin_needs_ip = false;
            }
            if trace.steps.len() < 256 || outcome == "matched" {
                trace.steps.push(step(entry, outcome));
            } else {
                trace.omitted_steps += 1;
            }
            if outcome == "matched" {
                trace.action = entry.action.clone();
                trace.matched_rule = entry.config_index;
                trace.matched_builtin = entry.config_index.is_none();
                break;
            }
        }
        if builtin_misses > 0 {
            push_builtin(&mut trace, builtin_misses, builtin_needs_ip);
        }
        trace
    }
}

fn push_builtin(trace: &mut RouteTrace, checked: usize, needs_ip: bool) {
    if trace.steps.len() < 256 {
        trace.steps.push(RouteTraceStep {
            config_index: None,
            rule_type: "builtin",
            pattern: String::new(),
            source: None,
            action: RouteAction::Direct,
            outcome: if needs_ip { "needs_ip" } else { "not_matched" },
            checked,
        });
    } else {
        trace.omitted_steps += checked;
    }
}

fn step(entry: &RuleEntry, outcome: &'static str) -> RouteTraceStep {
    let (rule_type, pattern) = match &entry.rule {
        MatchRule::Domain(p) => ("domain", p.clone()),
        MatchRule::DomainSuffix(p) => ("domain-suffix", p.trim_start_matches('.').into()),
        MatchRule::DomainKeyword(p) => ("domain-keyword", p.clone()),
        MatchRule::IpCidr { addr, prefix_len } => ("ip-cidr", format!("{addr}/{prefix_len}")),
        MatchRule::GeoIp(p) => ("geoip", p.clone()),
        MatchRule::DstPort { min, max } => (
            "dst-port",
            if min == max {
                min.to_string()
            } else {
                format!("{min}-{max}")
            },
        ),
        MatchRule::MatchAll => ("final", "*".into()),
        MatchRule::DomesticHint => ("domestic-hint", String::new()),
    };
    RouteTraceStep {
        config_index: entry.config_index,
        rule_type,
        pattern,
        source: entry.source.map(|(ip, prefix)| format!("{ip}/{prefix}")),
        action: entry.action.clone(),
        outcome,
        checked: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::{china_geoip_db, rule_mode_ruleset};

    fn rule(kind: &str, pattern: &str, target: &str, source: &str, priority: i32) -> RoutingRule {
        RoutingRule {
            rule_type: kind.into(),
            pattern: pattern.into(),
            target: target.into(),
            source: source.into(),
            priority,
            enabled: true,
            name: String::new(),
            group: String::new(),
        }
    }

    #[tokio::test]
    async fn trace_preserves_original_indices_and_device_phase_order() {
        let mut rules = vec![rule("domain", "example.com", "reject", "", 999)];
        rules.extend((0..7).map(|_| rule("domain", "other.invalid", "reject", "", 0)));
        rules.push(rule("dst-port", "443", "proxy", "192.0.2.0/24", 5));
        rules.push(rule("final", "*", "direct", "192.0.2.0/24", 999));
        let router = Router::new(rule_mode_ruleset(&rules));
        let trace = router
            .trace_route(
                "example.com",
                443,
                Some("::ffff:192.0.2.5".parse().unwrap()),
                None,
                false,
            )
            .await;
        assert_eq!(trace.matched_rule, Some(8));
        assert_eq!(trace.action, RouteAction::Proxy);
        assert!(trace.definitive);
        assert_eq!(trace.dns, "not_needed");
        assert!(
            router.cache.read().unwrap().is_empty(),
            "explanation must not modify live decisions"
        );
        let trace = router
            .trace_route(
                "example.com",
                80,
                Some("192.0.2.5".parse().unwrap()),
                None,
                false,
            )
            .await;
        assert_eq!(
            trace.matched_rule,
            Some(0),
            "global destination rules precede a device final"
        );
        assert_eq!(trace.action, RouteAction::Reject);
        let trace = router
            .trace_route("example.com", 443, None, None, false)
            .await;
        assert_eq!(trace.matched_rule, Some(0));
    }

    #[tokio::test]
    async fn trace_uses_real_cidr_geoip_and_ipv6_matchers() {
        let rules = vec![
            rule("ip-cidr", "2001:db8::/32", "reject", "2001:db8:1::/48", 10),
            rule("geoip", "CN", "direct", "", 0),
            rule("final", "*", "proxy", "", 0),
        ];
        let router = Router::new(rule_mode_ruleset(&rules)).with_geoip(Arc::new(china_geoip_db()));
        let trace = router
            .trace_route(
                "example.com",
                443,
                Some("2001:db8:1::5".parse().unwrap()),
                Some("2001:db8::9".parse().unwrap()),
                false,
            )
            .await;
        assert_eq!(trace.matched_rule, Some(0));
        assert_eq!(trace.dns, "provided");
        let trace = router
            .trace_route("223.5.5.5", 443, None, None, false)
            .await;
        assert_eq!(trace.matched_rule, Some(1));
        assert_eq!(trace.action, router.route("223.5.5.5", 443));
        assert!(trace.definitive);
        let trace = router
            .trace_route("example.com", 443, None, None, false)
            .await;
        assert!(
            !trace.definitive,
            "missing IP must not become a conclusive proxy result"
        );
        assert_eq!(trace.dns, "skipped");
        assert!(trace.steps.iter().any(|s| s.outcome == "needs_ip"));
    }

    #[tokio::test]
    async fn trace_keeps_winner_beyond_display_limit_and_disabled_indices() {
        let mut rules: Vec<_> = (0..300)
            .map(|_| rule("domain", "other.invalid", "reject", "", 0))
            .collect();
        rules[0].enabled = false;
        rules.push(rule("domain", "example.com", "direct", "", 0));
        rules.push(rule("domain", "example.com", "reject", "", 0));
        let router = Router::new(rule_mode_ruleset(&rules));
        let trace = router
            .trace_route("example.com", 443, None, None, false)
            .await;
        assert_eq!(trace.matched_rule, Some(300));
        assert_eq!(trace.action, router.route("example.com", 443));
        assert_eq!(trace.steps.last().unwrap().outcome, "matched");
        assert!(trace.steps.len() <= 257);
        assert_eq!(trace.omitted_steps, 43);
    }

    #[tokio::test]
    async fn builtin_domestic_rules_precede_global_final() {
        let rules = vec![rule("final", "*", "reject", "", 999)];
        let router = Router::new(rule_mode_ruleset(&rules));
        let trace = router
            .trace_route("www.baidu.com", 443, None, None, false)
            .await;
        assert_eq!(trace.action, RouteAction::Direct);
        assert_eq!(trace.action, router.route("www.baidu.com", 443));
        assert!(trace.matched_builtin);
        assert!(
            !trace.definitive,
            "earlier IP rules remain unverified without DNS"
        );
        let trace = router
            .trace_route(
                "example.invalid",
                443,
                None,
                Some("203.0.113.1".parse().unwrap()),
                false,
            )
            .await;
        assert_eq!(trace.matched_rule, Some(0));
        assert!(trace.definitive);
    }
}

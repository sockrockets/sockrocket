//! Optional scene templates — premade rule batches users can apply.
//!
//! Templates are generic (not app-branded). They prepend high-priority rules
//! into the user's exception list so they beat the built-in China-direct layer.

use crate::config::model::RoutingRule;

/// A reusable rule pack the UI can one-click apply.
#[derive(Debug, Clone, Copy)]
pub struct RuleScene {
    pub id: &'static str,
    /// Stable i18n key suffix, e.g. `rules.scene.cross_border.title`.
    pub title_key: &'static str,
    pub desc_key: &'static str,
    /// Base priority assigned to every rule in the pack (list order still
    /// tie-breaks). High enough to stay above typical user priority `0`.
    pub base_priority: i32,
    build: fn() -> Vec<RoutingRule>,
}

impl RuleScene {
    pub fn rules(&self) -> Vec<RoutingRule> {
        let mut rules = (self.build)();
        for r in &mut rules {
            r.priority = self.base_priority;
            if r.group.is_empty() {
                r.group = self.id.to_string();
            }
        }
        rules
    }

    /// Same as [`Self::rules`] but force every rule into `group`.
    pub fn rules_in_group(&self, group: &str) -> Vec<RoutingRule> {
        let mut rules = self.rules();
        let g = group.trim();
        if !g.is_empty() {
            for r in &mut rules {
                r.group = g.to_string();
            }
        }
        rules
    }
}

/// Built-in scenes shown in GUI / Merlin.
pub fn builtin_rule_scenes() -> &'static [RuleScene] {
    &SCENES
}

const SCENES: [RuleScene; 2] = [
    RuleScene {
        id: "cross_border",
        title_key: "rules.scene.cross_border.title",
        desc_key: "rules.scene.cross_border.desc",
        base_priority: 100,
        build: scene_cross_border,
    },
    RuleScene {
        id: "domestic_direct",
        title_key: "rules.scene.domestic_direct.title",
        desc_key: "rules.scene.domestic_direct.desc",
        base_priority: 80,
        build: scene_domestic_direct,
    },
];

fn rule(rule_type: &str, pattern: &str, target: &str) -> RoutingRule {
    RoutingRule {
        name: String::new(),
        group: String::new(),
        rule_type: rule_type.into(),
        pattern: pattern.into(),
        target: target.into(),
        enabled: true,
        priority: 0,
    }
}

/// Cross-border hybrid apps: force common overseas SaaS / CDN suffixes via
/// proxy so they are not swallowed by China-direct when resolved to CN edges.
/// Users should edit / extend with their app's own domains & IPs via batch.
fn scene_cross_border() -> Vec<RoutingRule> {
    vec![
        rule("domain-suffix", "googleapis.com", "proxy"),
        rule("domain-suffix", "gstatic.com", "proxy"),
        rule("domain-suffix", "google.com", "proxy"),
        rule("domain-suffix", "cloudflare.com", "proxy"),
        rule("domain-suffix", "cloudflare.net", "proxy"),
        rule("domain-suffix", "amazonaws.com", "proxy"),
        rule("domain-suffix", "apple.com", "proxy"),
        rule("domain-suffix", "icloud.com", "proxy"),
        rule("domain-suffix", "microsoft.com", "proxy"),
        rule("domain-suffix", "office.com", "proxy"),
        rule("domain-suffix", "live.com", "proxy"),
        rule("domain-suffix", "github.com", "proxy"),
        rule("domain-suffix", "githubusercontent.com", "proxy"),
        rule("domain-suffix", "openai.com", "proxy"),
        rule("domain-suffix", "chatgpt.com", "proxy"),
    ]
}

/// Domestic apps that often break when forced through a proxy.
fn scene_domestic_direct() -> Vec<RoutingRule> {
    vec![
        // Explicit geoip:CN (also covered by the built-in China-direct layer).
        rule("geoip", "CN", "direct"),
        rule("domain-suffix", "qq.com", "direct"),
        rule("domain-suffix", "weixin.qq.com", "direct"),
        rule("domain-suffix", "wechat.com", "direct"),
        rule("domain-suffix", "tenpay.com", "direct"),
        rule("domain-suffix", "alipay.com", "direct"),
        rule("domain-suffix", "alipayobjects.com", "direct"),
        rule("domain-suffix", "taobao.com", "direct"),
        rule("domain-suffix", "tmall.com", "direct"),
        rule("domain-suffix", "aliyun.com", "direct"),
        rule("domain-suffix", "alicdn.com", "direct"),
        rule("domain-suffix", "jd.com", "direct"),
        rule("domain-suffix", "360buyimg.com", "direct"),
        rule("domain-suffix", "bilibili.com", "direct"),
        rule("domain-suffix", "hdslb.com", "direct"),
        rule("domain-suffix", "163.com", "direct"),
        rule("domain-suffix", "126.com", "direct"),
        rule("domain-suffix", "netease.com", "direct"),
        rule("domain-suffix", "baidu.com", "direct"),
        rule("domain-suffix", "bdstatic.com", "direct"),
        rule("domain-keyword", "weixin", "direct"),
        rule("domain-keyword", "alipay", "direct"),
    ]
}

pub fn rule_scene_by_id(id: &str) -> Option<&'static RuleScene> {
    builtin_rule_scenes().iter().find(|s| s.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenes_assign_priority() {
        let s = rule_scene_by_id("cross_border").unwrap();
        let rules = s.rules();
        assert!(!rules.is_empty());
        assert!(rules.iter().all(|r| r.priority == 100));
        assert!(rules.iter().all(|r| r.target == "proxy"));
    }

    #[test]
    fn domestic_scene_is_direct() {
        let rules = rule_scene_by_id("domestic_direct").unwrap().rules();
        assert!(rules.iter().all(|r| r.target == "direct"));
    }
}

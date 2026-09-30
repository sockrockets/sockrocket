//! Built-in routing rules for China-direct / overseas-proxy mode.
//!
//! These rules cover common China domains plus geoip:CN (expanded CIDR DB) so that
//! domestic traffic goes direct while international traffic uses the proxy.

use super::engine::{MatchRule, RouteAction, RuleSet};
use super::geoip::GeoIpDb;

/// Build a [`RuleSet`] with built-in China-direct rules.
///
/// Order:
/// 1. Private / LAN IPs → Direct
/// 2. Common China domain suffixes → Direct
/// 3. Common China domain keywords → Direct
/// 4. GeoIP CN (expanded built-in CIDR DB) → Direct
/// 5. Catch-all → Proxy
pub fn china_direct_ruleset() -> RuleSet {
    let mut rs = RuleSet::new();

    // --- Private / LAN ---
    for cidr in PRIVATE_CIDRS {
        if let Some((addr, prefix_len)) = parse_cidr(cidr) {
            rs.add_rule(MatchRule::IpCidr { addr, prefix_len }, RouteAction::Direct);
        }
    }

    // --- China domain suffixes ---
    for suffix in CHINA_DOMAIN_SUFFIXES {
        let s = if suffix.starts_with('.') {
            suffix.to_string()
        } else {
            format!(".{}", suffix)
        };
        rs.add_rule(MatchRule::DomainSuffix(s), RouteAction::Direct);
    }

    // --- China domain keywords ---
    for kw in CHINA_DOMAIN_KEYWORDS {
        rs.add_rule(
            MatchRule::DomainKeyword(kw.to_string()),
            RouteAction::Direct,
        );
    }

    // --- China via GeoIP (Router must be built with china_geoip_db()) ---
    rs.add_rule(MatchRule::GeoIp("CN".into()), RouteAction::Direct);

    // --- Catch-all: proxy ---
    rs.add_rule(MatchRule::MatchAll, RouteAction::Proxy);

    rs
}

/// Build a [`GeoIpDb`] pre-loaded with private ranges + major China CIDRs.
pub fn china_geoip_db() -> GeoIpDb {
    let mut db = GeoIpDb::builtin(); // private ranges already included

    for cidr in CHINA_IP_CIDRS {
        db.add_entry(cidr, "CN");
    }

    db.build();
    db
}

/// The curated China IPv4 CIDR list backing `geoip:CN` — exposed so the
/// router can load the same ranges into a kernel ipset for hardware-NAT
/// direct routing (single source of truth for both layers).
pub fn china_ipv4_cidrs() -> Vec<&'static str> {
    CHINA_IP_CIDRS
        .iter()
        .copied()
        .filter(|c| !c.contains(':'))
        .collect()
}

fn parse_cidr(cidr: &str) -> Option<(std::net::IpAddr, u8)> {
    let parts: Vec<&str> = cidr.splitn(2, '/').collect();
    if parts.len() != 2 {
        return None;
    }
    let addr: std::net::IpAddr = parts[0].parse().ok()?;
    let prefix_len: u8 = parts[1].parse().ok()?;
    Some((addr, prefix_len))
}

// ---- Static data ----

const PRIVATE_CIDRS: &[&str] = &[
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "127.0.0.0/8",
    "169.254.0.0/16",
    "100.64.0.0/10",
    "::1/128",
    "fc00::/7",
    "fe80::/10",
];

/// Common China top-level and service domain suffixes.
const CHINA_DOMAIN_SUFFIXES: &[&str] = &[
    // TLDs (IDN TLDs in A-label/punycode form, matching DNS wire format)
    "cn",
    "xn--fiqs8s", // .xn--fiqs8s (China IDN TLD)
    "xn--55qx5d", // .xn--55qx5d (company IDN TLD)
    "xn--io0a7i", // .xn--io0a7i (network IDN TLD)
    // Major services
    "baidu.com",
    "bdstatic.com",
    "bdimg.com",
    "bcebos.com",
    "baidupcs.com",
    "qq.com",
    "gtimg.cn",
    "qpic.cn",
    "qlogo.cn",
    "weixin.qq.com",
    "wechat.com",
    "tencent.com",
    "tencent-cloud.net",
    "tencentcs.com",
    "myqcloud.com",
    "qcloud.com",
    "alibaba.com",
    "alibabacloud.com",
    "taobao.com",
    "tmall.com",
    "alipay.com",
    "aliyun.com",
    "aliyuncs.com",
    "alicdn.com",
    "amap.com",
    "dingtalk.com",
    "jd.com",
    "jd.hk",
    "360buyimg.com",
    "163.com",
    "126.com",
    "126.net",
    "netease.com",
    "yeah.net",
    "ydstatic.com",
    "weibo.com",
    "weibo.cn",
    "sinaimg.cn",
    "sina.com.cn",
    "sina.cn",
    "douyin.com",
    "tiktokv.com",
    "bytedance.com",
    "bytecdn.cn",
    "bytegoofy.com",
    "snssdk.com",
    "pstatp.com",
    "toutiao.com",
    "ixigua.com",
    "huoshan.com",
    "bilibili.com",
    "bilivideo.com",
    "biliapi.com",
    "hdslb.com",
    "acgvideo.com",
    "zhihu.com",
    "zhimg.com",
    "douban.com",
    "doubanio.com",
    "xiaomi.com",
    "mi.com",
    "miui.com",
    "xiaomiev.com",
    "huawei.com",
    "hicloud.com",
    "vmall.com",
    "oppo.com",
    "vivo.com",
    "meizu.com",
    "ifeng.com",
    "sohu.com",
    "sogou.com",
    "sogo.com",
    "suning.com",
    "pinduoduo.com",
    "meituan.com",
    "dianping.com",
    "ctrip.com",
    "qunar.com",
    "eleme.cn",
    "youku.com",
    "tudou.com",
    "iqiyi.com",
    "qiyipic.com",
    "iqiyipic.com",
    "le.com",
    "kuaishou.com",
    "kspkg.com",
    "csdn.net",
    "jianshu.com",
    "cnblogs.com",
    "oschina.net",
    "gitee.com",
    "coding.net",
    "zhangmen.com",
    "youdao.com",
    "microsoft.cn",
    "msn.cn",
    "windows.cn",
    "apple.com.cn",
    "icloud.com.cn",
    "ximalaya.com",
    "lixinger.com",
    "eastmoney.com",
    "10jqka.com.cn",
    "cnki.net",
    "gov.cn",
    "edu.cn",
    "mil.cn",
    "org.cn",
    "ac.cn",
    "bj.cn",
    "sh.cn",
    "tj.cn",
    "cq.cn",
    "zj.cn",
    "js.cn",
    "gd.cn",
];

/// Keywords that indicate China-specific traffic.
const CHINA_DOMAIN_KEYWORDS: &[&str] = &["chinaz", "cnzz", "umeng"];

/// Major China IP CIDR ranges (aggregated blocks from APNIC for CN).
/// This is a curated subset covering the largest allocations.
/// Built-in China IPv4 CIDRs for `geoip:CN` and Merlin `ipset`.
/// Generated by `scripts/update-china-cidrs.sh` from 17mon/china_ip_list —
/// do not edit by hand. Expand/refresh with that script.
const CHINA_IP_CIDRS: &[&str] =
    &include!(concat!(env!("CARGO_MANIFEST_DIR"), "/data/china_ipv4_cidrs.inc.rs"));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn china_ruleset_has_rules() {
        let rs = china_direct_ruleset();
        assert!(rs.rules().len() > 100, "should have many rules");
    }

    #[test]
    fn china_ruleset_routes_cn_domains_direct() {
        let rs = china_direct_ruleset();
        let router = crate::router::Router::new(rs);

        assert_eq!(
            router.route("www.baidu.com", 443),
            RouteAction::Direct,
            "baidu.com should be direct"
        );
        assert_eq!(
            router.route("www.qq.com", 443),
            RouteAction::Direct,
            "qq.com should be direct"
        );
        assert_eq!(
            router.route("example.cn", 80),
            RouteAction::Direct,
            ".cn should be direct"
        );
    }

    #[test]
    fn china_ruleset_routes_foreign_domains_proxy() {
        let rs = china_direct_ruleset();
        let router = crate::router::Router::new(rs);

        assert_eq!(
            router.route("www.google.com", 443),
            RouteAction::Proxy,
            "google.com should be proxy"
        );
        assert_eq!(
            router.route("www.youtube.com", 443),
            RouteAction::Proxy,
            "youtube.com should be proxy"
        );
    }

    #[test]
    fn china_ruleset_routes_private_ip_direct() {
        let rs = china_direct_ruleset();
        let router = crate::router::Router::new(rs);

        assert_eq!(
            router.route("192.168.1.1", 80),
            RouteAction::Direct,
            "LAN should be direct"
        );
        assert_eq!(
            router.route("10.0.0.1", 80),
            RouteAction::Direct,
            "10.x should be direct"
        );
    }

    #[test]
    fn china_geoip_has_entries() {
        let db = china_geoip_db();
        assert!(
            db.entry_count() > 1000,
            "expanded CN list should be large, got {}",
            db.entry_count()
        );
        // Spot-check a well-known CN allocation (China Telecom 1.0.1.0/24).
        assert_eq!(
            db.lookup("1.0.1.1".parse().unwrap()),
            Some("CN"),
            "1.0.1.1 should map to CN"
        );
    }

    #[test]
    fn china_ruleset_geoip_cn_with_db() {
        let rs = china_direct_ruleset();
        let router = crate::router::Router::new(rs).with_geoip(std::sync::Arc::new(china_geoip_db()));
        assert_eq!(router.route("1.0.1.1", 80), RouteAction::Direct);
        assert_eq!(router.route("8.8.8.8", 80), RouteAction::Proxy);
    }
}

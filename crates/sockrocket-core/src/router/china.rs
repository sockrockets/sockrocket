//! Built-in routing rules for China-direct / overseas-proxy mode.
//!
//! These rules cover common China domains plus geoip:CN (expanded CIDR DB) so that
//! domestic traffic goes direct while international traffic uses the proxy.

use super::engine::{MatchRule, RouteAction, RuleSet};
use super::geoip::{GeoIpDb, private_networks};

/// Curated China domain suffixes shared by routing (`geoip`/domain Direct)
/// and Merlin split-DNS (Fallback / no fake-IP). Keep this the single list —
/// a shorter DNS-only copy used to miss CDN hostnames, which then got
/// fake-IP answers and were dialed through the proxy in Rule mode.
pub fn china_domain_suffix_list() -> &'static [&'static str] {
    CHINA_DOMAIN_SUFFIXES
}

/// Build a [`RuleSet`] with built-in China-direct rules.
///
/// Order:
/// 1. Private / LAN IPs → Direct
/// 2. Destinations identified as domestic by DNS policy → Direct
/// 3. Common China domain suffixes → Direct
/// 4. Common China domain keywords → Direct
/// 5. GeoIP CN (expanded built-in CIDR DB) → Direct
/// 6. Catch-all → Proxy
pub fn china_direct_ruleset() -> RuleSet {
    let mut rs = RuleSet::new();

    // --- Private / LAN ---
    for (addr, prefix_len) in private_networks() {
        rs.add_rule(MatchRule::IpCidr { addr, prefix_len }, RouteAction::Direct);
    }

    // Preserve DNS policy's domestic classification even with no custom rules.
    // A direct-domain CDN may use an address outside the built-in CN ranges.
    rs.add_rule(MatchRule::DomesticHint, RouteAction::Direct);

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
    let mut db = GeoIpDb::with_private_ranges();

    for (address, prefix) in china_ipv4_prefixes() {
        db.add_network(address.into(), prefix, "CN");
    }
    for (address, prefix) in china_ipv6_prefixes() {
        db.add_network(address.into(), prefix, "CN");
    }

    db.build();
    db
}

/// The curated China IPv4 CIDR list backing `geoip:CN` — exposed so the
/// router can load the same ranges into a kernel ipset for hardware-NAT
/// direct routing (single source of truth for both layers).
pub fn china_ipv4_cidrs() -> Vec<&'static str> {
    CHINA_IPV4_TEXT.lines().collect()
}

/// The original IPv6 CIDR strings in kernel bypass export order.
pub fn china_ipv6_cidrs() -> &'static [&'static str] {
    &include!("../../data/china_ipv6_cidrs.inc.rs")
}

/// Packed IPv4 prefixes for the Merlin kernel bypass export.
pub fn china_ipv4_prefixes() -> impl Iterator<Item = (std::net::Ipv4Addr, u8)> {
    unpack_prefixes::<4>(CHINA_IPV4).map(|(octets, prefix)| (octets.into(), prefix))
}

/// IPv6 prefixes shared by GeoIP routing and the Merlin kernel bypass.
pub fn china_ipv6_prefixes() -> impl Iterator<Item = (std::net::Ipv6Addr, u8)> {
    unpack_prefixes::<16>(CHINA_IPV6).map(|(octets, prefix)| (octets.into(), prefix))
}

fn unpack_prefixes<const N: usize>(table: &[u8]) -> impl Iterator<Item = ([u8; N], u8)> {
    let width = usize::from(table[0]);
    table[1..].chunks_exact(width + 1).map(move |entry| {
        let mut address = [0; N];
        address[..width].copy_from_slice(&entry[..width]);
        (address, entry[width])
    })
}

// ---- Static data ----

/// Common China top-level and service domain suffixes.
const CHINA_DOMAIN_SUFFIXES: &[&str] = &[
    // TLDs (IDN TLDs in A-label/punycode form, matching DNS wire format)
    "cn",
    "xn--fiqs8s", // .xn--fiqs8s (China IDN TLD)
    "xn--55qx5d", // .xn--55qx5d (company IDN TLD)
    "xn--io0a7i", // .xn--io0a7i (network IDN TLD)
    // Kimi web and its mainland platform use local DNS and the direct exit.
    // Keep international API names (moonshot.ai) outside this service rule.
    "kimi.com",
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
    // Accelerators geolocate the WAN address of their own API. A proxied
    // login is reported as overseas and the client switches to the
    // international build. These names use domestic DNS and stay direct
    // even when that DNS returns a CDN address missing from geoip:CN.
    "leigod.com",
    "leigod.cn",
    "leigodjsq.com",
    "nn.com",
    "nnbooster.com",
    // Chiphell rejects connections whose source address is not in China.
    "chiphell.com",
];

/// Keywords that indicate China-specific traffic.
const CHINA_DOMAIN_KEYWORDS: &[&str] = &["chinaz", "cnzz", "umeng"];

// Generated at build time from the checked-in CIDR lists.
const CHINA_IPV4: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/china_ipv4.bin"));
const CHINA_IPV6: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/china_ipv6.bin"));
/// Original IPv4 list in the line-oriented format consumed by Merlin's ipset loader.
pub const CHINA_IPV4_TEXT: &str = include_str!(concat!(env!("OUT_DIR"), "/china_ipv4.txt"));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_networks_preserve_every_cidr_in_export_order() {
        let v4 = include!("../../data/china_ipv4_cidrs.inc.rs");
        assert!(
            china_ipv4_prefixes()
                .map(|(ip, bits)| format!("{ip}/{bits}"))
                .eq(v4)
        );
        assert!(
            china_ipv6_prefixes()
                .map(|(ip, bits)| format!("{ip}/{bits}"))
                .eq(china_ipv6_cidrs().iter().copied())
        );
    }

    #[test]
    fn china_ruleset_has_rules() {
        let rs = china_direct_ruleset();
        assert!(rs.rules().len() > 100, "should have many rules");
    }

    #[test]
    fn dns_china_suffixes_match_routing_list() {
        let dns = crate::dns::china_domain_suffixes();
        let routing: Vec<&str> = china_domain_suffix_list().to_vec();
        assert_eq!(dns.len(), routing.len());
        for (a, b) in dns.iter().zip(routing.iter()) {
            assert_eq!(a, b);
        }
        assert!(
            dns.iter().any(|s| s == "meituan.com"),
            "CDN-heavy domestic names must be on the shared list"
        );
        assert!(dns.iter().any(|s| s == "hdslb.com"));
        assert!(dns.iter().any(|s| s == "leigod.com"));
        assert!(dns.iter().any(|s| s == "chiphell.com"));
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
        assert_eq!(
            router.route("webapi.leigod.com", 443),
            RouteAction::Direct,
            "Leigod API must egress from the WAN, or the client switches to the overseas build"
        );
        assert_eq!(
            router.route("www.chiphell.com", 443),
            RouteAction::Direct,
            "Chiphell rejects non-China source addresses"
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
        // Spot-check well-known CN allocations (incl. ranges 17mon used to miss).
        assert_eq!(
            db.lookup("1.0.1.1".parse().unwrap()),
            Some("CN"),
            "1.0.1.1 should map to CN"
        );
        assert_eq!(
            db.lookup("1.1.8.8".parse().unwrap()),
            Some("CN"),
            "1.1.8.8 (DNSPod) should map to CN"
        );
        assert_eq!(
            db.lookup("114.114.114.114".parse().unwrap()),
            Some("CN"),
            "114.114.114.114 should map to CN"
        );
    }

    #[test]
    fn china_ruleset_geoip_cn_with_db() {
        let rs = china_direct_ruleset();
        let router =
            crate::router::Router::new(rs).with_geoip(std::sync::Arc::new(china_geoip_db()));
        assert_eq!(router.route("1.0.1.1", 80), RouteAction::Direct);
        assert_eq!(router.route("8.8.8.8", 80), RouteAction::Proxy);
    }
}

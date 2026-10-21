//! Optional, locally bundled routing presets. Reading the catalog never changes
//! configuration; importing only appends disabled rules and never restarts.
use super::*;
use sockrocket_core::RoutingRule;

struct Preset {
    id: &'static str,
    title: &'static str,
    description: &'static str,
    target: &'static str,
    domains: &'static [&'static str],
}

const PRESETS: &[Preset] = &[
    Preset {
        id: "ai",
        title: "AI 服务",
        description: "ChatGPT、Claude 与 Gemini 的常用域名；不包含共享认证/CDN 的所有域名。",
        target: "proxy",
        domains: &[
            "openai.com",
            "chatgpt.com",
            "oaistatic.com",
            "oaiusercontent.com",
            "anthropic.com",
            "claude.ai",
            "gemini.google.com",
            "generativelanguage.googleapis.com",
            "aistudio.google.com",
        ],
    },
    Preset {
        id: "video",
        title: "海外视频",
        description: "YouTube 与 Netflix 的网页、图片及视频域名；播放能力取决于节点和服务地区。",
        target: "proxy",
        domains: &[
            "youtube.com",
            "youtu.be",
            "youtube-nocookie.com",
            "googlevideo.com",
            "ytimg.com",
            "ggpht.com",
            "netflix.com",
            "netflix.net",
            "nflxvideo.net",
            "nflximg.net",
            "nflximg.com",
            "nflxso.net",
            "nflxext.com",
        ],
    },
    Preset {
        id: "telegram",
        title: "Telegram",
        description: "Telegram 网页与链接域名。客户端直连数据中心 IP 时由其余 IP 规则或兜底决定。",
        target: "proxy",
        domains: &[
            "telegram.org",
            "t.me",
            "telegram.me",
            "telegra.ph",
            "telesco.pe",
        ],
    },
    Preset {
        id: "development",
        title: "开发工具",
        description: "GitHub、npm、Docker Hub、Rust 和 Python 官方包仓库的常用域名。",
        target: "proxy",
        domains: &[
            "github.com",
            "githubusercontent.com",
            "githubassets.com",
            "github.io",
            "githubcopilot.com",
            "npmjs.com",
            "npmjs.org",
            "docker.com",
            "docker.io",
            "rust-lang.org",
            "crates.io",
            "pypi.org",
            "pythonhosted.org",
        ],
    },
    Preset {
        id: "apple",
        title: "Apple 直连",
        description: "Apple、iCloud、App Store 等常用域名。部分地区服务可能仍需单独配置。",
        target: "direct",
        domains: &[
            "apple.com",
            "icloud.com",
            "icloud-content.com",
            "cdn-apple.com",
            "aaplimg.com",
            "mzstatic.com",
        ],
    },
    Preset {
        id: "microsoft",
        title: "Microsoft 直连",
        description: "Microsoft、Windows 更新、Office、OneDrive 与 Live 常用域名；不包含全部企业端点。",
        target: "direct",
        domains: &[
            "microsoft.com",
            "windowsupdate.com",
            "office.com",
            "office365.com",
            "office.net",
            "onedrive.com",
            "live.com",
            "msftconnecttest.com",
            "msftncsi.com",
            "microsoftonline.com",
            "sharepoint.com",
            "sharepointonline.com",
            "cloud.microsoft",
            "static.microsoft",
            "usercontent.microsoft",
        ],
    },
    Preset {
        id: "domestic",
        title: "国内常用网站",
        description: "百度、微信、哔哩哔哩、抖音和常用电商域名。规则模式已有内置国内直连，可按需显式添加。",
        target: "direct",
        domains: &[
            "baidu.com",
            "bdstatic.com",
            "qq.com",
            "weixin.com",
            "wechat.com",
            "qpic.cn",
            "gtimg.com",
            "bilibili.com",
            "bilivideo.com",
            "hdslb.com",
            "douyin.com",
            "douyinvod.com",
            "taobao.com",
            "tmall.com",
            "alicdn.com",
            "jd.com",
            "360buyimg.com",
            "alipay.com",
        ],
    },
];

fn group(p: &Preset) -> String {
    format!("预设 · {}", p.title)
}

pub(super) fn catalog() -> J {
    json!(PRESETS.iter().map(|p|json!({"id":p.id,"title":p.title,"description":p.description,"group":group(p),"target":p.target,"domains":p.domains,"priority":-10,"enabled":false})).collect::<Vec<_>>())
}

pub(super) fn add(post: &J) -> J {
    let Some(preset) = PRESETS
        .iter()
        .find(|p| post.get("preset").and_then(J::as_str) == Some(p.id))
    else {
        return err_msg("未知规则预设，请刷新后重试");
    };
    if post.get("enabled").is_some_and(|v| v != &J::Bool(false)) {
        return err_msg("预设只能添加为停用状态，请添加后在规则页选择开启");
    }
    let Ok(mut map) = load_conf() else {
        return err_msg("无法读取规则配置");
    };
    if post.get("expected_revision").and_then(J::as_str) != Some(&route_trace::rules_revision(&map))
    {
        return err_msg("规则已变化，请刷新后重新添加预设");
    }
    let mut rules = load_routing_rules(&map);
    if rules.len() != conf_seq(&map, "rules").len() {
        return err_msg("规则配置损坏，请先修复后再添加预设");
    }
    let mut added = 0;
    for domain in preset.domains {
        // An existing global selector belongs to the user, even if its target,
        // priority or enabled state differs. Do not replace or re-enable it.
        if rules.iter().any(|r| {
            r.source.is_empty()
                && r.rule_type.eq_ignore_ascii_case("domain-suffix")
                && r.pattern
                    .trim_start_matches('.')
                    .eq_ignore_ascii_case(domain)
        }) {
            continue;
        }
        rules.push(RoutingRule {
            source: String::new(),
            name: String::new(),
            group: group(preset),
            rule_type: "domain-suffix".into(),
            pattern: (*domain).into(),
            target: preset.target.into(),
            priority: -10,
            enabled: false,
        });
        added += 1;
    }
    if added > 0 {
        save_routing_rules(&mut map, rules);
        if save_conf(&map).is_err() {
            return err_msg("预设保存失败，请检查存储状态后重试");
        }
    }
    json!({"ok":true,"added":added,"skipped":preset.domains.len()-added,"group":group(preset),"msg":format!("已添加 {added} 条停用规则，跳过 {} 条已有规则；可在规则页按组开启",preset.domains.len()-added)})
}

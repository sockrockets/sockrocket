//! Authenticated, read-only routing explanation. Query input is neither saved
//! nor logged; URL paths, queries and credentials are never echoed in results.
use super::*;
use sha2::{Digest, Sha256};
use sockrocket_core::router::{RouteAction, Router, china_geoip_db, rule_mode_ruleset};
use std::net::IpAddr;

pub(super) fn rules_revision(map: &Mapping) -> String {
    let input = json!({"groups":map.get(ykey("groups")), "nodes":map.get(ykey("nodes")), "rules": load_routing_rules(map),
        "mode": conf_str(map,"proxy_mode").unwrap_or_else(||"rule".into()).to_lowercase(),
        "active_node": conf_usize(map,"active_node"), "dns":map.get(ykey("dns")),"dns_direct_server":map.get(ykey("dns_direct_server")),"dns_direct_domains":map.get(ykey("dns_direct_domains")),"ipv6_mode":map.get(ykey("ipv6_mode"))});
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&input).unwrap_or_default())
    )
}

fn destination(post: &J) -> Result<(String, u16), &'static str> {
    let raw = post
        .get("destination")
        .and_then(J::as_str)
        .unwrap_or("")
        .trim();
    if raw.is_empty() || raw.len() > 2048 || raw.chars().any(char::is_whitespace) {
        return Err("请输入有效的域名、IP 或 HTTP/HTTPS 网址");
    }
    let (host, default_port) = if let Ok(ip) = raw.trim_matches(['[', ']']).parse::<IpAddr>() {
        (ip.to_string(), 443)
    } else {
        let input = if raw.contains("://") {
            raw.to_owned()
        } else {
            format!("https://{raw}")
        };
        let url = url::Url::parse(&input).map_err(|_| "网址格式无效")?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err("仅支持不含用户名和密码的 HTTP/HTTPS 网址");
        }
        let host = url
            .host_str()
            .ok_or("网址缺少主机名")?
            .trim_matches(['[', ']'])
            .trim_end_matches('.')
            .to_ascii_lowercase();
        if host.is_empty() || host.len() > 253 {
            return Err("主机名无效");
        }
        (host, url.port_or_known_default().unwrap_or(443))
    };
    let port = match post.get("port") {
        None | Some(J::Null) => default_port,
        Some(value) => value
            .as_u64()
            .and_then(|p| u16::try_from(p).ok())
            .filter(|p| *p > 0)
            .ok_or("端口必须为 1–65535")?,
    };
    Ok((host, port))
}
fn optional_ip(post: &J, key: &str) -> Result<Option<IpAddr>, &'static str> {
    match post.get(key) {
        None | Some(J::Null) => Ok(None),
        Some(J::String(value)) if value.trim().is_empty() => Ok(None),
        Some(J::String(value)) => value
            .trim()
            .parse()
            .map(Some)
            .map_err(|_| "设备或目的 IP 必须是单个 IPv4/IPv6 地址，不能填写网段"),
        _ => Err("设备或目的 IP 必须是 IP 地址字符串"),
    }
}
fn failure(code: &str, message: &str) -> J {
    json!({"ok":false,"error_code":code,"error_message":message,"msg":message})
}

pub(super) fn act_trace_route(post: &J) -> J {
    // Keep on-demand ruleset construction and DNS work from competing with
    // lifecycle/status requests on low-memory routers. CGI bridges also have
    // their own process admission limits.
    static QUERY: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let Ok(_query) = QUERY.try_lock() else {
        return failure("route_trace_busy", "已有分流检查正在进行，请稍后重试");
    };
    let (host, port) = match destination(post) {
        Ok(value) => value,
        Err(msg) => return failure("route_trace_invalid", msg),
    };
    let source = match optional_ip(post, "source") {
        Ok(value) => value,
        Err(msg) => return failure("route_trace_invalid", msg),
    };
    let destination_ip = match optional_ip(post, "destination_ip") {
        Ok(value) => value,
        Err(msg) => return failure("route_trace_invalid", msg),
    };
    let resolve = match post.get("resolve_dns") {
        None => true,
        Some(J::Bool(value)) => *value,
        _ => return failure("route_trace_invalid", "解析选项必须为布尔值"),
    };
    if host
        .parse::<IpAddr>()
        .ok()
        .or(destination_ip)
        .is_some_and(|ip| sockrocket_core::dns::fakeip::fake_ipv4(ip).is_some())
    {
        return failure(
            "route_trace_fake_ip",
            "Fake-IP 不能用于 IP 规则判定，请填写真实目的 IP 或查询域名",
        );
    }
    let map = match load_conf() {
        Ok(map) => map,
        Err(_) => {
            return failure(
                "route_trace_config",
                "无法读取规则配置，请通过软件中心检查完整安装",
            );
        }
    };
    let revision = rules_revision(&map);
    if post
        .get("expected_revision")
        .is_some_and(|v| v.as_str() != Some(&revision))
    {
        return failure(
            "route_trace_stale",
            "规则或模式已经更新，请刷新规则后重新检查",
        );
    }
    let rules = load_routing_rules(&map);
    if rules.len() != conf_seq(&map, "rules").len() {
        return failure(
            "route_trace_config",
            "部分规则配置损坏，无法可靠判定，请先修正规则配置",
        );
    }
    let mode = conf_str(&map, "proxy_mode")
        .unwrap_or_else(|| "rule".into())
        .to_ascii_lowercase();
    if !matches!(mode.as_str(), "rule" | "global" | "direct") {
        return failure("route_trace_config", "当前分流模式无效");
    }
    let mut trace = if mode == "rule" {
        let router = Router::new(rule_mode_ruleset(&rules))
            .with_geoip(std::sync::Arc::new(china_geoip_db()));
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(_) => return failure("route_trace_unavailable", "无法启动分流查询，请稍后重试"),
        };
        match rt.block_on(async {
            let context = async {
                let settings =
                    sockrocket_core::config::dns::DnsSettings::from_yaml(&Y::Mapping(map.clone()))?;
                let config: sockrocket_core::AppConfig =
                    serde_yaml::from_value(Y::Mapping(map.clone()))?;
                let dns = settings
                    .resolver_config(config.nodes.iter().map(|node| node.server.clone()))?;
                let mut resolver = sockrocket_core::DnsResolver::with_config(dns);
                let outbound = match config.nodes.get(config.active_node.unwrap_or(0)) {
                    Some(node) => sockrocket_core::create_outbound_unwarmed(node)?,
                    None => sockrocket_core::SharedOutbound::direct(),
                };
                resolver.follow_runtime(outbound);
                Ok::<_, anyhow::Error>(
                    resolver
                        .scope(router.trace_route(&host, port, source, destination_ip, resolve))
                        .await,
                )
            };
            tokio::time::timeout(Duration::from_secs(12), context).await
        }) {
            Ok(Ok(value)) => serde_json::to_value(value).unwrap(),
            Ok(Err(_)) => {
                return failure(
                    "route_trace_dns_config",
                    "无法构建当前 DNS 或节点配置，请检查网络与 DNS 设置",
                );
            }
            Err(_) => {
                return failure(
                    "route_trace_timeout",
                    "DNS 查询超时，可填写实际目的 IP 后重试",
                );
            }
        }
    } else {
        json!({"action":if mode=="direct" {RouteAction::Direct} else {RouteAction::Proxy},
            "matched_rule":null,"matched_builtin":false,"resolved_ip":null,"dns":"not_needed",
            "definitive":true,"steps":[],"omitted_steps":0,"checked_rules":0})
    };
    let matched = trace["matched_rule"]
        .as_u64()
        .and_then(|i| rules.get(i as usize));
    trace["matched_snapshot"] = matched.map(rule_edit_snapshot).unwrap_or(J::Null);
    let nodes = conf_seq(&map, "nodes");
    // The daemon defaults an omitted active_node to the first configured node.
    // Match that default so polling status does not invalidate a valid trace.
    let active = Some(conf_usize(&map, "active_node").unwrap_or(0)).filter(|i| *i < nodes.len());
    let selected_group = trace["action"]
        .as_str()
        .and_then(|s| s.strip_prefix("group:"));
    let group_info = proxy_groups::catalog(&map);
    let group_node = selected_group
        .and_then(|name| {
            group_info
                .as_array()?
                .iter()
                .find(|g| g["name"].as_str() == Some(name))
        })
        .and_then(|g| g["selected_node"].as_str());
    let name = active
        .and_then(|i| nodes[i].get(ykey("name")))
        .and_then(Y::as_str);
    let (transparent_proxy, _) = effective_toggles(&map);
    json!({"ok":true,"scope":"saved_config","revision":revision,"destination":host,"port":port,"source":source,
        "mode":mode,"running":is_running(),"transparent_proxy":transparent_proxy,
        "lan_degraded":sockrocket_dir().join(".lan_degraded").exists(),
        "cn_ipset_direct":conf_bool(&map,"cn_ipset_direct",false),
        "active_node":active,"node_name":if selected_group.is_some() {group_node} else {name},"proxy_group":selected_group,"group_type":selected_group.and_then(|name|group_info.as_array()?.iter().find(|g|g["name"].as_str()==Some(name))).map(|g|g["type"].clone()),"trace":trace})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn destination_parsing_handles_urls_ip_ports_and_idn() {
        for (input, host, port) in [
            (
                "https://Example.COM.:8443/a?token=secret",
                "example.com",
                8443,
            ),
            ("http://example.com", "example.com", 80),
            ("2001:db8::1", "2001:db8::1", 443),
            ("[2001:db8::1]:444", "2001:db8::1", 444),
            ("https://例子.com", "xn--fsqu00a.com", 443),
        ] {
            assert_eq!(
                destination(&json!({"destination":input})).unwrap(),
                (host.into(), port)
            );
        }
        assert_eq!(
            destination(&json!({"destination":"example.com:80","port":8080})).unwrap(),
            ("example.com".into(), 8080)
        );
        for input in [
            "",
            "ftp://example.com",
            "https://",
            "a b.com",
            "https://u:p@example.com",
        ] {
            assert!(destination(&json!({"destination":input})).is_err());
        }
    }
}

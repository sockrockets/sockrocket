//! Structured network policy API; never echo resolver URLs in diagnostics.
use super::*;
use sockrocket_core::config::dns::{DnsSettings, Ipv6Mode};

fn failure(code: &str, msg: &str) -> J {
    json!({"ok":false,"error_code":code,"msg":msg})
}
pub(super) fn revision(map: &Mapping) -> String {
    use sha2::{Digest, Sha256};
    let policy = json!({"dns":map.get(ykey("dns")),"ipv6_mode":map.get(ykey("ipv6_mode")),"legacy_server":map.get(ykey("dns_direct_server")),"legacy_domains":map.get(ykey("dns_direct_domains"))});
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&policy).unwrap_or_default())
    )
}
pub(super) fn get() -> J {
    let Ok(map) = load_conf() else {
        return failure(
            "network_config_unavailable",
            "无法读取网络配置，请检查完整安装和文件权限",
        );
    };
    let dns = match DnsSettings::from_yaml(&Y::Mapping(map.clone())) {
        Ok(s) => s,
        Err(_) => {
            return failure(
                "dns_config_invalid",
                "DNS 配置无效，请检查高级配置中的 DNS 字段",
            );
        }
    };
    let mode: Ipv6Mode = match map.get(ykey("ipv6_mode")) {
        None => Ipv6Mode::default(),
        Some(value) => match serde_yaml::from_value(value.clone()) {
            Ok(mode) => mode,
            Err(_) => return failure("ipv6_mode_invalid", "IPv6 模式无效"),
        },
    };
    let running = is_running();
    let active = ipv6_active(
        running,
        super::effective_toggles(&map).0,
        mode == Ipv6Mode::DualStack,
    );
    json!({"ok":true,"dns":dns,"ipv6_mode":mode,"ipv6_active":active,"running":running,"revision":revision(&map)})
}

fn runtime_dir() -> std::path::PathBuf {
    std::env::var_os("SOCKROCKET_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "/tmp/sockrocket-run".into())
}

/// LAN IPv6 reaches the proxy only with the routing chain installed and no
/// fallback recorded. With the fallback marker the firewall serves no IPv6
/// answers, so clients stay on IPv4 even though some rules are in place.
pub(super) fn ipv6_active(running: bool, transparent_proxy: bool, dual_stack: bool) -> bool {
    running
        && transparent_proxy
        && dual_stack
        && !runtime_dir().join("ipv6_fallback").exists()
        && run_cmd(
            "ip6tables",
            &[
                "-t",
                "mangle",
                "-C",
                "PREROUTING",
                "-j",
                "SOCKROCKET_IPV6_MANGLE",
            ],
        )
        .is_some()
}
pub(super) fn save(post: &J) -> J {
    let dns = match DnsSettings::from_json(post.get("dns").cloned().unwrap_or(J::Null)) {
        Ok(dns) => dns,
        Err(error) => {
            return failure("dns_config_invalid", &format!("DNS 配置校验失败：{error}"));
        }
    };
    let mode =
        match serde_json::from_value::<Ipv6Mode>(post.get("ipv6_mode").cloned().unwrap_or(J::Null))
        {
            Ok(mode) => mode,
            Err(_) => return failure("ipv6_mode_invalid", "请选择 IPv4 兼容或 IPv6 双栈模式"),
        };
    if mode == Ipv6Mode::Compatibility
        && dns.strategy == sockrocket_core::config::dns::AddressStrategy::Ipv6Only
    {
        return failure(
            "dns_strategy_conflict",
            "仅 IPv6 解析需要先选择双栈模式；IPv4 兼容模式无法转发 IPv6",
        );
    }
    let Ok(mut map) = load_conf() else {
        return failure("network_config_unavailable", "无法读取当前配置，未保存更改");
    };
    if post.get("revision").and_then(J::as_str) != Some(revision(&map).as_str()) {
        return failure(
            "network_config_stale",
            "配置已在其他位置更新，请重新加载后再保存",
        );
    }
    let old_dns = DnsSettings::from_yaml(&Y::Mapping(map.clone())).ok();
    let old_mode = map
        .get(ykey("ipv6_mode"))
        .cloned()
        .and_then(|v| serde_yaml::from_value::<Ipv6Mode>(v).ok())
        .unwrap_or_default();
    if old_dns.as_ref() == Some(&dns) && old_mode == mode {
        return json!({"ok":true,"saved":false,"applied":false,"unchanged":true,"msg":"设置未改变，服务保持当前状态"});
    }
    // Reject explicit router-local resolvers as well as loopback, preventing
    // dnsmasq -> listener -> dnsmasq bootstrap loops.
    if let Some(router) = run_cmd("nvram", &["get", "lan_ipaddr"])
        .and_then(|s| s.trim().parse::<std::net::IpAddr>().ok())
    {
        let loops = dns.bootstrap_servers.iter().any(|s| s.ip() == router)
            || dns
                .direct_servers
                .iter()
                .chain(&dns.proxy_servers)
                .any(|s| {
                    url::Url::parse(s).ok().and_then(|u| {
                        u.host_str().and_then(|h| {
                            h.trim_matches(['[', ']']).parse::<std::net::IpAddr>().ok()
                        })
                    }) == Some(router)
                });
        if loops {
            return failure(
                "dns_resolver_loop",
                "DNS 上游不能填写路由器自身地址，否则会形成解析循环",
            );
        }
    }
    map.insert(ykey("dns"), serde_yaml::to_value(dns).unwrap());
    map.insert(ykey("ipv6_mode"), serde_yaml::to_value(mode).unwrap());
    if save_conf(&map).is_err() {
        return failure("network_save_failed", "网络配置保存失败，原有配置未被覆盖");
    }
    let running = is_running();
    // DNS and ipv6_mode reload in the running process. Reapply only the IPv6
    // firewall; a full restart also tears the working IPv4 rules down.
    if running && run_sh("reapply-policy").is_err() {
        return json!({"ok":false,"saved":true,"applied":false,"error_code":"network_apply_failed","msg":"配置已保存，但防火墙策略没有重新应用。IPv4 代理保持原状，请查看诊断。"});
    }
    if running && sockrocket_dir().join(".lan_degraded").exists() {
        return json!({"ok":true,"saved":true,"applied":false,"msg":"配置已保存。代理节点暂不可用，局域网目前直连；代理恢复后会自动应用新策略。"});
    }
    let runtime = runtime_dir();
    if running && mode == Ipv6Mode::DualStack && runtime.join("ipv6_fallback").exists() {
        return json!({"ok":true,"saved":true,"applied":true,"ipv6_active":false,"msg":"配置已保存。当前主机还不能完成 IPv6 双栈，IPv4 代理和 DNS 劫持保持原样。"});
    }
    let applied = running && is_running() && !sockrocket_dir().join("stopped").exists();
    if running && !applied && !sockrocket_dir().join("stopped").exists() {
        return json!({"ok":false,"saved":true,"applied":false,"error_code":"network_apply_failed","msg":"配置已保存，但服务未恢复运行，请查看诊断。"});
    }
    json!({"ok":true,"saved":true,"applied":applied,"msg":if applied {"网络配置已保存并重新应用"}else{"网络配置已保存；服务保持停止，下次启动生效"}})
}
pub(super) fn test(post: &J) -> J {
    static TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let Ok(_guard) = TEST.try_lock() else {
        return failure("dns_test_busy", "已有 DNS 测试正在进行，请稍后重试");
    };
    let settings = match DnsSettings::from_json(post.get("dns").cloned().unwrap_or(J::Null)) {
        Ok(s) => s,
        Err(_) => return failure("dns_config_invalid", "请先检查 DNS 地址、超时和缓存范围"),
    };
    let host = post
        .get("domain")
        .and_then(J::as_str)
        .unwrap_or("")
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if !sockrocket_core::config::dns::valid_domain(&host)
        || host.parse::<std::net::IpAddr>().is_ok()
    {
        return failure("dns_test_invalid", "请输入域名，不要填写网址、路径或 IP");
    }
    let qtype = match post.get("qtype").and_then(J::as_str) {
        Some("A") => 1,
        Some("AAAA") => 28,
        _ => return failure("dns_test_invalid", "请选择 A 或 AAAA 查询"),
    };
    let group = post.get("group").and_then(J::as_str).unwrap_or("");
    if !matches!(group, "direct" | "proxy" | "bootstrap") {
        return failure("dns_test_invalid", "请选择 DNS 测试分组");
    }
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return failure("dns_test_failed", "无法创建 DNS 测试任务");
    };
    let started = std::time::Instant::now();
    let result = rt.block_on(async {
        let mut config = settings.resolver_config(std::iter::empty())?;
        config.domain_rules.clear();
        let outbound = if group == "proxy" {
            let value = Y::Mapping(load_conf()?);
            let app_config: sockrocket_core::AppConfig = serde_yaml::from_value(value)?;
            let node = app_config
                .nodes
                .get(app_config.active_node.unwrap_or(0))
                .ok_or_else(|| anyhow::anyhow!("no outbound"))?;
            config
                .node_hosts
                .insert(node.server.trim_end_matches('.').to_ascii_lowercase());
            sockrocket_core::create_outbound_unwarmed(node)?
        } else {
            sockrocket_core::SharedOutbound::direct()
        };
        config.servers = match group {
            "proxy" => config.proxy_servers.take().unwrap_or_default(),
            "bootstrap" => settings
                .bootstrap_servers
                .iter()
                .copied()
                .map(sockrocket_core::DnsServer::Udp)
                .collect(),
            _ => config.fallback.clone(),
        };
        let mut resolver = sockrocket_core::DnsResolver::with_config(config);
        resolver.set_outbound(outbound);
        resolver.scope(resolver.test_upstream(&host, qtype)).await
    });
    match result {
        Ok(addresses) => {
            json!({"ok":true,"group":group,"qtype":if qtype==1{"A"}else{"AAAA"},"addresses":addresses,"elapsed_ms":started.elapsed().as_millis(),"msg":"DNS 上游测试完成；此测试不代表 LAN 双栈转发已通过验证"})
        }
        Err(error) => {
            let text = format!("{error:#}").to_ascii_lowercase();
            let (code, msg) = if text.contains("resolver loop") {
                (
                    "dns_resolver_loop",
                    "DNS 上游指向本机或路由器自身，请改用独立上游，避免解析循环",
                )
            } else if text.contains("no outbound") {
                ("dns_no_outbound", "未选择可用节点，请先添加并选择节点")
            } else if text.contains("timeout") || text.contains("timed out") {
                (
                    "dns_timeout",
                    "DNS 查询超时，请检查上游可达性、节点和超时设置",
                )
            } else if text.contains("tls") || text.contains("certificate") {
                (
                    "dns_tls_failed",
                    "DNS TLS 握手或证书验证失败，请检查地址、系统时间和证书",
                )
            } else if text.contains("bootstrap") {
                (
                    "dns_bootstrap_failed",
                    "DNS 上游主机名解析失败，请检查引导 DNS",
                )
            } else {
                (
                    "dns_exchange_failed",
                    "DNS 交换失败，请检查上游协议、地址端口、网络和代理节点",
                )
            };
            failure(code, msg)
        }
    }
}

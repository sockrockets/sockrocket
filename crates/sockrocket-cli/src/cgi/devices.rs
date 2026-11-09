//! Merlin LAN inventory and live links from a device to named routing rules.
use super::*;
use serde::{Deserialize, Serialize};
use sockrocket_core::config::model::DeviceRuleGroup;
use std::collections::BTreeMap;
use std::net::Ipv4Addr;

pub(super) fn links(map: &Mapping) -> Result<Vec<DeviceRuleGroup>> {
    Ok(serde_yaml::from_value(
        map.get(ykey("device_rule_groups"))
            .cloned()
            .unwrap_or(Y::Sequence(vec![])),
    )?)
}
pub(super) fn store_links(map: &mut Mapping, links: &[DeviceRuleGroup]) {
    if links.is_empty() {
        map.remove(ykey("device_rule_groups"));
    } else {
        map.insert(
            ykey("device_rule_groups"),
            serde_yaml::to_value(links).unwrap(),
        );
    }
}
// Materialize legacy membership before an editor can add or rename a group.
pub(super) fn migrate_bindings(map: &mut Mapping) -> Result<()> {
    if map
        .get(ykey("explicit_group_bindings"))
        .and_then(Y::as_bool)
        == Some(true)
    {
        return Ok(());
    }
    let mut bindings = links(map)?;
    let mut seen = std::collections::BTreeSet::new();
    for rule in checked_rules(map)? {
        let name = rule.group.trim();
        if rule.is_global_destination() && !name.is_empty() && seen.insert(name.to_owned()) {
            bindings.push(DeviceRuleGroup {
                source: String::new(),
                group: name.into(),
                priority: 0,
                target: String::new(),
            });
        }
    }
    store_links(map, &bindings);
    map.insert(ykey("explicit_group_bindings"), Y::Bool(true));
    Ok(())
}
pub(super) fn save_binding(post: &J) -> J {
    let Ok(mut map) = load_conf() else {
        return err_msg("配置读取失败");
    };
    if !proxy_groups::check_revision(&map, post) {
        return err_msg("规则已变化，请刷新后重试");
    }
    let Ok(source) = requested_rule_source(post, "") else {
        return err_msg("请填写有效的设备 IP 或网段");
    };
    if source.is_empty() && post["all_devices"].as_bool() != Some(true) {
        return err_msg("请选择设备和规则组");
    }
    let Ok(mut bindings) = links(&map) else {
        return err_msg("设备关联配置无效");
    };
    let Ok(rules) = checked_rules(&map) else {
        return err_msg("规则配置无效");
    };
    let priority = match post.get("priority") {
        None => None,
        Some(value) => match value.as_u64().and_then(|n| u16::try_from(n).ok()) {
            Some(n) => Some(n),
            None => return err_msg("关联优先级须为 0 至 65535 的整数"),
        },
    };
    // Adding several groups is one configuration write and one service apply.
    // Existing bindings retain their per-device exits, including on retry.
    if let Some(names) = post.get("groups") {
        let Some(target) = post
            .get("target")
            .and_then(J::as_str)
            .filter(|t| !t.is_empty() && proxy_groups::valid_target(t))
        else {
            return err_msg("请选择有效的规则组出口");
        };
        let Some(names) = names.as_array().filter(|v| !v.is_empty() && v.len() <= 256) else {
            return err_msg("请选择 1 至 256 个规则组");
        };
        let eligible: std::collections::HashSet<_> = rules
            .iter()
            .filter(|r| r.is_global_destination())
            .map(|r| r.group.trim())
            .collect();
        let before = bindings.len();
        for name in names {
            let Some(group) = name
                .as_str()
                .map(str::trim)
                .filter(|g| !g.is_empty() && eligible.contains(g))
            else {
                return err_msg("所选组中有无效规则组，请刷新后重试");
            };
            if !bindings
                .iter()
                .any(|b| b.source == source && b.group == group)
            {
                bindings.push(DeviceRuleGroup {
                    source: source.clone(),
                    group: group.into(),
                    priority: priority.unwrap_or_default(),
                    target: target.into(),
                });
            }
        }
        if bindings.len() == before {
            return json!({"ok":true,"msg":"所选组均已关联"});
        }
        store_links(&mut map, &bindings);
        return proxy_groups::applied(&map);
    }
    let group = post["group"].as_str().unwrap_or("").trim();
    if group.is_empty() {
        return err_msg("请选择规则组");
    }
    let remove = post["remove"].as_bool() == Some(true);
    let Some(target) = post["target"].as_str() else {
        return err_msg("请选择规则组出口");
    };
    if !remove {
        if target.is_empty() || !proxy_groups::valid_target(target) {
            return err_msg("请选择有效出口");
        }
        if !rules
            .iter()
            .any(|r| r.is_global_destination() && r.group.trim() == group)
        {
            return err_msg("这个组没有可关联的全局目的规则");
        }
    }
    let position = bindings
        .iter()
        .position(|b| b.source == source && b.group == group);
    match (position, remove) {
        (Some(i), true) => {
            bindings.remove(i);
        }
        (Some(i), false)
            if bindings[i].target != target
                || priority.is_some_and(|p| p != bindings[i].priority) =>
        {
            if let Some(p) = priority {
                bindings[i].priority = p;
            }
            bindings[i].target = target.to_owned();
        }
        (None, false) => bindings.push(DeviceRuleGroup {
            source,
            group: group.to_owned(),
            priority: priority.unwrap_or_default(),
            target: target.to_owned(),
        }),
        _ => return json!({"ok":true,"msg":"设备关联已是所选状态"}),
    }
    store_links(&mut map, &bindings);
    proxy_groups::applied(&map)
}
fn checked_rules(map: &Mapping) -> Result<Vec<sockrocket_core::RoutingRule>> {
    Ok(serde_yaml::from_value(
        map.get(ykey("rules"))
            .cloned()
            .unwrap_or(Y::Sequence(vec![])),
    )?)
}

#[derive(Serialize, Deserialize)]
struct Alias {
    name: String,
    source: String,
}
type Aliases = BTreeMap<String, Alias>;
fn aliases() -> Result<Aliases> {
    match fs::read(sockrocket_dir().join("device-aliases.json")) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Aliases::new()),
        Err(err) => Err(err.into()),
    }
}
fn mac(value: &str) -> Option<String> {
    let parts: Vec<_> = value.split(':').collect();
    (parts.len() == 6
        && parts
            .iter()
            .all(|s| s.len() == 2 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        && value != "00:00:00:00:00:00")
        .then(|| value.to_ascii_lowercase())
}
#[derive(Default)]
struct Client {
    mac: String,
    host: String,
}
fn clients(leases: &str, arp: &str) -> BTreeMap<Ipv4Addr, Client> {
    let mut rows = BTreeMap::new();
    for line in leases.lines().take(2048) {
        let fields: Vec<_> = line.split_whitespace().take(4).collect();
        if let [_, hw, addr, host] = fields.as_slice()
            && let (Ok(ip), Some(mac)) = (addr.parse::<Ipv4Addr>(), mac(hw))
        {
            rows.insert(
                ip,
                Client {
                    mac,
                    host: if *host == "*" {
                        String::new()
                    } else {
                        host.chars().take(128).collect()
                    },
                },
            );
        }
    }
    for line in arp.lines().take(2048) {
        let fields: Vec<_> = line.split_whitespace().collect();
        if let [addr, _, flags, hw, _, iface] = fields.as_slice()
            && iface.starts_with("br")
            && u32::from_str_radix(flags.trim_start_matches("0x"), 16).is_ok_and(|f| f & 2 != 0)
            && let (Ok(ip), Some(mac)) = (addr.parse::<Ipv4Addr>(), mac(hw))
        {
            let row = rows.entry(ip).or_default();
            if row.mac != mac {
                *row = Client {
                    mac,
                    host: String::new(),
                };
            }
        }
    }
    rows
}
pub(super) fn catalog() -> Vec<J> {
    let mut aliases = aliases().unwrap_or_default();
    let removed: Vec<String> = load_conf()
        .ok()
        .and_then(|m| m.get(ykey("removed_device_sources")).cloned())
        .and_then(|v| serde_yaml::from_value(v).ok())
        .unwrap_or_default();
    aliases.retain(|_, a| !removed.contains(&a.source));
    let leases = fs::read_to_string("/var/lib/misc/dnsmasq.leases").unwrap_or_default();
    let arp = fs::read_to_string("/proc/net/arp").unwrap_or_default();
    let mut rows = BTreeMap::new();
    let mut visible = std::collections::HashSet::new();
    for (ip, c) in clients(&leases, &arp) {
        let source = format!("{ip}/32");
        let key = format!("mac:{}", c.mac);
        let alias = aliases.get(&key).or_else(|| aliases.get(&source));
        rows.insert(source.clone(),json!({"source":source,"key":key,"host":c.host,"alias":alias.map(|a|a.name.as_str()).unwrap_or(""),"alias_key":if aliases.contains_key(&key)||alias.is_none(){&key}else{&source}}));
        visible.insert(key);
    }
    for (key, alias) in aliases {
        if !visible.contains(&key) {
            rows.entry(alias.source.clone()).or_insert_with(
                || json!({"source":alias.source,"key":key,"alias_key":key,"alias":alias.name}),
            );
        }
    }
    rows.into_values().collect()
}
/// Remove only this exact source's policies; broader subnet policies still apply.
pub(super) fn delete(post: &J) -> J {
    let Ok(mut map) = load_conf() else {
        return err_msg("配置读取失败");
    };
    if !proxy_groups::check_revision(&map, post) {
        return err_msg("规则已变化，请刷新后重试");
    }
    let Ok(source) = requested_rule_source(post, "") else {
        return err_msg("设备来源无效");
    };
    if source.is_empty() {
        return err_msg("不能删除全部设备");
    }
    let Ok(mut rules) = checked_rules(&map) else {
        return err_msg("规则读取失败");
    };
    let Ok(mut bindings) = links(&map) else {
        return err_msg("绑定读取失败");
    };
    rules.retain(|r| r.source != source);
    bindings.retain(|b| b.source != source);
    store_links(&mut map, &bindings);
    save_routing_rules(&mut map, rules);
    // Alias cleanup is recorded in the same configuration transaction.
    let mut removed: Vec<String> = serde_yaml::from_value(
        map.get(ykey("removed_device_sources"))
            .cloned()
            .unwrap_or(Y::Sequence(vec![])),
    )
    .unwrap_or_default();
    if !removed.contains(&source) {
        removed.push(source);
    }
    map.insert(
        ykey("removed_device_sources"),
        serde_yaml::to_value(removed).unwrap(),
    );
    proxy_groups::applied(&map)
}
pub(super) fn save_alias(post: &J) -> J {
    let Ok(source) = requested_rule_source(post, "") else {
        return err_msg("无效设备来源");
    };
    let key = post["key"]
        .as_str()
        .filter(|key| !key.is_empty())
        .unwrap_or(&source);
    if source.is_empty()
        || !(key == source
            || key
                .strip_prefix("mac:")
                .and_then(mac)
                .is_some_and(|m| key == format!("mac:{m}")))
    {
        return err_msg("无效设备标识");
    }
    let Some(name) = post["name"].as_str().map(str::trim) else {
        return err_msg("无效名称");
    };
    if name.chars().count() > 48 || name.chars().any(char::is_control) {
        return err_msg("设备名称最多 48 个字符，不能包含控制字符");
    }
    let Ok(mut aliases) = aliases() else {
        return err_msg("读取设备名称失败");
    };
    let removed: Vec<String> = load_conf()
        .ok()
        .and_then(|m| m.get(ykey("removed_device_sources")).cloned())
        .and_then(|v| serde_yaml::from_value(v).ok())
        .unwrap_or_default();
    let previous = aliases
        .get(key)
        .filter(|a| !removed.contains(&a.source))
        .map_or("", |a| a.name.as_str());
    if post["previous"].as_str() != Some(previous) {
        return err_msg("名称已变化，请刷新后重试");
    }
    if name.is_empty() {
        aliases.remove(key);
    } else {
        aliases.insert(
            key.into(),
            Alias {
                name: name.into(),
                source: source.clone(),
            },
        );
    }
    if let Ok(mut map) = load_conf() {
        if let Some(Y::Sequence(removed)) = map.get_mut(ykey("removed_device_sources")) {
            removed.retain(|v| v.as_str() != Some(&source));
            if save_conf(&map).is_err() {
                return err_msg("设备名称恢复失败");
            }
        }
    }
    let path = sockrocket_dir().join("device-aliases.json");
    let saved = serde_json::to_string(&aliases)
        .map_err(anyhow::Error::from)
        .and_then(|text| {
            sockrocket_core::config::storage::atomic_write_private(&path, &text, false)
        });
    if saved.is_err() {
        return err_msg("设备名称保存失败");
    }
    json!({"ok":true,"msg":"设备名称已保存"})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn combines_dhcp_and_lan_neighbors_without_reusing_a_stale_hostname() {
        let rows = clients(
            "1 10:00:00:00:00:01 192.0.2.1 TV *\n1 10:00:00:00:00:02 192.0.2.2 Old *",
            "192.0.2.2 0x1 0x2 10:00:00:00:00:03 * br0\n192.0.2.3 0x1 0x2 10:00:00:00:00:04 * eth0\n192.0.2.4 0x1 0x12 10:00:00:00:00:04 * br1\n192.0.2.5 0x1 0x10 10:00:00:00:00:05 * br0",
        );
        assert_eq!(rows.len(), 3);
        assert!(rows.contains_key(&Ipv4Addr::new(192, 0, 2, 4)));
        assert!(!rows.contains_key(&Ipv4Addr::new(192, 0, 2, 5)));
        assert_eq!(rows[&Ipv4Addr::new(192, 0, 2, 1)].host, "TV");
        assert_eq!(rows[&Ipv4Addr::new(192, 0, 2, 2)].host, "");
        assert_eq!(rows[&Ipv4Addr::new(192, 0, 2, 2)].mac, "10:00:00:00:00:03");
    }
}

//! Group and device policy mutations share the config transaction with rules.
use super::*;
use sockrocket_core::config::{
    dedup::{node_fingerprint, node_public_id},
    group::{NodeIndex, node_target_id, valid_group_name},
    model::proxy_groups_from_yaml,
};
use sockrocket_core::{Node, ProxyGroupConfig, RoutingRule};

fn nodes(map: &Mapping) -> Vec<Node> {
    conf_seq(map, "nodes")
        .iter()
        .cloned()
        .map(Node::from_yaml_value)
        .collect::<Result<_, _>>()
        .unwrap_or_default()
}
fn groups_and_rules(map: &Mapping) -> Option<(Vec<ProxyGroupConfig>, Vec<RoutingRule>)> {
    let groups = proxy_groups_from_yaml(map).ok()?;
    let rules = load_routing_rules(map);
    (rules.len() == conf_seq(map, "rules").len()).then_some((groups, rules))
}

fn selected_index(group: &ProxyGroupConfig, index: &NodeIndex) -> Option<usize> {
    if group.gtype == sockrocket_core::GroupType::Fallback {
        group.members.iter().find_map(|fp| index.get(fp))
    } else {
        index.selected_member(group)
    }
}

fn group_catalog(groups: &[ProxyGroupConfig], nodes: &[Node]) -> J {
    if groups.is_empty() {
        return J::Array(Vec::new());
    }
    let index = NodeIndex::new(nodes);
    let rows: Vec<_> = groups.iter().map(|group| {
        let selected = selected_index(group, &index);
        json!({
            "name": group.name, "type": group.gtype,
            "members": group.members.iter().map(|fp| node_public_id(fp)).collect::<Vec<_>>(),
            "current": group.current.as_deref().map(node_public_id),
            "selected_node": selected.map(|i| &nodes[i].name),
            "missing_members": group.members.iter().filter(|fp| index.get(fp).is_none()).count(),
        })
    }).collect();
    J::Array(rows)
}

pub(super) fn trace_group(
    map: &Mapping,
    name: &str,
) -> Option<(sockrocket_core::GroupType, Option<String>)> {
    let group = proxy_groups_from_yaml(map)
        .ok()?
        .into_iter()
        .find(|g| g.name == name)?;
    let ns = nodes(map);
    let selected = selected_index(&group, &NodeIndex::new(&ns)).map(|i| ns[i].name.clone());
    Some((group.gtype, selected))
}

fn node_catalog(nodes: &[Node], id_key: &str) -> J {
    J::Array(
        nodes
            .iter()
            .map(|node| json!({"name": node.name, (id_key): node_public_id(&node_fingerprint(node))}))
            .collect(),
    )
}

pub(super) fn rule_catalogs(map: &Mapping) -> (J, J) {
    let groups = proxy_groups_from_yaml(map).unwrap_or_default();
    let nodes = nodes(map);
    (group_catalog(&groups, &nodes), node_catalog(&nodes, "id"))
}

pub(super) fn get() -> J {
    let Ok(map) = load_conf() else {
        return err_msg("配置读取失败");
    };
    let Some((groups, rules)) = groups_and_rules(&map) else {
        return err_msg("现有规则或分组配置损坏");
    };
    let nodes = nodes(&map);
    json!({"ok":true,"groups":group_catalog(&groups, &nodes),"revision":route_trace::revision_for_rules(&map, &rules),
        "nodes":node_catalog(&nodes, "fingerprint")})
}
pub(super) fn node_name(map: &Mapping, id: &str) -> Option<String> {
    nodes(map)
        .into_iter()
        .find(|node| node_public_id(&node_fingerprint(node)) == id)
        .map(|node| node.name)
}
pub(super) fn valid_target(target: &str) -> bool {
    RULE_TARGETS.contains(&target)
        || node_target_id(target).is_some()
        || target.strip_prefix("group:").is_some_and(|name| {
            load_conf().is_ok_and(|map| {
                proxy_groups_from_yaml(&map)
                    .is_ok_and(|groups| groups.iter().any(|g| g.name == name))
            })
        })
}
pub(super) fn check_revision(map: &Mapping, post: &J) -> bool {
    revision_matches(map, &load_routing_rules(map), post)
}
fn revision_matches(map: &Mapping, rules: &[RoutingRule], post: &J) -> bool {
    post.get("revision")
        .and_then(J::as_str)
        .is_some_and(|expected| expected == route_trace::revision_for_rules(map, rules))
}
pub(super) fn applied(map: &Mapping) -> J {
    if save_conf(map).is_err() {
        return err_msg("保存失败：检查分组引用与存储状态，原配置未覆盖");
    }
    if is_running() && run_sh("restart").is_err() {
        return json!({"ok":false,"saved":true,"applied":false,"msg":"配置已保存，但服务应用失败，请查看诊断后重试"});
    }
    json!({"ok":true,"saved":true,"msg":"配置已保存；运行中的服务已应用，停止状态保持停止"})
}
pub(super) fn save(post: &J) -> J {
    let Ok(mut map) = load_conf() else {
        return err_msg("配置读取失败");
    };
    let Some((mut gs, mut rules)) = groups_and_rules(&map) else {
        return err_msg("现有规则或分组配置损坏，请先修复；未保存更改");
    };
    if !revision_matches(&map, &rules, post) {
        return err_msg("配置已变化，请刷新后重试");
    }
    let old = post.get("original_name").and_then(J::as_str).unwrap_or("");
    let Ok(mut group) =
        serde_json::from_value::<ProxyGroupConfig>(post.get("group").cloned().unwrap_or(J::Null))
    else {
        return err_msg("节点分组格式无效");
    };
    if !valid_group_name(&group.name) {
        return err_msg("名称限 96 字节，使用文字、数字、横线或下划线");
    }
    if gs.iter().any(|g| g.name == group.name && g.name != old) {
        return err_msg("分组名称已存在");
    }
    let position = gs.iter().position(|g| g.name == old);
    if !old.is_empty() && position.is_none() {
        return err_msg("原分组已不存在，请刷新");
    }
    let mut unique = std::collections::HashSet::new();
    group.members.retain(|fp| unique.insert(fp.clone()));
    let ns = nodes(&map);
    let previous = position.map(|i| &gs[i]);
    // Legacy fingerprints can contain credentials. Resolve the opaque IDs from the API locally.
    let identities: std::collections::HashMap<String, String> = ns
        .iter()
        .map(node_fingerprint)
        .chain(previous.into_iter().flat_map(|g| g.members.iter().cloned()))
        .map(|fp| (node_public_id(&fp), fp))
        .collect();
    if group.members.iter().any(|id| !identities.contains_key(id)) {
        return err_msg("分组成员已变化，请刷新后重试");
    }
    if group
        .current
        .as_ref()
        .is_some_and(|id| !group.members.contains(id))
    {
        return err_msg("当前成员必须属于此分组");
    }
    group.members = group
        .members
        .iter()
        .map(|id| identities[id].clone())
        .collect();
    group.current = group.current.and_then(|id| identities.get(&id).cloned());
    if group.members.is_empty() || group.members.len() > 256 {
        return err_msg("请选择 1–256 个有效节点；订阅中失效的原成员可保留或移除");
    }
    if group.current.is_none() {
        group.current = group.members.first().cloned();
    }
    let Ok(mut links) = devices::links(&map) else {
        return err_msg("设备关联配置无效");
    };
    // A rename rewrites all references, including disabled/device rules.
    if !old.is_empty() && old != group.name {
        let from = format!("group:{old}");
        let to = format!("group:{}", group.name);
        for link in &mut links {
            if link.target == from {
                link.target.clone_from(&to);
            }
        }
        for rule in &mut rules {
            if rule.target == from {
                rule.target.clone_from(&to);
            }
        }
    }
    if let Some(i) = position {
        gs[i] = group;
    } else {
        gs.push(group);
    }
    devices::store_links(&mut map, &links);
    save_routing_rules(&mut map, rules);
    map.insert(ykey("groups"), serde_yaml::to_value(gs).unwrap());
    applied(&map)
}
pub(super) fn delete(post: &J) -> J {
    let Ok(mut map) = load_conf() else {
        return err_msg("配置读取失败");
    };
    let Some((mut gs, rules)) = groups_and_rules(&map) else {
        return err_msg("现有规则或分组配置损坏，请先修复；未保存更改");
    };
    if !revision_matches(&map, &rules, post) {
        return err_msg("配置已变化，请刷新后重试");
    }
    let name = post.get("name").and_then(J::as_str).unwrap_or("");
    let target = format!("group:{name}");
    if devices::links(&map).is_ok_and(|links| links.iter().any(|b| b.target == target))
        || rules.iter().any(|r| r.target == target)
    {
        return err_msg("仍有规则或设备策略引用此分组，请先调整出口");
    }
    gs.retain(|g| g.name != name);
    map.insert(ykey("groups"), serde_yaml::to_value(gs).unwrap());
    applied(&map)
}
pub(super) fn builtin_route(map: &Mapping) -> Result<sockrocket_core::config::model::BuiltinRoute> {
    Ok(map
        .get("builtin_route")
        .cloned()
        .map(serde_yaml::from_value)
        .transpose()?
        .unwrap_or_default())
}

pub(super) fn save_builtin_route(post: &J) -> J {
    use sockrocket_core::config::model::BuiltinRoute;
    let Ok(mut map) = load_conf() else {
        return err_msg("配置读取失败");
    };
    if !groups_and_rules(&map).is_some_and(|(_, rules)| revision_matches(&map, &rules, post)) {
        return err_msg("配置已变化或无效，请刷新后重试");
    }
    let Ok(target) = serde_json::from_value::<BuiltinRoute>(post["target"].clone()) else {
        return err_msg("请选择直连、代理或拒绝");
    };
    map.insert(ykey("builtin_route"), serde_yaml::to_value(target).unwrap());
    if target != BuiltinRoute::Direct {
        map.insert(ykey("cn_ipset_direct"), Y::Bool(false));
    }
    applied(&map)
}

pub(super) fn device(post: &J) -> J {
    let Ok(mut map) = load_conf() else {
        return err_msg("配置读取失败");
    };
    let Some((_, mut rules)) = groups_and_rules(&map) else {
        return err_msg("现有规则或分组配置损坏，请先修复；未保存更改");
    };
    if !revision_matches(&map, &rules, post) {
        return err_msg("配置已变化，请刷新后重试");
    }
    let source = match requested_rule_source(post, "") {
        Ok(v) => v,
        Err(e) => return err_msg(&e),
    };
    if source.is_empty() && post["all_devices"].as_bool() != Some(true) {
        return err_msg("请选择设备或明确选择全局设备");
    }
    let target = post.get("target").and_then(J::as_str).unwrap_or("");
    if target != "inherit" && !valid_target(target) {
        return err_msg("请选择有效的默认出口");
    }
    // This editor owns the final policy for exactly this source. Destination
    // exceptions stay intact. Reset removes only the final for this source.
    let same_final = |r: &sockrocket_core::RoutingRule| {
        sockrocket_core::router::normalize_rule_source(&r.source)
            .ok()
            .as_deref()
            == Some(source.as_str())
            && rule_kind(&r.rule_type) == "final"
    };
    if target == "inherit" {
        rules.retain(|r| !same_final(r));
    } else if let Some(rule) = rules
        .iter_mut()
        .filter(|r| r.enabled && same_final(r))
        .min_by_key(|r| std::cmp::Reverse(r.priority))
    {
        rule.target = target.into();
    } else {
        let (name, group) = if source.is_empty() {
            ("全局设备兜底", "")
        } else {
            ("默认出口", "设备默认策略")
        };
        rules.push(sockrocket_core::RoutingRule {
            source,
            name: name.into(),
            group: group.into(),
            rule_type: "final".into(),
            pattern: "*".into(),
            target: target.into(),
            enabled: true,
            priority: 0,
        });
    }
    save_routing_rules(&mut map, rules);
    applied(&map)
}

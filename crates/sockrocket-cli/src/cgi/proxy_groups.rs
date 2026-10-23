//! Group and device policy mutations share the config transaction with rules.
use super::*;
use sockrocket_core::config::{
    dedup::node_fingerprint,
    group::{resolve_members, selected_member, valid_group_name},
};
use sockrocket_core::{Node, ProxyGroupConfig};

// Legacy config fingerprints contain credentials. Only SHA-256 opaque IDs
// cross the UI/API boundary; raw identities stay in the private config file.
fn member_id(value: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn groups(map: &Mapping) -> Vec<ProxyGroupConfig> {
    serde_yaml::from_value(
        map.get(ykey("groups"))
            .cloned()
            .unwrap_or(Y::Sequence(vec![])),
    )
    .unwrap_or_default()
}
fn nodes(map: &Mapping) -> Vec<Node> {
    serde_yaml::from_value(Y::Sequence(
        conf_seq(map, "nodes").into_iter().cloned().collect(),
    ))
    .unwrap_or_default()
}
fn readable(map: &Mapping) -> bool {
    serde_yaml::from_value::<Vec<ProxyGroupConfig>>(
        map.get(ykey("groups"))
            .cloned()
            .unwrap_or(Y::Sequence(vec![])),
    )
    .is_ok()
        && load_routing_rules(map).len() == conf_seq(map, "rules").len()
}
pub(super) fn catalog(map: &Mapping) -> J {
    let ns = nodes(map);
    json!(groups(map).iter().map(|g| json!({"name":g.name,"type":g.gtype,"members":g.members.iter().map(|fp|member_id(fp)).collect::<Vec<_>>(),"current":g.current.as_deref().map(member_id),
        "selected_node":(if g.gtype==sockrocket_core::GroupType::Fallback {resolve_members(g,&ns).first().copied()} else {selected_member(g,&ns)}).map(|i|ns[i].name.clone()),
        "missing_members":g.members.iter().filter(|fp|!ns.iter().any(|n|node_fingerprint(n)==**fp)).count()})).collect::<Vec<_>>())
}
pub(super) fn get() -> J {
    let Ok(map) = load_conf() else {
        return err_msg("配置读取失败");
    };
    if !readable(&map) {
        return err_msg("现有规则或分组配置损坏");
    }
    json!({"ok":true,"groups":catalog(&map),"revision":route_trace::rules_revision(&map),
        "nodes":nodes(&map).iter().map(|n|json!({"name":n.name,"fingerprint":member_id(&node_fingerprint(n))})).collect::<Vec<_>>()})
}
pub(super) fn valid_target(target: &str) -> bool {
    RULE_TARGETS.contains(&target)
        || target.strip_prefix("group:").is_some_and(|name| {
            load_conf().is_ok_and(|map| groups(&map).iter().any(|g| g.name == name))
        })
}
fn check_revision(map: &Mapping, post: &J) -> bool {
    post.get("revision").and_then(J::as_str) == Some(route_trace::rules_revision(map).as_str())
}
fn applied(map: &Mapping) -> J {
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
    if !readable(&map) {
        return err_msg("现有规则或分组配置损坏，请先修复；未保存更改");
    }
    if !check_revision(&map, post) {
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
    let mut gs = groups(&map);
    if gs.iter().any(|g| g.name == group.name && g.name != old) {
        return err_msg("分组名称已存在");
    }
    if !old.is_empty() && !gs.iter().any(|g| g.name == old) {
        return err_msg("原分组已不存在，请刷新");
    }
    let mut unique = std::collections::HashSet::new();
    group.members.retain(|fp| unique.insert(fp.clone()));
    let ns = nodes(&map);
    let previous = gs.iter().find(|g| g.name == old);
    let identities: std::collections::HashMap<String, String> = ns
        .iter()
        .map(node_fingerprint)
        .chain(previous.into_iter().flat_map(|g| g.members.iter().cloned()))
        .map(|fp| (member_id(&fp), fp))
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
    if group.members.is_empty()
        || group.members.len() > 256
        || group.members.iter().any(|fp| {
            !ns.iter().any(|n| node_fingerprint(n) == *fp)
                && !previous.is_some_and(|g| g.members.contains(fp))
        })
    {
        return err_msg("请选择 1–256 个有效节点；订阅中失效的原成员可保留或移除");
    }
    if !group
        .current
        .as_ref()
        .is_some_and(|fp| group.members.contains(fp))
    {
        group.current = group.members.first().cloned();
    }
    if let Some(i) = gs.iter().position(|g| g.name == old) {
        gs[i] = group.clone();
    } else {
        gs.push(group.clone());
    }
    // A rename rewrites all references, including disabled/device rules.
    let mut rules = load_routing_rules(&map);
    if !old.is_empty() {
        for rule in &mut rules {
            if rule.target == format!("group:{old}") {
                rule.target = format!("group:{}", group.name);
            }
        }
    }
    save_routing_rules(&mut map, rules);
    map.insert(ykey("groups"), serde_yaml::to_value(gs).unwrap());
    applied(&map)
}
pub(super) fn delete(post: &J) -> J {
    let Ok(mut map) = load_conf() else {
        return err_msg("配置读取失败");
    };
    if !readable(&map) {
        return err_msg("现有规则或分组配置损坏，请先修复；未保存更改");
    }
    if !check_revision(&map, post) {
        return err_msg("配置已变化，请刷新后重试");
    }
    let name = post.get("name").and_then(J::as_str).unwrap_or("");
    if load_routing_rules(&map)
        .iter()
        .any(|r| r.target == format!("group:{name}"))
    {
        return err_msg("仍有规则或设备策略引用此分组，请先调整出口");
    }
    let mut gs = groups(&map);
    gs.retain(|g| g.name != name);
    map.insert(ykey("groups"), serde_yaml::to_value(gs).unwrap());
    applied(&map)
}
pub(super) fn device(post: &J) -> J {
    let Ok(mut map) = load_conf() else {
        return err_msg("配置读取失败");
    };
    if !readable(&map) {
        return err_msg("现有规则或分组配置损坏，请先修复；未保存更改");
    }
    if !check_revision(&map, post) {
        return err_msg("配置已变化，请刷新后重试");
    }
    let source = match requested_rule_source(post, "") {
        Ok(v) => v,
        Err(e) => return err_msg(&e),
    };
    let target = post.get("target").and_then(J::as_str).unwrap_or("");
    if target != "inherit" && !valid_target(target) {
        return err_msg("请选择有效的默认出口");
    }
    let mut rules = load_routing_rules(&map);
    // This editor owns the final policy for exactly this source. Destination
    // exceptions stay intact. Reset removes the final and resumes inheritance.
    rules.retain(|r| {
        !(sockrocket_core::router::normalize_rule_source(&r.source)
            .ok()
            .as_deref()
            == Some(source.as_str())
            && matches!(rule_kind(&r.rule_type), "final" | "match"))
    });
    if target != "inherit" {
        rules.push(sockrocket_core::RoutingRule {
            source,
            name: "默认出口".into(),
            group: "设备默认策略".into(),
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

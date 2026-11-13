//! Versioned connection identities and migration of saved node references.
use super::{dedup, model::Node};
use serde_json::Value;
use std::collections::HashMap;

fn lowercase_field(value: &mut Value, key: &str) {
    if let Some(Value::String(text)) = value.get_mut(key) {
        *text = text.to_ascii_lowercase();
    }
}

pub(super) fn fingerprint(node: &Node) -> String {
    let mut protocol = serde_json::to_value(&node.protocol).expect("serializable protocol");
    for key in ["uuid", "cipher"] {
        lowercase_field(&mut protocol, key);
    }
    if let Some(shadow) = protocol.get_mut("shadow_tls") {
        lowercase_field(shadow, "host");
    }
    let mut transport = serde_json::to_value(&node.transport).expect("serializable transport");
    if let Some(ws) = transport.get_mut("ws") {
        lowercase_field(ws, "host");
    }
    for kind in ["tls", "reality"] {
        if let Some(settings) = transport.get_mut(kind) {
            lowercase_field(settings, "sni");
        }
    }
    let extra = [
        super::ss_plugin::KEY,
        super::v2ray::VLESS_ENCRYPTION,
        super::v2ray::TLS_CERT_PIN,
        super::v2ray::TUIC_CERT_PIN,
        super::v2ray::TUIC_ZERO_RTT,
        super::v2ray::UDP_ONLY,
        super::v2ray::VMESS_PADDING,
        super::v2ray::VMESS_AUTH_LEN,
        super::v2ray::HY2_UP,
        super::v2ray::HY2_DOWN,
        super::v2ray::HY2_FAST_OPEN,
        super::v2ray::HY2_MPORT,
        super::v2ray::HY2_HOP_INTERVAL,
        super::v2ray::HY2_HOP_MAX,
        super::v2ray::HY2_CLIENT_FP_HINT,
        super::server_dns::KEY,
    ]
    .into_iter()
    .filter_map(|key| {
        let value = node.extra.get(key)?;
        Some((key.to_owned(), Value::String(value.clone())))
    })
    .collect();
    let mut value = Value::Array(vec![
        Value::String(node.server.to_ascii_lowercase()),
        Value::from(node.port),
        protocol,
        transport,
        Value::Object(extra),
    ]);
    // Preserve persisted identities even when serde_json's preserve_order feature is enabled.
    value.sort_all_objects();
    // Credentials participate in equality but no longer appear in group keys.
    format!(
        "v2:{}",
        dedup::node_public_id(&serde_json::to_string(&value).expect("identity JSON"))
    )
}

/// Upgrade only references, preserving unknown Merlin settings and node order.
/// Ambiguous old identities retain the first node, exactly as the old lookup
/// did. New identities can distinguish all variants after migration.
pub fn migrate_references(raw: &mut serde_yaml::Value) -> Result<bool, serde_yaml::Error> {
    let Some(map) = raw.as_mapping_mut() else {
        return Ok(false);
    };
    let key = |s: &str| serde_yaml::Value::String(s.into());
    let has_references = map
        .get(key("groups"))
        .and_then(serde_yaml::Value::as_sequence)
        .is_some_and(|v| !v.is_empty())
        || map
            .get(key("rules"))
            .and_then(serde_yaml::Value::as_sequence)
            .is_some_and(|rules| {
                rules.iter().any(|rule| {
                    rule.get("target")
                        .and_then(serde_yaml::Value::as_str)
                        .is_some_and(|t| t.starts_with("node:"))
                })
            });
    if !has_references {
        return Ok(false);
    }
    let nodes: Vec<Node> = serde_yaml::from_value(
        map.get(key("nodes"))
            .cloned()
            .unwrap_or(serde_yaml::Value::Sequence(vec![])),
    )?;
    let mut members = HashMap::new();
    let mut targets = HashMap::new();
    for node in &nodes {
        let old = dedup::legacy_node_fingerprint(node);
        let new = fingerprint(node);
        targets
            .entry(format!("node:{}", dedup::node_public_id(&old)))
            .or_insert_with(|| format!("node:{}", dedup::node_public_id(&new)));
        members.entry(old).or_insert(new);
    }
    let mut changed = false;
    let mut replace = |value: &mut serde_yaml::Value, replacements: &HashMap<String, String>| {
        if let Some(new) = value.as_str().and_then(|s| replacements.get(s)) {
            *value = serde_yaml::Value::String(new.clone());
            changed = true;
        }
    };
    if let Some(groups) = map
        .get_mut(key("groups"))
        .and_then(serde_yaml::Value::as_sequence_mut)
    {
        for group in groups {
            if let Some(current) = group.get_mut("current") {
                replace(current, &members);
            }
            if let Some(list) = group
                .get_mut("members")
                .and_then(serde_yaml::Value::as_sequence_mut)
            {
                for member in list {
                    replace(member, &members);
                }
            }
        }
    }
    if let Some(rules) = map
        .get_mut(key("rules"))
        .and_then(serde_yaml::Value::as_sequence_mut)
    {
        for rule in rules {
            if let Some(target) = rule.get_mut("target") {
                replace(target, &targets);
            }
        }
    }
    Ok(changed)
}

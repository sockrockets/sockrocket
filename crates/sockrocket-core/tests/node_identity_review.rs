use sockrocket_core::config::{model::*, node_identity::migrate_references};
use sockrocket_core::{AppConfig, Node, node_fingerprint, node_public_id};

fn node() -> Node {
    Node {
        name: "example".into(),
        server: "node.example".into(),
        port: 443,
        protocol: ProxyProtocol::VLess {
            uuid: "test-uuid".into(),
            flow: None,
            udp: true,
        },
        transport: Some(TransportConfig {
            grpc: None,
            transport_type: TransportType::Tls,
            tls: Some(TlsConfig {
                sni: Some("one.example".into()),
                skip_cert_verify: false,
                alpn: None,
                fingerprint: Some("chrome".into()),
            }),
            ws: None,
            xhttp: None,
            reality: None,
        }),
        latency_ms: None,
        tags: vec![],
        extra: Default::default(),
    }
}

#[test]
fn connection_parameters_are_distinct_but_metadata_and_header_order_are_not() {
    let base = node();
    // This identifier is persisted by groups, routing rules, and traffic history.
    assert_eq!(
        node_fingerprint(&base),
        "v2:1f0a7c5b4f6e48fd3df953d071679d00bab7fa576fa4de6f38b0ced6ce67616e"
    );
    let mut other = base.clone();
    other.transport.as_mut().unwrap().tls.as_mut().unwrap().sni = Some("two.example".into());
    assert_ne!(node_fingerprint(&base), node_fingerprint(&other));
    other = base.clone();
    other
        .transport
        .as_mut()
        .unwrap()
        .tls
        .as_mut()
        .unwrap()
        .skip_cert_verify = true;
    assert_ne!(node_fingerprint(&base), node_fingerprint(&other));
    let mut a = base.clone();
    a.transport.as_mut().unwrap().reality = Some(RealityConfig {
        public_key: "test-key".into(),
        short_id: "01".into(),
        sni: None,
        client_fingerprint: None,
        skip_cert_verify: false,
    });
    let mut b = a.clone();
    b.transport
        .as_mut()
        .unwrap()
        .reality
        .as_mut()
        .unwrap()
        .short_id = "02".into();
    assert_ne!(node_fingerprint(&a), node_fingerprint(&b));
    let ws = |pairs: &[(&str, &str)]| WsConfig {
        path: Some("/ws".into()),
        host: None,
        headers: Some(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        ),
    };
    a = base.clone();
    a.transport.as_mut().unwrap().ws = Some(ws(&[("X-A", "1"), ("X-B", "2")]));
    b = a.clone();
    b.transport.as_mut().unwrap().ws = Some(ws(&[("X-B", "2"), ("X-A", "1")]));
    b.name = "renamed".into();
    b.latency_ms = Some(12);
    b.tags.push("tag".into());
    b.extra
        .insert("sub_url".into(), "https://example.invalid/sub".into());
    assert_eq!(node_fingerprint(&a), node_fingerprint(&b));
    b.transport
        .as_mut()
        .unwrap()
        .ws
        .as_mut()
        .unwrap()
        .headers
        .as_mut()
        .unwrap()
        .insert("X-A".into(), "3".into());
    assert_ne!(node_fingerprint(&a), node_fingerprint(&b));
}

#[test]
fn old_groups_and_pinned_rules_migrate_once_preserving_first_match_and_unknown_keys() {
    let first = node();
    let mut second = first.clone();
    second.transport.as_mut().unwrap().tls.as_mut().unwrap().sni = Some("two.example".into());
    let legacy = "node.example|443|vless|test-uuid||tls";
    let config = AppConfig {
        nodes: vec![first.clone(), second],
        groups: vec![ProxyGroupConfig {
            name: "Example".into(),
            gtype: GroupType::Select,
            members: vec![legacy.into(), "missing".into()],
            current: Some(legacy.into()),
        }],
        rules: vec![RoutingRule {
            source: String::new(),
            name: String::new(),
            group: String::new(),
            rule_type: "final".into(),
            pattern: "*".into(),
            target: format!("node:{}", node_public_id(legacy)),
            enabled: true,
            priority: 0,
        }],
        ..Default::default()
    };
    let text = format!(
        "{}\nmerlin_private_setting: retained\n",
        serde_yaml::to_string(&config).unwrap()
    );
    let mut raw: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
    assert!(migrate_references(&mut raw).unwrap());
    assert!(!migrate_references(&mut raw).unwrap());
    assert_eq!(raw["merlin_private_setting"].as_str(), Some("retained"));
    let migrated = AppConfig::from_yaml_str(&text).unwrap();
    assert_eq!(
        migrated.groups[0].current.as_deref(),
        Some(node_fingerprint(&first).as_str())
    );
    assert_eq!(migrated.groups[0].members[1], "missing");
    assert_eq!(
        migrated.rules[0].target,
        format!("node:{}", node_public_id(&node_fingerprint(&first)))
    );
    assert_eq!(
        sockrocket_core::config::group::selected_member(&migrated.groups[0], &migrated.nodes),
        Some(0)
    );
}

#[test]
fn enabled_invalid_rules_fail_configuration_validation() {
    let mut config = AppConfig::default();
    for (kind, pattern, target, source) in [
        ("domain-sufix", "example.com", "proxy", ""),
        ("ip-cidr", "192.0.2.0/99", "direct", ""),
        ("dst-port", "65536", "reject", ""),
        ("domain", "example.com", "proxxy", ""),
        ("domain", "example.com", "proxy", "invalid-client"),
    ] {
        config.rules = vec![RoutingRule {
            rule_type: kind.into(),
            pattern: pattern.into(),
            target: target.into(),
            source: source.into(),
            name: String::new(),
            group: String::new(),
            enabled: true,
            priority: 0,
        }];
        assert!(AppConfig::from_yaml_str(&serde_yaml::to_string(&config).unwrap()).is_err());
        config.rules[0].enabled = false;
        if source.is_empty() {
            assert!(AppConfig::from_yaml_str(&serde_yaml::to_string(&config).unwrap()).is_ok());
        }
    }
}

#[test]
fn historical_traffic_is_merged_once_into_new_identity() {
    let node = node();
    let old = node_public_id("node.example|443|vless|test-uuid||tls");
    let new = node_public_id(&node_fingerprint(&node));
    let path = std::env::temp_dir().join(format!("identity-traffic-{}.yaml", uuid::Uuid::new_v4()));
    let month = sockrocket_core::traffic::recent_month_keys(1)
        .pop()
        .unwrap();
    let history = serde_json::json!({"history": {
        old: {"name":"old", "months":{&month:{"up":10,"down":20}}},
        &new: {"name":"new", "months":{&month:{"up":1,"down":2}}}
    }});
    std::fs::write(&path, serde_yaml::to_string(&history).unwrap()).unwrap();
    let ledger = sockrocket_core::traffic::TrafficLedger::load_or_new(&path);
    ledger.migrate_node_identities(std::slice::from_ref(&node));
    ledger.migrate_node_identities(std::slice::from_ref(&node));
    let rows = ledger.snapshots();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, new);
    assert_eq!((rows[0].history.up, rows[0].history.down), (11, 22));
    ledger.flush().unwrap();
    drop(ledger);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn subscription_write_migrates_references_before_old_nodes_disappear() {
    let old = "node.example|443|vless|test-uuid||tls";
    let config = AppConfig {
        nodes: vec![node()],
        groups: vec![ProxyGroupConfig {
            name: "Example".into(),
            gtype: GroupType::Select,
            members: vec![old.into()],
            current: Some(old.into()),
        }],
        ..Default::default()
    };
    let dir = std::env::temp_dir().join(format!("identity-write-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("config.yaml");
    std::fs::write(&path, serde_yaml::to_string(&config).unwrap()).unwrap();
    let replacement = AppConfig {
        nodes: vec![],
        ..config
    };
    sockrocket_core::config::storage::write_nodes_and_subscriptions(&path, &replacement).unwrap();
    let saved = AppConfig::from_yaml_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(saved.groups[0].current, Some(node_fingerprint(&node())));
    assert!(
        sockrocket_core::config::group::selected_member(&saved.groups[0], &saved.nodes).is_none()
    );
    std::fs::remove_dir_all(dir).unwrap();
}

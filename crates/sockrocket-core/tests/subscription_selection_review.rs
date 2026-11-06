use sockrocket_core::{AppConfig, Node, ProxyProtocol, Subscription, apply_subscription_refresh};
fn node(server: &str) -> Node {
    Node {
        name: "Tokyo".into(),
        server: server.into(),
        port: 443,
        protocol: ProxyProtocol::Trojan {
            password: "synthetic-only".into(),
            udp: true,
        },
        transport: None,
        latency_ms: None,
        tags: vec![],
        extra: Default::default(),
    }
}
#[test]
fn unique_name_within_selected_subscription_survives_automatic_display_suffix() {
    let a = "https://example.com/a";
    let b = "https://example.com/b";
    let mut c = AppConfig::default();
    for url in [a, b] {
        c.subscriptions.push(Subscription {
            name: url.into(),
            url: url.into(),
            format: "auto".into(),
            last_updated: None,
            refresh_interval_hours: 24,
            refresh_revision: String::new(),
        });
    }
    assert!(apply_subscription_refresh(
        &mut c,
        vec![
            (a.into(), vec![node("192.0.2.1")]),
            (b.into(), vec![node("192.0.2.2")])
        ]
    ));
    assert_eq!(c.nodes[1].name, "Tokyo #2");
    c.active_node = Some(1);
    // The provider name must survive persistence and a daemon restart.
    c = serde_yaml::from_str(&serde_yaml::to_string(&c).unwrap()).unwrap();
    assert!(apply_subscription_refresh(
        &mut c,
        vec![(b.into(), vec![node("192.0.2.3")])]
    ));
    assert_eq!(
        c.active_node.map(|i| c.nodes[i].server.as_str()),
        Some("192.0.2.3")
    );
    assert!(apply_subscription_refresh(
        &mut c,
        vec![(b.into(), vec![node("192.0.2.4")])]
    ));
    assert_eq!(
        c.active_node.map(|i| c.nodes[i].server.as_str()),
        Some("192.0.2.4")
    );
}

#[test]
fn literal_suffix_is_not_stripped_and_ambiguous_names_are_not_guessed() {
    let url = "https://example.com/a";
    let mut c = AppConfig::default();
    c.subscriptions.push(Subscription {
        name: "A".into(),
        url: url.into(),
        format: "auto".into(),
        last_updated: None,
        refresh_interval_hours: 24,
        refresh_revision: String::new(),
    });
    let mut literal = node("192.0.2.2");
    literal.name = "Tokyo #2".into();
    apply_subscription_refresh(
        &mut c,
        vec![(url.into(), vec![node("192.0.2.1"), literal.clone()])],
    );
    c.active_node = Some(1);
    literal.server = "192.0.2.3".into();
    apply_subscription_refresh(
        &mut c,
        vec![(url.into(), vec![node("192.0.2.4"), literal.clone()])],
    );
    assert_eq!(
        c.active_node.map(|i| c.nodes[i].server.as_str()),
        Some("192.0.2.3")
    );
    let mut second = literal.clone();
    second.server = "192.0.2.5".into();
    literal.server = "192.0.2.6".into();
    apply_subscription_refresh(&mut c, vec![(url.into(), vec![second, literal])]);
    assert_eq!(c.active_node, None);
}

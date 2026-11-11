use serde_json::json;
use sockrocket_core::config::{
    subscription::parse_subscription_content,
    v2ray::{node_to_share_uri, parse_proxy_uri},
};

#[test]
fn reality_imports_preserve_endpoint_password_and_handshake() {
    let key = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    let uri = format!(
        "trojan://p%40%252F@node.invalid:443?security=reality&pbk={key}&sid=abcd&sni=tls.invalid&fp=firefox#test"
    );
    let clash = format!(
        "proxies:\n - {{name: test, type: trojan, server: node.invalid, port: 443, password: 'p@%2F', udp: true, sni: tls.invalid, client-fingerprint: firefox, reality-opts: {{public-key: {key}, short-id: abcd}}}}"
    );
    let sing = json!({"outbounds":[{"type":"trojan","tag":"test","server":"node.invalid","server_port":443,"password":"p@%2F","tls":{"enabled":true,"server_name":"tls.invalid","reality":{"enabled":true,"public_key":key,"short_id":"abcd"},"utls":{"enabled":true,"fingerprint":"firefox"}}}]});
    let endpoint = json!({"address":"node.invalid","port":443,"password":"p@%2F"});
    let reference = parse_proxy_uri(&uri).unwrap();
    let expected = serde_json::to_value(&reference).unwrap();
    assert_eq!(expected["protocol"]["password"], "p@%2F");
    let roundtrip = parse_proxy_uri(&node_to_share_uri(&reference)).unwrap();
    assert_eq!(serde_json::to_value(roundtrip).unwrap(), expected);
    let mut inputs = vec![clash, sing.to_string()];
    for settings in [endpoint.clone(), json!({"servers":[endpoint]})] {
        inputs.push(json!({"outbounds":[{"protocol":"trojan","tag":"test","settings":settings,"streamSettings":{"security":"reality","realitySettings":{"password":key,"shortId":"abcd","serverName":"tls.invalid","fingerprint":"firefox"}}}]}).to_string());
    }
    for input in inputs {
        let nodes = parse_subscription_content(&input, "auto").unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(serde_json::to_value(&nodes[0]).unwrap(), expected);
    }
}

#[tokio::test]
async fn udp_only_rejects_streams_before_connecting_and_survives_export() {
    let input = json!({"type":"trojan","server":"example.invalid","server_port":443,
        "password":"fixture","network":"udp"});
    let node = sockrocket_core::parse_singbox_outbound(&input.to_string())
        .unwrap()
        .unwrap();
    let yaml = sockrocket_core::export_clash_config(std::slice::from_ref(&node));
    for n in [
        parse_proxy_uri(&node_to_share_uri(&node)).unwrap(),
        sockrocket_core::parse_clash_config(&yaml)
            .unwrap()
            .remove(0),
        node,
    ] {
        let out = sockrocket_core::create_outbound_unwarmed(&n).unwrap();
        let error = out
            .connect("localhost", 80)
            .await
            .err()
            .expect("TCP must be rejected");
        assert!(error.to_string().contains("UDP traffic only"));
        assert!(n.protocol.supports_udp());
    }
}

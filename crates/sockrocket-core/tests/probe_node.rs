//! Parametrized single-node probe with full error chains.
//! Run: NODE="<name>" cargo test -p sockrocket-core --test probe_node -- --ignored --nocapture
//! Local REALITY probe reads UUID / keys from `.local/live.env` when set.

mod common;

use sockrocket_core::config::model::AppConfig;
use sockrocket_core::proxy::connector::Outbound;
use sockrocket_core::proxy::factory::create_outbound;

/// Probe REALITY client against a local xray on 127.0.0.1 (config in `.local/`).
#[tokio::test]
#[ignore = "requires local xray reality server"]
async fn probe_local_reality() {
    use sockrocket_core::config::model::*;

    let env = common::load_env_file("live.env");
    let uuid = common::optional(&env, "LIVE_UUID")
        .unwrap_or_else(|| "00000000-0000-0000-0000-000000000001".into());
    let public_key = common::optional(&env, "LIVE_REALITY_PUBLIC_KEY")
        .unwrap_or_else(|| "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into());
    let short_id = common::optional(&env, "LIVE_REALITY_SHORT_ID")
        .unwrap_or_else(|| "0123456789abcdef".into());

    let node = Node {
        name: "local-reality".into(),
        server: "127.0.0.1".into(),
        port: 14449,
        protocol: ProxyProtocol::VLess {
            uuid,
            flow: None,
            udp: true,
        },
        transport: Some(TransportConfig {
            transport_type: TransportType::Tcp,
            tls: Some(TlsConfig {
                sni: Some("localhost".into()),
                skip_cert_verify: true,
                alpn: None,
                fingerprint: Some("chrome".into()),
            }),
            ws: None,
            reality: Some(RealityConfig {
                public_key,
                short_id,
                sni: Some("localhost".into()),
                client_fingerprint: None,
                skip_cert_verify: true,
            }),
        }),
        latency_ms: None,
        extra: Default::default(),
        tags: vec![],
    };

    let outbound = create_outbound(&node).expect("create_outbound");
    let r = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        outbound.connect("www.gstatic.com", 80),
    )
    .await;
    match r {
        Err(_) => panic!("connect timed out"),
        Ok(Err(e)) => panic!("connect failed: {e:#}"),
        Ok(Ok(_)) => println!("REALITY local handshake OK"),
    }
}

#[tokio::test]
#[ignore = "needs NODE env var and network"]
async fn probe_single_node() {
    let want = std::env::var("NODE").expect("set NODE=<node name>");
    let state_path = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .expect("HOME")
        .join(".config/sockrocket/gui-state.yaml");
    let content = std::fs::read_to_string(&state_path).expect("read gui-state.yaml");
    let config: AppConfig = serde_yaml::from_str(&content).expect("parse gui-state.yaml");
    let mut node = config
        .nodes
        .iter()
        .find(|n| n.name == want)
        .unwrap_or_else(|| panic!("node '{}' not found", want))
        .clone();
    if let Ok(fp) = std::env::var("FP") {
        if let Some(t) = node.transport.as_mut().and_then(|t| t.tls.as_mut()) {
            t.fingerprint = if fp.is_empty() { None } else { Some(fp) };
        }
        println!("fingerprint overridden");
    }
    if let Ok(s) = std::env::var("SERVER") {
        node.server = s;
    }
    if let Ok(p) = std::env::var("PORT") {
        node.port = p.parse().expect("PORT");
    }
    println!("probing: {} ({}:{})", node.name, node.server, node.port);

    let outbound = create_outbound(&node).expect("create_outbound");
    let r = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        outbound.connect("www.gstatic.com", 80),
    )
    .await;
    match r {
        Err(_) => println!("RESULT: connect timed out (20s)"),
        Ok(Err(ref e)) => println!("RESULT: connect failed:\n{e:#}\n\nchain:"),
        Ok(Ok(_)) => println!("RESULT: connect OK"),
    }
    if let Ok(Err(e)) = &r {
        let mut src: &dyn std::error::Error = e.as_ref();
        let mut i = 0;
        while let Some(next) = src.source() {
            println!("  [{i}] {next}");
            src = next;
            i += 1;
            if i > 10 {
                break;
            }
        }
    }
}

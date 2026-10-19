//! VMess+WS live probes. Remote credentials from `.local/live.env`.
//! Run with: cargo test -p sockrocket-core --test vmess_ws_live -- --ignored --nocapture

mod common;

use sockrocket_core::config::model::*;
use sockrocket_core::proxy::connector::Outbound;
use sockrocket_core::proxy::factory::create_outbound;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn local_test_uuid(env: &std::collections::HashMap<String, String>) -> String {
    common::optional(env, "LIVE_UUID")
        .unwrap_or_else(|| "00000000-0000-0000-0000-000000000001".into())
}

#[tokio::test]
#[ignore = "local capture helper"]
async fn vmess_ws_local_capture() {
    let env = common::load_env_file("live.env");
    let uuid = local_test_uuid(&env);
    let node = Node {
        name: "local-capture".into(),
        server: "127.0.0.1".into(),
        port: 14444,
        protocol: ProxyProtocol::VMess {
            uuid,
            alter_id: 0,
            cipher: "none".into(),
            udp: true,
        },
        transport: Some(TransportConfig {
            transport_type: TransportType::WebSocket,
            tls: None,
            ws: Some(WsConfig {
                path: Some("/".into()),
                host: None,
                headers: None,
            }),
            reality: None,
        }),
        latency_ms: None,
        extra: Default::default(),
        tags: vec![],
    };
    let outbound = create_outbound(&node).expect("create outbound");
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        outbound.connect("www.gstatic.com", 80),
    )
    .await;
    println!("done (failure expected; check capture)");
}

#[tokio::test]
#[ignore = "needs LIVE_* in .local/live.env"]
async fn vmess_ws_remote_connectivity() {
    let env = common::load_env_file("live.env");
    let server = common::require(&env, "LIVE_SERVER");
    let port: u16 = common::require(&env, "LIVE_PORT")
        .parse()
        .expect("LIVE_PORT");
    let uuid = common::require(&env, "LIVE_UUID");
    let ws_host = common::optional(&env, "LIVE_WS_HOST");
    let sni = common::optional(&env, "LIVE_SNI");
    let use_tls = common::optional(&env, "LIVE_TLS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let node = Node {
        name: "live-vmess-ws".into(),
        server: server.clone(),
        port,
        protocol: ProxyProtocol::VMess {
            uuid,
            alter_id: 0,
            cipher: "none".into(),
            udp: true,
        },
        transport: Some(TransportConfig {
            transport_type: TransportType::WebSocket,
            tls: if use_tls {
                Some(TlsConfig {
                    sni,
                    skip_cert_verify: true,
                    alpn: None,
                    fingerprint: Some("chrome".into()),
                })
            } else {
                None
            },
            ws: Some(WsConfig {
                path: Some("/".into()),
                host: ws_host.clone(),
                headers: ws_host.map(|h| [("Host".to_string(), h)].into_iter().collect()),
            }),
            reality: None,
        }),
        latency_ms: None,
        extra: Default::default(),
        tags: vec![],
    };

    let outbound = create_outbound(&node).expect("create outbound");
    let mut stream = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        outbound.connect("www.gstatic.com", 80),
    )
    .await
    .expect("connect timed out")
    .unwrap_or_else(|e| panic!("connect failed: {e:#}"));
    println!("connect OK");

    stream
        .write_all(
            b"GET /generate_204 HTTP/1.1\r\nHost: www.gstatic.com\r\nConnection: close\r\n\r\n",
        )
        .await
        .expect("write");

    let mut buf = Vec::new();
    let deadline = tokio::time::sleep(std::time::Duration::from_secs(15));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => panic!("no response within 15s"),
            n = stream.read_buf(&mut buf) => {
                let n = n.expect("read failed");
                if n == 0 { break; }
                if String::from_utf8_lossy(&buf[..buf.len().min(64)]).contains("HTTP/1.") {
                    println!("OK: {} bytes", buf.len());
                    return;
                }
            }
        }
    }
    panic!("closed without HTTP response");
}

/// Local differential test against a local xray on 127.0.0.1 (see `.local/`).
#[tokio::test]
#[ignore = "requires local xray server on 14445"]
async fn vmess_ws_against_local_xray() {
    let env = common::load_env_file("live.env");
    let uuid = local_test_uuid(&env);
    let node = Node {
        name: "local-xray".into(),
        server: "127.0.0.1".into(),
        port: 14445,
        protocol: ProxyProtocol::VMess {
            uuid,
            alter_id: 0,
            cipher: "none".into(),
            udp: true,
        },
        transport: Some(TransportConfig {
            transport_type: TransportType::WebSocket,
            tls: None,
            ws: Some(WsConfig {
                path: Some("/".into()),
                host: None,
                headers: None,
            }),
            reality: None,
        }),
        latency_ms: None,
        extra: Default::default(),
        tags: vec![],
    };

    let outbound = create_outbound(&node).expect("create outbound");
    let mut stream = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        outbound.connect("www.gstatic.com", 80),
    )
    .await
    .expect("connect timed out")
    .unwrap_or_else(|e| panic!("connect failed: {e:#}"));
    println!("connect OK (vmess verified by xray)");

    stream
        .write_all(
            b"GET /generate_204 HTTP/1.1\r\nHost: www.gstatic.com\r\nConnection: close\r\n\r\n",
        )
        .await
        .expect("write");

    let mut buf = Vec::new();
    let deadline = tokio::time::sleep(std::time::Duration::from_secs(15));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => panic!("no response within 15s, got {} bytes", buf.len()),
            n = stream.read_buf(&mut buf) => {
                let n = n.expect("read failed");
                if n == 0 { break; }
                if String::from_utf8_lossy(&buf[..buf.len().min(64)]).contains("HTTP/1.") {
                    println!("OK: {} bytes", buf.len());
                    return;
                }
            }
        }
    }
    panic!("closed without HTTP response");
}

//! Local-only wire interoperability with independently installed reference servers.
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::json;
use sockrocket_core::{create_outbound_unwarmed, parse_singbox_outbound};
use std::{
    process::{Child, Command, Stdio},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, UdpSocket},
};

struct Server(Child, Option<std::path::PathBuf>);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        if let Some(dir) = &self.1 {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn pem(label: &str, bytes: &[u8]) -> String {
    let body = STANDARD.encode(bytes);
    format!("-----BEGIN {label}-----\n{body}\n-----END {label}-----")
}
async fn check(kind: &str, option: Option<&str>, mihomo: bool) -> Result<()> {
    let host = "127.0.0.1";
    let port = if kind == "tuic" {
        UdpSocket::bind((host, 0)).await?.local_addr()?.port()
    } else {
        TcpListener::bind((host, 0)).await?.local_addr()?.port()
    };
    let id = "00000000-0000-4000-8000-000000000001";
    let pw = "local-only";
    let mut server = json!({"type":kind,"listen":host,"listen_port":port});
    let mut client = json!({"type":kind,"server":host,"server_port":port});
    match kind {
        "shadowsocks" => {
            for side in [&mut server, &mut client] {
                side["method"] = json!("aes-128-gcm");
                side["password"] = json!(pw);
            }
        }
        "vmess" => {
            server["users"] = json!([{"uuid":id}]);
            if let Some(security) = option {
                let mut parts = security.split('+');
                client["security"] = json!(parts.next().unwrap());
                for option in parts {
                    client[option] = json!(true);
                }
            }
            client["uuid"] = json!(id);
        }
        "trojan" | "tuic" => {
            let alpn = if kind == "tuic" { "h3" } else { "http/1.1" };
            server["tls"] = json!({"enabled":true,"alpn":[alpn],"certificate":pem("CERTIFICATE",include_bytes!("../src/proxy/testdata/localhost-cert.der")),"key":pem("PRIVATE KEY",include_bytes!("../src/proxy/testdata/localhost-key.der"))});
            client["tls"] =
                json!({"enabled":true,"insecure":true,"server_name":"localhost","alpn":[alpn]});
            if matches!(option, Some("grpc" | "xray_grpc_multi")) {
                let transport = json!({"type":"grpc","service_name":"fixture.Service"});
                server["transport"] = transport.clone();
                client["transport"] = transport;
                if option == Some("xray_grpc_multi") {
                    client["transport"]["multi_mode"] = true.into();
                }
                server["tls"]["alpn"] = json!(["h2"]);
                client["tls"]["alpn"] = json!(["h2"]);
            }
            if option == Some("httpupgrade") {
                let transport = json!({"type":"httpupgrade","host":"echo.example.invalid","path":"/synthetic","headers":{"X-Example":"fixture"}});
                server["transport"] = transport.clone();
                client["transport"] = transport;
            }
            server["users"] = json!([{"password":pw}]);
            client["password"] = json!(pw);
            if kind == "tuic" {
                client["heartbeat"] = json!("0.25s");
                match option {
                    Some("quic") => client["udp_relay_mode"] = json!("quic"),
                    Some("uot_v2") => client["udp_over_stream"] = json!(true),
                    _ => {}
                }
                server["users"][0]["uuid"] = json!(id);
                client["uuid"] = json!(id);
            }
        }
        _ => unreachable!(),
    }
    let mut proc = if mihomo {
        let cmd = std::env::var_os("SOCKROCKET_REFERENCE_MIHOMO")
            .context("Mihomo executable required")?;
        let config = json!({"listeners":[{"name":"test","type":"tuic","listen":host,"port":port,"users":{(id):pw},"certificate":server["tls"]["certificate"],"private-key":server["tls"]["key"],"alpn":["h3"]}],"rules":["MATCH,DIRECT"],"log-level":"error"});
        let dir = std::env::temp_dir().join(format!("sockrocket-mihomo-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir)?;
        let child = Command::new(cmd)
            .arg("-d")
            .arg(&dir)
            .arg("-config")
            .arg(STANDARD.encode(serde_json::to_vec(&config)?))
            .stdout(Stdio::null())
            .spawn();
        if child.is_err() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        Server(child?, Some(dir))
    } else {
        let (conf, cmd, args) = if option == Some("xray_grpc_multi") {
            let cert = server["tls"]["certificate"]
                .as_str()
                .unwrap()
                .lines()
                .collect::<Vec<_>>();
            let key = server["tls"]["key"]
                .as_str()
                .unwrap()
                .lines()
                .collect::<Vec<_>>();
            let config = json!({"log":{"loglevel":"error"},"inbounds":[{"listen":host,"port":port,"protocol":"trojan","settings":{"clients":[{"password":pw}]},"streamSettings":{"network":"grpc","security":"tls","tlsSettings":{"certificates":[{"certificate":cert,"key":key}]},"grpcSettings":{"serviceName":"fixture.Service","multiMode":true}}}],"outbounds":[{"protocol":"freedom","settings":{"finalRules":[{"action":"allow","ip":["127.0.0.1"]}]}}]});
            (
                config,
                std::env::var_os("SOCKROCKET_REFERENCE_XRAY")
                    .context("Xray executable required")?,
                ["run", "-config", "stdin:"],
            )
        } else {
            (
                json!({"log":{"level":"error"},"inbounds":[server],"outbounds":[{"type":"direct"}]}),
                std::env::var_os("SOCKROCKET_REFERENCE_SING_BOX")
                    .context("sing-box executable required")?,
                ["run", "-c", "stdin"],
            )
        };
        let mut proc = Server(
            Command::new(cmd)
                .args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .spawn()?,
            None,
        );
        serde_json::to_writer(proc.0.stdin.take().unwrap(), &conf)?;
        proc
    };
    if kind == "tuic" {
        tokio::time::sleep(Duration::from_millis(400)).await;
    } else {
        let listening_deadline = tokio::time::Instant::now() + Duration::from_secs(4);
        while tokio::net::TcpStream::connect((host, port)).await.is_err() {
            ensure!(
                tokio::time::Instant::now() < listening_deadline,
                "reference did not listen"
            );
            ensure!(proc.0.try_wait()?.is_none(), "server exited");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    let node = if kind == "tuic" && option == Some("uot_v1") {
        let yaml = json!({"proxies":[{"name":"local","type":"tuic","server":host,"port":port,"uuid":id,"password":pw,"udp":true,"udp-over-stream":true,"heartbeat-interval":250,"skip-cert-verify":true,"sni":"localhost","alpn":["h3"]}]});
        sockrocket_core::parse_clash_config(&yaml.to_string())?
            .pop()
            .context("node skipped")?
    } else {
        parse_singbox_outbound(&client.to_string())?.context("node skipped")?
    };
    if kind == "tuic" {
        let wanted = option.unwrap_or("native");
        let protocol = serde_json::to_value(&node.protocol)?;
        ensure!(
            protocol["heartbeat_ms"] == 250,
            "TUIC heartbeat changed during import"
        );
        ensure!(
            protocol["relay"].as_str().unwrap_or("native") == wanted,
            "TUIC relay changed during import"
        );
        let yaml =
            sockrocket_core::config::export::nodes_to_clash_yaml(std::slice::from_ref(&node))?;
        let parsed = sockrocket_core::parse_clash_config(&yaml)?
            .pop()
            .context("node skipped")?;
        ensure!(
            serde_json::to_value(parsed.protocol)? == protocol,
            "relay changed on Mihomo YAML roundtrip"
        );
        let uri = sockrocket_core::node_to_share_uri(&node);
        let restored = sockrocket_core::parse_tuic_uri(&uri)?;
        ensure!(
            serde_json::to_value(restored.protocol)? == protocol,
            "relay changed on URI roundtrip"
        );
    }
    if kind == "vmess"
        && let Some(expected) = option
    {
        ensure!(
            matches!(&node.protocol, sockrocket_core::ProxyProtocol::VMess { cipher, .. } if cipher == expected.split('+').next().unwrap()),
            "VMess security changed during import"
        );
    }
    let outbound = create_outbound_unwarmed(&node)?;
    let tcp = TcpListener::bind((host, 0)).await?;
    let port = tcp.local_addr()?.port();
    let echo = tokio::spawn(async move {
        let (mut peer, _) = tcp.accept().await?;
        let mut data = [0; 256];
        peer.read_exact(&mut data).await?;
        peer.write_all(&data).await
    });
    let mut stream = tokio::time::timeout(Duration::from_secs(5), outbound.connect(host, port))
        .await
        .context("reference TCP handshake timeout")??;
    stream.write_all(&[0x42; 256]).await?;
    stream.flush().await?;
    let mut reply = [0; 256];
    tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut reply))
        .await
        .context("reference TCP reply timeout")??;
    ensure!(reply == [0x42; 256], "TCP payload mismatch");
    echo.await??;
    let udp = UdpSocket::bind((host, 0)).await?;
    let tunnel = outbound
        .0
        .connect_datagram(host, udp.local_addr()?.port())
        .await?;
    let large = [0x74; 8192];
    for payload in [
        b"first".as_slice(),
        if kind == "tuic" {
            large.as_slice()
        } else {
            &large[..512]
        },
    ] {
        tunnel.send(payload).await?;
        let mut buf = [0; 8192];
        let (n, peer) = tokio::time::timeout(Duration::from_secs(5), udp.recv_from(&mut buf))
            .await
            .context("reference UDP send timeout")??;
        ensure!(&buf[..n] == payload, "UDP request mismatch");
        udp.send_to(&buf[..n], peer).await?;
        let n = tunnel.recv(&mut buf).await?;
        ensure!(&buf[..n] == payload, "UDP reply mismatch");
    }
    Ok(())
}

macro_rules! reference {
    ($name:ident, $kind:literal, $security:expr) => {
        #[tokio::test]
        #[ignore = "requires SOCKROCKET_REFERENCE_SING_BOX"]
        async fn $name() {
            tokio::time::timeout(Duration::from_secs(15), check($kind, $security, false))
                .await
                .expect("reference timeout")
                .unwrap();
        }
    };
}
reference!(reference_ss_tcp_udp, "shadowsocks", None);
reference!(reference_vmess_tcp_udp, "vmess", None);
reference!(
    reference_vmess_chacha_tcp_udp,
    "vmess",
    Some("chacha20-poly1305")
);
reference!(reference_vmess_none_tcp_udp, "vmess", Some("none"));
reference!(
    reference_vmess_padding_tcp_udp,
    "vmess",
    Some("aes-128-gcm+global_padding")
);
reference!(
    reference_vmess_auth_length_tcp_udp,
    "vmess",
    Some("chacha20-poly1305+authenticated_length")
);
reference!(
    reference_vmess_padded_auth_length_tcp_udp,
    "vmess",
    Some("aes-128-gcm+global_padding+authenticated_length")
);
reference!(reference_vmess_zero_tcp_udp, "vmess", Some("zero"));
reference!(
    reference_trojan_httpupgrade_tcp_udp,
    "trojan",
    Some("httpupgrade")
);
reference!(reference_trojan_grpc_tcp_udp, "trojan", Some("grpc"));
#[tokio::test]
#[ignore = "requires SOCKROCKET_REFERENCE_XRAY"]
async fn reference_xray_trojan_grpc_multi_tcp_udp() -> Result<()> {
    tokio::time::timeout(
        Duration::from_secs(15),
        check("trojan", Some("xray_grpc_multi"), false),
    )
    .await??;
    Ok(())
}
reference!(reference_trojan_tcp_udp, "trojan", None);
reference!(reference_tuic_tcp_udp, "tuic", None);
reference!(reference_vmess_aes_tcp_udp, "vmess", Some("aes-128-gcm"));

reference!(reference_tuic_quic_tcp_udp, "tuic", Some("quic"));
reference!(reference_tuic_uot_v1_tcp_udp, "tuic", Some("uot_v1"));
reference!(reference_tuic_uot_v2_tcp_udp, "tuic", Some("uot_v2"));

#[tokio::test]
#[ignore = "requires SOCKROCKET_REFERENCE_MIHOMO"]
async fn reference_tuic_mihomo_all_udp_modes() -> Result<()> {
    for mode in [None, Some("quic"), Some("uot_v1"), Some("uot_v2")] {
        tokio::time::timeout(Duration::from_secs(15), check("tuic", mode, true))
            .await
            .context("TUIC reference timeout")?
            .with_context(|| format!("Mihomo TUIC mode {mode:?}"))?;
    }
    Ok(())
}

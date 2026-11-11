use super::*;
use crate::config::model::{TlsConfig, TransportConfig, TransportType};
use crate::proxy::{connector::Outbound, vless::VlessOutbound};
use base64::Engine;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
struct Reference {
    child: std::process::Child,
    dir: std::path::PathBuf,
}
impl Drop for Reference {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
#[tokio::test]
#[ignore = "requires SOCKROCKET_REFERENCE_XRAY"]
async fn reference_xray_full_http_mode_matrix() {
    for alpn in ["http/1.1", "h2", "h3"] {
        for mode in ["packet-up", "stream-up", "stream-one", "auto"] {
            if alpn == "http/1.1" && mode == "stream-one" {
                continue;
            }
            eprintln!("reference case: {alpn} {mode}");
            reference_case(alpn, mode, serde_json::json!({}), false, false).await;
        }
    }
}
#[tokio::test]
#[ignore = "requires SOCKROCKET_REFERENCE_XRAY"]
async fn reference_xray_metadata_padding_payload_and_split_download() {
    for placement in ["header", "cookie", "query", "queryInHeader"] {
        for payload in ["body", "header", "cookie"] {
            let extra = serde_json::json!({"xPaddingObfsMode":true,"xPaddingPlacement":placement,"xPaddingMethod":"tokenish","xPaddingBytes":"100-180","sessionIDPlacement":"header","seqPlacement":"cookie","uplinkDataPlacement":payload,"headers":{"x-synthetic":"test"},"serverMaxHeaderBytes":65536,"scMaxEachPostBytes":16384,"scMinPostsIntervalMs":0,"xmux":{"maxConnections":1,"hMaxRequestTimes":8}});
            eprintln!("reference case: padding={placement} payload={payload}");
            reference_case("h2", "packet-up", extra, false, false).await;
        }
    }
    for place in ["path", "query", "header", "cookie"] {
        for method in ["GET", "OPTIONS"] {
            reference_case("h2","packet-up",serde_json::json!({"sessionIDPlacement":place,"seqPlacement":place,"uplinkHTTPMethod":method,"uplinkDataPlacement":"header","scMaxEachPostBytes":2048,"scMinPostsIntervalMs":0,"sessionIDTable":"Base62","sessionIDLength":"16-24"}),false,false).await;
        }
    }
    reference_case(
        "h2",
        "stream-up",
        serde_json::json!({"noGRPCHeader":true}),
        true,
        false,
    )
    .await;
}
#[tokio::test]
#[ignore = "requires SOCKROCKET_REFERENCE_XRAY"]
async fn reference_xray_reality_modes() {
    for mode in ["packet-up", "stream-up", "stream-one", "auto"] {
        eprintln!("reference REALITY case: {mode}");
        reference_case("h2", mode, serde_json::json!({}), false, true).await;
    }
    reference_case("h2", "auto", serde_json::json!({}), true, true).await;
}
async fn reference_case(
    alpn: &str,
    mode: &str,
    extra: serde_json::Value,
    split: bool,
    reality: bool,
) {
    let executable = std::env::var_os("SOCKROCKET_REFERENCE_XRAY").expect("reference executable");
    let dir = std::env::temp_dir().join(format!(
        "sockrocket-xhttp-reference-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir(&dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let pem = |label: &str, bytes: &[u8]| {
        let b = base64::engine::general_purpose::STANDARD.encode(bytes);
        format!(
            "-----BEGIN {label}-----\n{}\n-----END {label}-----\n",
            b.as_bytes()
                .chunks(64)
                .map(|line| std::str::from_utf8(line).unwrap())
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    std::fs::write(
        dir.join("cert.pem"),
        pem(
            "CERTIFICATE",
            include_bytes!("../testdata/localhost-cert.der"),
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("key.pem"),
        pem(
            "PRIVATE KEY",
            include_bytes!("../testdata/localhost-key.der"),
        ),
    )
    .unwrap();
    let mut keys = None;
    let mut target = None;
    let _camouflage = if reality {
        crate::proxy::quic_conn::ensure_quic_crypto_provider();
        let cert = rustls::pki_types::CertificateDer::from(
            include_bytes!("../testdata/localhost-cert.der").to_vec(),
        );
        let key =
            rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
                include_bytes!("../testdata/localhost-key.der").to_vec(),
            ));
        let mut tls =
            rustls::ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
                .with_no_client_auth()
                .with_single_cert(vec![cert], key)
                .unwrap();
        tls.alpn_protocols = vec![b"h2".to_vec()];
        tls.send_tls13_tickets = 0;
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        target = Some(listener.local_addr().unwrap().to_string());
        let output = std::process::Command::new(&executable)
            .arg("x25519")
            .output()
            .unwrap();
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        let get = |label: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(label).map(|v| v.trim().to_string()))
                .unwrap()
        };
        keys = Some((get("PrivateKey:"), get("Password (PublicKey):")));
        Some(Task(tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    result=listener.accept()=>{let Ok((stream,_))=result else {break};let acceptor=acceptor.clone();tasks.spawn(async move{if let Ok(_tls)=acceptor.accept(stream).await{tokio::time::sleep(Duration::from_secs(60)).await;}});},
                    Some(_)=tasks.join_next(),if !tasks.is_empty()=>{}
                }
            }
        })))
    } else {
        None
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let id = "00000000-0000-0000-0000-000000000001";
    let mut config = serde_json::json!({"log":{"loglevel":"none"},"inbounds":[{"listen":"127.0.0.1","port":port,"protocol":"vless","settings":{"clients":[{"id":id}],"decryption":"none"},"streamSettings":{"network":"xhttp","security":"tls","tlsSettings":{"alpn":["h2"],"certificates":[{"certificateFile":dir.join("cert.pem"),"keyFile":dir.join("key.pem")}]},"xhttpSettings":{"path":"/synthetic","host":"front.example.invalid","mode":"stream-one"}}}],"outbounds":[{"protocol":"freedom","settings":{"finalRules":[{"action":"allow","ip":["127.0.0.1"]}]}}]});
    let server_alpn = if alpn == "h3" {
        serde_json::json!(["h3"])
    } else {
        serde_json::json!(["h2", "http/1.1"])
    };
    config["inbounds"][0]["streamSettings"]["tlsSettings"]["alpn"] = server_alpn;
    config["inbounds"][0]["streamSettings"]["xhttpSettings"]["mode"] = mode.into();
    config["inbounds"][0]["streamSettings"]["xhttpSettings"]["extra"] = extra.clone();
    if let Some((private, _)) = &keys {
        config["inbounds"][0]["streamSettings"]["security"] = "reality".into();
        config["inbounds"][0]["streamSettings"]
            .as_object_mut()
            .unwrap()
            .remove("tlsSettings");
        config["inbounds"][0]["streamSettings"]["realitySettings"] = serde_json::json!({"dest":target,"serverNames":["localhost"],"privateKey":private,"shortIds":["0123456789abcdef"]});
    }
    std::fs::write(
        dir.join("config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let child = std::process::Command::new(executable)
        .args(["run", "-config"])
        .arg(dir.join("config.json"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut reference = Reference { child, dir };
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            assert!(
                reference.child.try_wait().unwrap().is_none(),
                "reference server exited during startup"
            );
            if alpn == "h3" {
                tokio::time::sleep(Duration::from_millis(100)).await;
                break;
            }
            if tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_ok()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let mut transport = TransportConfig {
        grpc: None,
        transport_type: TransportType::Xhttp,
        tls: Some(TlsConfig {
            sni: Some("localhost".into()),
            skip_cert_verify: true,
            alpn: Some(vec![alpn.into()]),
            fingerprint: if alpn == "h3" {
                Some("chrome".into())
            } else {
                None
            },
        }),
        ws: None,
        reality: None,
        xhttp: Some(XhttpConfig {
            options: std::collections::BTreeMap::from([("extra".into(), extra.clone())]),
            path: Some("/synthetic".into()),
            host: Some("front.example.invalid".into()),
            mode: mode.into(),
        }),
    };
    if split {
        transport.xhttp.as_mut().unwrap().options.get_mut("extra").unwrap().as_object_mut().unwrap().insert("downloadSettings".into(),serde_json::json!({"address":"127.0.0.1","port":port,"network":"xhttp","security":"tls","tlsSettings":{"serverName":"localhost","allowInsecure":true,"alpn":["http/1.1"]},"xhttpSettings":{"path":"/synthetic","host":"front.example.invalid","mode":"auto"}}));
    }
    if split {
        if let Some((_, public)) = &keys {
            let download = transport
                .xhttp
                .as_mut()
                .unwrap()
                .options
                .get_mut("extra")
                .unwrap()
                .as_object_mut()
                .unwrap()
                .get_mut("downloadSettings")
                .unwrap();
            download["security"] = "reality".into();
            download.as_object_mut().unwrap().remove("tlsSettings");
            download["realitySettings"] = serde_json::json!({"serverName":"localhost","publicKey":public,"shortId":"0123456789abcdef","fingerprint":"chrome"});
        }
    }
    if let Some((_, public)) = keys {
        transport.tls = None;
        transport.reality = Some(crate::config::model::RealityConfig {
            public_key: public,
            short_id: "0123456789abcdef".into(),
            sni: Some("localhost".into()),
            client_fingerprint: Some("chrome".into()),
            skip_cert_verify: false,
        });
    }
    let outbound = VlessOutbound::new_unwarmed("127.0.0.1", port, id, Some(&transport)).unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut peer, _) = listener.accept().await.unwrap();
            let mut bytes = vec![0; 128 * 1024];
            peer.read_exact(&mut bytes).await.unwrap();
            peer.write_all(&bytes).await.unwrap();
            peer.shutdown().await.unwrap();
        });
        let mut stream = outbound.connect("127.0.0.1", address.port()).await.unwrap();
        eprintln!("reference stage: TCP connected");
        let payload = vec![37; 128 * 1024];
        for bytes in payload.chunks(997) {
            stream.write_all(bytes).await.unwrap();
        }
        eprintln!("reference stage: TCP upload queued");
        // Xray stream-one treats request EOF as session termination.
        // Read the normal response before ending the upload body.
        let mut response = vec![0; payload.len()];
        stream.read_exact(&mut response).await.unwrap();
        assert!(response == payload, "reference TCP response differs");
        eprintln!("reference stage: TCP echo received");
        drop(stream);
        server.await.unwrap();
        let udp = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = udp.local_addr().unwrap();
        let echo = tokio::spawn(async move {
            let mut buf = [0; 2048];
            let (n, from) = udp.recv_from(&mut buf).await.unwrap();
            udp.send_to(&buf[..n], from).await.unwrap();
        });
        let datagram = outbound
            .connect_datagram("127.0.0.1", address.port())
            .await
            .unwrap();
        eprintln!("reference stage: UDP connected");
        datagram.send(b"synthetic UDP payload").await.unwrap();
        eprintln!("reference stage: UDP sent");
        let mut reply = [0; 128];
        let n = datagram.recv(&mut reply).await.unwrap();
        assert_eq!(&reply[..n], b"synthetic UDP payload");
        echo.await.unwrap();
    })
    .await
    .unwrap();
}

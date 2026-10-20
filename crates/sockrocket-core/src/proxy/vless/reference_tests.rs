//! Independent interoperability; sing-box is a test-only reference server.
use super::*;
use crate::config::model::{TlsConfig, TransportType};
use base64::Engine;
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
#[ignore = "requires SOCKROCKET_REFERENCE_SING_BOX, an independently verified test-only executable"]
async fn reference_vision_plain_tls_direct_udp_and_shutdown() {
    let executable =
        std::env::var_os("SOCKROCKET_REFERENCE_SING_BOX").expect("reference server executable");
    let dir = std::env::temp_dir().join(format!(
        "sockrocket-vision-reference-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir(&dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let cert = include_bytes!("../testdata/localhost-cert.der");
    let key = include_bytes!("../testdata/localhost-key.der");
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
    std::fs::write(dir.join("cert.pem"), pem("CERTIFICATE", cert)).unwrap();
    std::fs::write(dir.join("key.pem"), pem("PRIVATE KEY", key)).unwrap();
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let id = "00000000-0000-0000-0000-000000000001";
    let config = serde_json::json!({"log":{"disabled":true},"inbounds":[{"type":"vless","listen":"127.0.0.1","listen_port":port,"users":[{"uuid":id,"flow":"xtls-rprx-vision"}],"tls":{"enabled":true,"certificate_path":dir.join("cert.pem"),"key_path":dir.join("key.pem")}}],"outbounds":[{"type":"direct"}]});
    std::fs::write(
        dir.join("config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let child = std::process::Command::new(executable)
        .args(["run", "-c"])
        .arg(dir.join("config.json"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let _reference = Reference { child, dir };
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_ok()
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let transport = TransportConfig {
        transport_type: TransportType::Tls,
        tls: Some(TlsConfig {
            sni: Some("localhost".into()),
            skip_cert_verify: true,
            alpn: None,
            fingerprint: None,
        }),
        ws: None,
        reality: None,
    };
    let outbound = VlessOutbound::new_vision("127.0.0.1", port, id, Some(&transport)).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        // Plain TCP, fragmented writes and half-close/EOF.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut peer, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            peer.read_to_end(&mut bytes).await.unwrap();
            peer.write_all(&bytes).await.unwrap();
            peer.shutdown().await.unwrap();
        });
        let mut stream = outbound.connect("127.0.0.1", address.port()).await.unwrap();
        let payload = vec![7u8; 32768];
        for chunk in payload.chunks(127) {
            stream.write_all(chunk).await.unwrap();
        }
        stream.shutdown().await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        assert_eq!(response, payload);
        server.await.unwrap();
        // Inner TLS 1.3 activates Vision direct copy in each direction.
        let _ = rustls::crypto::ring::default_provider().install_default();
        for tls13 in [true, false] {
            let cert = rustls::pki_types::CertificateDer::from(cert.to_vec());
            let key = rustls::pki_types::PrivateKeyDer::Pkcs8(
                rustls::pki_types::PrivatePkcs8KeyDer::from(key.to_vec()),
            );
            let versions = if tls13 {
                vec![&rustls::version::TLS13]
            } else {
                vec![&rustls::version::TLS12]
            };
            let mut config = rustls::ServerConfig::builder_with_protocol_versions(&versions)
                .with_no_client_auth()
                .with_single_cert(vec![cert.clone()], key)
                .unwrap();
            config.send_tls13_tickets = 0;
            let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (peer, _) = listener.accept().await.unwrap();
                let mut tls = acceptor.accept(peer).await.unwrap();
                for _ in 0..256 {
                    let mut bytes = [0; 8192];
                    tls.read_exact(&mut bytes).await.unwrap();
                    tls.write_all(&bytes).await.unwrap();
                    tls.flush().await.unwrap();
                }
                tls.shutdown().await.unwrap();
            });
            let mut roots = rustls::RootCertStore::empty();
            roots.add(cert).unwrap();
            let config = rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth();
            let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
            let stream = outbound.connect("127.0.0.1", address.port()).await.unwrap();
            let mut tls = connector
                .connect(
                    rustls::pki_types::ServerName::try_from("localhost").unwrap(),
                    stream,
                )
                .await
                .unwrap();
            for index in 0..256 {
                let bytes = [index as u8; 8192];
                tls.write_all(&bytes).await.unwrap();
                tls.flush().await.unwrap();
                let mut response = [0; 8192];
                tls.read_exact(&mut response).await.unwrap();
                assert_eq!(response, bytes);
            }
            tls.shutdown().await.unwrap();
            server.await.unwrap();
        }
        // Native UDP (non-443) retains message boundaries, including DNS.
        let echo = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = echo.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let mut bytes = [0; 2048];
            let (n, peer) = echo.recv_from(&mut bytes).await.unwrap();
            echo.send_to(&bytes[..n], peer).await.unwrap();
        });
        let datagram = outbound.connect_datagram("127.0.0.1", port).await.unwrap();
        datagram.send(b"synthetic DNS payload").await.unwrap();
        let mut response = [0; 128];
        let n = datagram.recv(&mut response).await.unwrap();
        assert_eq!(&response[..n], b"synthetic DNS payload");
        server.await.unwrap();
        assert!(outbound.connect_datagram("127.0.0.1", 443).await.is_err());
    })
    .await
    .expect("Vision reference test must finish within its deadline");
}

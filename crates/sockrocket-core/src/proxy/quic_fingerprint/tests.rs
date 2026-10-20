use super::*;
use sha2::{Digest, Sha256};

fn config(skip: bool, pin: Option<[u8; 32]>) -> quinn::ClientConfig {
    let mut config = client_config(skip, &["h3".into()], pin).unwrap();
    let mut transport = quinn::TransportConfig::default();
    transport_config(&mut transport);
    config.transport_config(Arc::new(transport));
    config
}
fn client(profile: bool, config: quinn::ClientConfig) -> quinn::Endpoint {
    let ec = if profile {
        endpoint_config()
    } else {
        quinn::EndpointConfig::default()
    };
    let mut ep =
        super::super::quic_conn::client_endpoint_with_config("127.0.0.1:0".parse().unwrap(), ec)
            .unwrap();
    ep.set_default_client_config(config);
    ep
}
async fn initial(profile: bool) -> Vec<u8> {
    super::super::quic_conn::ensure_quic_crypto_provider();
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let config = if profile {
        config(true, None)
    } else {
        super::super::quic_conn::build_quic_client_config(true, &["h3".into()]).unwrap()
    };
    let endpoint = client(profile, config);
    let connecting = endpoint
        .connect(socket.local_addr().unwrap(), "example.invalid")
        .unwrap();
    let mut packet = vec![0; 65536];
    let (n, _) = tokio::time::timeout(Duration::from_secs(3), socket.recv_from(&mut packet))
        .await
        .unwrap()
        .unwrap();
    packet.truncate(n);
    drop(connecting);
    endpoint.close(0u32.into(), b"capture complete");
    packet
}

// Independent rustls 0.23 QUIC key implementation decodes the actual UDP
// packet produced by craftls 0.22 + Quinn. Parsing our own profile object
// alone would miss crypto adapters that drop/overwrite the ClientHello.
fn decode_initial(mut packet: Vec<u8>) -> (usize, usize, Vec<u8>) {
    assert_eq!(packet[0] & 0xf0, 0xc0);
    assert_eq!(&packet[1..5], &1u32.to_be_bytes());
    let dcid_len = packet[5] as usize;
    let dcid = packet[6..6 + dcid_len].to_vec();
    let scid_len = packet[6 + dcid_len] as usize;
    let mut offset = 7 + dcid_len + scid_len;
    let mut rest = &packet[offset..];
    let token = read_varint(&mut rest).unwrap() as usize;
    assert_eq!(token, 0);
    let len = read_varint(&mut rest).unwrap() as usize;
    offset = packet.len() - rest.len();
    packet.truncate(offset + len);
    let suite = rustls::crypto::ring::cipher_suite::TLS13_AES_128_GCM_SHA256
        .tls13()
        .unwrap();
    let keys = rustls::quic::Keys::initial(
        rustls::quic::Version::V1,
        suite,
        suite.quic.unwrap(),
        &dcid,
        rustls::Side::Server,
    );
    let sample = packet[offset + 4..offset + 20].to_vec();
    let (header, _) = packet.split_at_mut(offset + 4);
    let (first, bytes) = header.split_at_mut(1);
    keys.remote
        .header
        .decrypt_in_place(&sample, &mut first[0], &mut bytes[offset - 1..offset + 3])
        .unwrap();
    let pn_len = ((packet[0] & 3) + 1) as usize;
    let mut pn = 0u64;
    for b in &packet[offset..offset + pn_len] {
        pn = pn * 256 + u64::from(*b);
    }
    let (header, ciphertext) = packet.split_at_mut(offset + pn_len);
    let mut frames = keys
        .remote
        .packet
        .decrypt_in_place(pn, header, ciphertext)
        .unwrap();
    let mut hello = Vec::new();
    while !frames.is_empty() {
        match read_varint(&mut frames).unwrap() {
            0 | 1 => {}
            6 => {
                let pos = read_varint(&mut frames).unwrap() as usize;
                let n = read_varint(&mut frames).unwrap() as usize;
                if hello.len() < pos + n {
                    hello.resize(pos + n, 0);
                }
                hello[pos..pos + n].copy_from_slice(&frames[..n]);
                frames = &frames[n..];
            }
            other => panic!("unexpected Initial frame: {other}"),
        }
    }
    (dcid_len, scid_len, hello)
}
fn hello_extensions(hello: &[u8]) -> Vec<(u16, Vec<u8>)> {
    assert_eq!(hello[0], 1);
    assert_eq!(&hello[4..6], &[3, 3]);
    assert_eq!(hello[38], 0, "QUIC has an empty legacy session ID");
    let n = u16::from_be_bytes([hello[39], hello[40]]) as usize;
    assert_eq!(&hello[41..41 + n], &[0x13, 1, 0x13, 2, 0x13, 3]);
    let mut offset = 41 + n;
    assert_eq!(&hello[offset..offset + 2], &[1, 0]);
    offset += 2;
    let len = u16::from_be_bytes([hello[offset], hello[offset + 1]]) as usize;
    offset += 2;
    assert_eq!(offset + len, hello.len());
    let mut extensions = Vec::new();
    while offset < hello.len() {
        let id = u16::from_be_bytes([hello[offset], hello[offset + 1]]);
        let len = u16::from_be_bytes([hello[offset + 2], hello[offset + 3]]) as usize;
        offset += 4;
        extensions.push((id, hello[offset..offset + len].to_vec()));
        offset += len;
    }
    extensions
}

#[tokio::test]
async fn chrome_profile_changes_actual_encrypted_initial_and_native_stays_native() {
    let (dcid, scid, hello) = decode_initial(initial(true).await);
    assert_eq!((dcid, scid), (8, 0));
    let extensions = hello_extensions(&hello);
    let get = |id| &extensions.iter().find(|(k, _)| *k == id).unwrap().1;
    let mut ids: Vec<_> = extensions.iter().map(|(id, _)| *id).collect();
    ids.sort_unstable();
    assert_eq!(ids, [0, 10, 13, 16, 27, 43, 45, 51, 57]);
    assert_eq!(get(43), &[2, 3, 4]);
    assert_eq!(get(16), &[0, 3, 2, b'h', b'3']);
    assert_eq!(get(27), &[2, 0, 2]); // actual Brotli decoder is configured
    assert_eq!(get(10), &[0, 6, 0, 29, 0, 23, 0, 24]);
    assert_eq!(&get(51)[..6], &[0, 36, 0, 29, 0, 32]);
    let parameters = parse_parameters(get(57)).unwrap();
    for (id, value) in [
        (1, 30000),
        (3, 1472),
        (4, 15728640),
        (5, 6291456),
        (6, 6291456),
        (7, 6291456),
        (8, 100),
        (9, 103),
        (0x20, 65536),
    ] {
        assert_eq!(
            read_varint(&mut &parameters.iter().find(|(key, _)| *key == id).unwrap().1[..])
                .unwrap(),
            value
        );
    }
    assert!(parameters.iter().any(|(id, _)| *id % 31 == 27));
    assert!(
        parameters
            .iter()
            .any(|(id, v)| *id == 0x3128 && v == b"RVCM")
    );
    assert!(
        parameters
            .iter()
            .all(|(id, _)| ![10, 11, 14, 0x2ab2, 0xff04de1b].contains(id))
    );
    let (native_dcid, native_scid, _) = decode_initial(initial(false).await);
    assert_eq!(native_dcid, 20);
    assert!(native_scid > 0);

    let first_order: Vec<_> = extensions.iter().map(|(id, _)| *id).collect();
    let mut changed = false;
    for _ in 0..3 {
        let (_, _, hello) = decode_initial(initial(true).await);
        let order: Vec<_> = hello_extensions(&hello).iter().map(|(id, _)| *id).collect();
        changed |= order != first_order;
    }
    assert!(changed, "extension order must be randomized per connection");
}

fn server() -> quinn::Endpoint {
    use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
    super::super::quic_conn::ensure_quic_crypto_provider();
    let cert = CertificateDer::from(include_bytes!("../testdata/localhost-cert.der").to_vec());
    let key = PrivatePkcs8KeyDer::from(include_bytes!("../testdata/localhost-key.der").to_vec());
    let mut tls = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key.into())
        .unwrap();
    tls.alpn_protocols = vec![b"h3".to_vec()];
    let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls).unwrap();
    quinn::Endpoint::server(
        quinn::ServerConfig::with_crypto(Arc::new(crypto)),
        "127.0.0.1:0".parse().unwrap(),
    )
    .unwrap()
}
#[tokio::test]
async fn chrome_retry_streams_datagrams_exporter_and_repeated_key_updates_interoperate() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let server = server();
        let address = server.local_addr().unwrap();
        let accept = tokio::spawn(async move {
            let incoming = server.accept().await.unwrap();
            incoming.retry().unwrap();
            let conn = server.accept().await.unwrap().await.unwrap();
            (server, conn)
        });
        let endpoint = client(true, config(true, None));
        let client = endpoint
            .connect(address, "localhost")
            .unwrap()
            .await
            .unwrap();
        let (_server_endpoint, server) = accept.await.unwrap();
        assert_eq!(
            client
                .handshake_data()
                .unwrap()
                .downcast::<quinn::crypto::rustls::HandshakeData>()
                .unwrap()
                .protocol
                .as_deref(),
            Some(b"h3".as_slice())
        );
        assert!(
            !client
                .peer_identity()
                .unwrap()
                .downcast::<Vec<rustls_pki_types::CertificateDer<'static>>>()
                .unwrap()
                .is_empty()
        );
        let mut a = [0; 32];
        let mut b = [0; 32];
        client
            .export_keying_material(&mut a, b"synthetic-test", b"context")
            .unwrap();
        server
            .export_keying_material(&mut b, b"synthetic-test", b"context")
            .unwrap();
        assert_eq!(a, b);
        for i in 0..4u8 {
            if i > 0 {
                client.force_key_update();
                server.force_key_update();
            }
            let echo = tokio::spawn({
                let server = server.clone();
                async move {
                    let (mut tx, mut rx) = server.accept_bi().await.unwrap();
                    let body = rx.read_to_end(131072).await.unwrap();
                    tx.write_all(&body).await.unwrap();
                    tx.finish().unwrap();
                    let body = server.read_datagram().await.unwrap();
                    server.send_datagram(body).unwrap();
                }
            });
            let (mut tx, mut rx) = client.open_bi().await.unwrap();
            let body = vec![i; 65536];
            tx.write_all(&body).await.unwrap();
            tx.finish().unwrap();
            assert_eq!(rx.read_to_end(131072).await.unwrap(), body);
            client
                .send_datagram(bytes::Bytes::from(vec![i; 1200]))
                .unwrap();
            assert_eq!(
                client.read_datagram().await.unwrap().as_ref(),
                vec![i; 1200]
            );
            echo.await.unwrap();
            // Wait for ACKs before the next key update.
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        client.close(0u32.into(), b"done");
    })
    .await
    .expect("QUIC interoperability deadline");
}
#[tokio::test]
async fn chrome_certificate_verification_and_leaf_pins_are_enforced() {
    let pin: [u8; 32] = Sha256::digest(include_bytes!("../testdata/localhost-cert.der")).into();
    for (skip, selected_pin, expected) in [
        (false, None, false),
        (false, Some(pin), true),
        (true, Some([0; 32]), false),
        (true, None, true),
    ] {
        let server = server();
        let address = server.local_addr().unwrap();
        let accept = tokio::spawn(async move {
            let incoming = server.accept().await.unwrap();
            let conn = incoming.await;
            (server, conn)
        });
        let endpoint = client(true, config(skip, selected_pin));
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            endpoint.connect(address, "localhost").unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(result.is_ok(), expected);
        endpoint.close(0u32.into(), b"done");
        drop(accept.await.unwrap());
    }
}
#[test]
fn profile_rejects_incompatible_alpn_and_malformed_parameters() {
    assert!(client_config(false, &["hysteria".into()], None).is_err());
    assert!(chrome_parameters(&[]).is_err());
    assert!(parse_parameters(&[1, 3, 0]).is_err());
    assert!(parse_parameters(&[1, 0, 1, 0]).is_err());
    for value in [
        0,
        63,
        64,
        16383,
        16384,
        (1 << 30) - 1,
        1 << 30,
        (1 << 62) - 1,
    ] {
        assert_eq!(read_varint(&mut &varint(value)[..]).unwrap(), value);
    }
}

/// Independent Go server interoperability, using only synthetic loopback
/// credentials. Run with SOCKROCKET_REFERENCE_SING_BOX=/path/to/sing-box.
#[tokio::test]
#[ignore = "requires an independent sing-box executable"]
async fn chrome_hysteria2_reference_tcp_and_udp() {
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
    let executable =
        std::env::var_os("SOCKROCKET_REFERENCE_SING_BOX").expect("reference executable");
    let dir = std::env::temp_dir().join(format!(
        "sockrocket-quic-reference-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir(&dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    for (file, label, data) in [
        (
            "cert.pem",
            "CERTIFICATE",
            include_bytes!("../testdata/localhost-cert.der").as_slice(),
        ),
        (
            "key.pem",
            "PRIVATE KEY",
            include_bytes!("../testdata/localhost-key.der").as_slice(),
        ),
    ] {
        let b64 = base64::engine::general_purpose::STANDARD.encode(data);
        std::fs::write(
            dir.join(file),
            format!("-----BEGIN {label}-----\n{b64}\n-----END {label}-----\n"),
        )
        .unwrap();
    }
    let port = std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let server = serde_json::json!({"log":{"level":"error"},"inbounds":[{"type":"hysteria2","listen":"127.0.0.1","listen_port":port,"users":[{"name":"synthetic","password":"integration-only"}],"tls":{"enabled":true,"alpn":["h3"],"certificate_path":dir.join("cert.pem"),"key_path":dir.join("key.pem")}}],"outbounds":[{"type":"direct"}]});
    let path = dir.join("server.json");
    std::fs::write(&path, serde_json::to_vec(&server).unwrap()).unwrap();
    let child = std::process::Command::new(executable)
        .arg("run")
        .arg("-c")
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut reference = Reference { child, dir };
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        reference.child.try_wait().unwrap().is_none(),
        "reference server failed to start"
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tcp_address = listener.local_addr().unwrap();
    let echo = tokio::spawn(async move {
        for _ in 0..8 {
            let (mut conn, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut buffer = [0; 8192];
                loop {
                    let n = conn.read(&mut buffer).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    conn.write_all(&buffer[..n]).await.unwrap();
                }
            });
        }
    });
    let udp = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let udp_address = udp.local_addr().unwrap();
    let udp_echo = tokio::spawn(async move {
        let mut packet = [0; 8192];
        for _ in 0..8 {
            let (n, from) = udp.recv_from(&mut packet).await.unwrap();
            udp.send_to(&packet[..n], from).await.unwrap();
        }
    });
    tokio::time::timeout(Duration::from_secs(15), async {
        let node = crate::config::v2ray::parse_hysteria2_uri(&format!(
            "hy2://integration-only@127.0.0.1:{port}?insecure=1&sni=localhost&alpn=h3&fp=chrome"
        ))
        .unwrap();
        let outbound = crate::proxy::factory::create_outbound(&node).unwrap();
        let datagram = outbound
            .0
            .connect_datagram("127.0.0.1", udp_address.port())
            .await
            .unwrap();
        for i in 0..8u8 {
            let mut stream = outbound
                .connect("127.0.0.1", tcp_address.port())
                .await
                .unwrap();
            let body = vec![i; 32768];
            stream.write_all(&body).await.unwrap();
            stream.flush().await.unwrap();
            let mut received = vec![0; body.len()];
            stream.read_exact(&mut received).await.unwrap();
            assert_eq!(received, body);
            let packet = vec![i; 4000];
            datagram.send(&packet).await.unwrap();
            let mut reply = [0; 8192];
            let n = datagram.recv(&mut reply).await.unwrap();
            assert_eq!(&reply[..n], packet);
        }
        echo.await.unwrap();
        udp_echo.await.unwrap();
    })
    .await
    .expect("Hysteria2 independent-server deadline");
}

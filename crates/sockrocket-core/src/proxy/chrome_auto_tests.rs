use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn extensions(config: Arc<ClientConfig>) -> HashMap<u16, Vec<u8>> {
    let mut client = craft_tls::rustls::ClientConnection::new(
        config,
        ServerName::try_from("localhost").unwrap(),
    )
    .unwrap();
    let mut wire = Vec::new();
    client.write_tls(&mut wire).unwrap();
    assert_eq!(wire[0], 22);
    extensions_from_wire(&wire)
}
fn extensions_from_wire(wire: &[u8]) -> HashMap<u16, Vec<u8>> {
    let mut record = 0;
    while wire[record] != 22 {
        record += 5 + u16::from_be_bytes([wire[record + 3], wire[record + 4]]) as usize;
    }
    let hello = &wire[record + 9..];
    let mut at = 34;
    at += 1 + hello[at] as usize;
    at += 2 + u16::from_be_bytes([hello[at], hello[at + 1]]) as usize;
    at += 1 + hello[at] as usize;
    let end = at + 2 + u16::from_be_bytes([hello[at], hello[at + 1]]) as usize;
    at += 2;
    let mut extensions = HashMap::new();
    while at < end {
        let kind = u16::from_be_bytes([hello[at], hello[at + 1]]);
        let len = u16::from_be_bytes([hello[at + 2], hello[at + 3]]) as usize;
        at += 4;
        assert!(
            extensions
                .insert(kind, hello[at..at + len].to_vec())
                .is_none()
        );
        at += len;
    }
    extensions
}
fn shares(extensions: &HashMap<u16, Vec<u8>>) -> Vec<(u16, Vec<u8>)> {
    let bytes = &extensions[&51];
    let mut at = 2;
    let mut shares = vec![];
    assert_eq!(
        u16::from_be_bytes([bytes[0], bytes[1]]) as usize,
        bytes.len() - 2
    );
    while at < bytes.len() {
        let group = u16::from_be_bytes([bytes[at], bytes[at + 1]]);
        let len = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
        at += 4;
        shares.push((group, bytes[at..at + len].to_vec()));
        at += len;
    }
    shares
}
#[tokio::test]
async fn trojan_reality_uri_selects_the_requested_client_hello() {
    use base64::Engine;
    let key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([7; 32]);
    for fp in ["chrome", "qq", "edge", "ios", "safari", "firefox"] {
        let uri = format!("trojan://pw@localhost:443?security=reality&pbk={key}&sid=abcd&fp={fp}");
        let node = crate::config::v2ray::parse_trojan_uri(&uri).unwrap();
        let factory =
            TlsConnFactory::from_transport("localhost", 443, node.transport.as_ref(), true);
        let connector = factory.reality_connector.unwrap();
        let (client, mut peer) = tokio::io::duplex(8192);
        let hello = async {
            let mut header = [0; 5];
            peer.read_exact(&mut header).await.unwrap();
            let mut wire = header.to_vec();
            wire.resize(5 + u16::from_be_bytes([header[3], header[4]]) as usize, 0);
            peer.read_exact(&mut wire[5..]).await.unwrap();
            extensions_from_wire(&wire)
        };
        let ext = tokio::select! {
            ext = hello => ext,
            result = connector.connect(ServerName::try_from("localhost").unwrap(), client) => panic!("premature TLS result {result:?}"),
        };
        assert_eq!(ext.contains_key(&0xfe0d), fp == "chrome", "{fp}");
        assert_eq!(ext.contains_key(&17513), fp == "qq", "{fp}");
        assert_eq!(ext.contains_key(&28), fp == "firefox", "{fp}");
        assert_eq!(
            ext.contains_key(&27),
            !matches!(fp, "ios" | "firefox"),
            "{fp}"
        );
        assert_eq!(
            shares(&ext).iter().any(|(g, _)| *g == 0x11ec),
            fp == "chrome",
            "{fp}"
        );
    }
}

#[test]
fn chrome_auto_wire_matches_reference_profile_and_has_fresh_real_keys() {
    let config = build_tls_config(None, None);
    let a = extensions(config.clone());
    let b = extensions(config);
    assert!(a.contains_key(&17613) && !a.contains_key(&17513) && !a.contains_key(&21));
    let groups: Vec<_> = a[&10][2..]
        .chunks_exact(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect();
    assert_eq!(&groups[1..], &[0x11ec, 29, 23, 24]);
    let keys = shares(&a);
    assert_eq!(
        keys.iter()
            .map(|(g, k)| (*g, k.len()))
            .skip(1)
            .collect::<Vec<_>>(),
        [(0x11ec, 1216), (29, 32)]
    );
    assert_ne!(keys[1].1, shares(&b)[1].1);
    let ech = &a[&0xfe0d];
    assert_eq!(&ech[..5], &[0, 0, 1, 0, 1]);
    assert_eq!(&ech[6..8], &[0, 32]);
    let payload = u16::from_be_bytes([ech[40], ech[41]]) as usize;
    assert!([144, 176, 208, 240].contains(&payload));
    assert_eq!(ech.len(), 42 + payload);
    assert_ne!(a[&0xfe0d], b[&0xfe0d]);
}
#[test]
fn chrome_auto_keeps_both_shares_after_a_classic_peer_hint() {
    let config = build_tls_config(None, None);
    for group in [
        craft_tls::rustls::NamedGroup::X25519,
        craft_tls::rustls::NamedGroup::secp256r1,
    ] {
        config
            .resumption
            .store
            .set_kx_hint(ServerName::try_from("localhost").unwrap(), group);
        let hello = extensions(config.clone());
        assert_eq!(
            shares(&hello)
                .iter()
                .skip(1)
                .map(|(g, k)| (*g, k.len()))
                .collect::<Vec<_>>(),
            [(0x11ec, 1216), (29, 32)]
        );
    }
}

#[test]
fn explicit_legacy_chrome_and_custom_alpn_are_preserved() {
    let legacy = TlsConfig {
        sni: None,
        skip_cert_verify: false,
        alpn: None,
        fingerprint: Some("chrome112".into()),
    };
    let old = extensions(build_tls_config(Some(&legacy), None));
    assert!(!old.contains_key(&0xfe0d));
    assert_eq!(
        shares(&old)
            .iter()
            .skip(1)
            .map(|(g, k)| (*g, k.len()))
            .collect::<Vec<_>>(),
        [(29, 32)]
    );
    let config = TlsConfig {
        alpn: Some(vec!["http/1.1".into()]),
        fingerprint: Some("chrome".into()),
        ..legacy
    };
    let custom = extensions(build_tls_config(Some(&config), None));
    assert_eq!(custom[&16], b"\0\x09\x08http/1.1");
}

async fn handshake(
    version: &'static rustls::SupportedProtocolVersion,
    group: &'static dyn rustls::crypto::SupportedKxGroup,
    expected: rustls::NamedGroup,
) {
    let mut provider = rustls::crypto::aws_lc_rs::default_provider();
    provider.kx_groups = vec![group];
    let cert = CertificateDer::from(include_bytes!("testdata/localhost-cert.der").to_vec());
    let key = rustls_pki_types::PrivatePkcs8KeyDer::from(
        include_bytes!("testdata/localhost-key.der").to_vec(),
    );
    let server = rustls::ServerConfig::builder_with_provider(Arc::new(provider))
        .with_protocol_versions(&[version])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key.into())
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut stream = tokio_rustls::TlsAcceptor::from(Arc::new(server))
            .accept(socket)
            .await
            .unwrap();
        assert_eq!(
            stream
                .get_ref()
                .1
                .negotiated_key_exchange_group()
                .unwrap()
                .name(),
            expected
        );
        stream.write_all(b"verified").await.unwrap();
        stream.flush().await.unwrap();
    });
    // A leaf pin gives the test an independent trust anchor without disabling
    // certificate checks for a loopback fixture.
    use sha2::Digest;
    let pin: [u8; 32] = sha2::Sha256::digest(include_bytes!("testdata/localhost-cert.der")).into();
    let config = TlsConfig {
        sni: Some("localhost".into()),
        skip_cert_verify: false,
        alpn: None,
        fingerprint: Some("chrome".into()),
    };
    let connector = build_tls_connector_pinned(Some(&config), Some(pin));
    let raw = tokio::net::TcpStream::connect(addr).await.unwrap();
    let mut stream = connect_tls_with(raw, "localhost", &connector)
        .await
        .unwrap();
    let mut response = [0; 8];
    stream.read_exact(&mut response).await.unwrap();
    assert_eq!(&response, b"verified");
    task.await.unwrap();
}
#[tokio::test]
async fn chrome_auto_completes_real_hybrid_classic_and_retry_handshakes() {
    tokio::time::timeout(Duration::from_secs(5), async {
        handshake(
            &rustls::version::TLS13,
            rustls::crypto::aws_lc_rs::kx_group::X25519MLKEM768,
            rustls::NamedGroup::X25519MLKEM768,
        )
        .await;
        handshake(
            &rustls::version::TLS13,
            rustls::crypto::aws_lc_rs::kx_group::X25519,
            rustls::NamedGroup::X25519,
        )
        .await;
        handshake(
            &rustls::version::TLS13,
            rustls::crypto::aws_lc_rs::kx_group::SECP256R1,
            rustls::NamedGroup::secp256r1,
        )
        .await;
        handshake(
            &rustls::version::TLS12,
            rustls::crypto::aws_lc_rs::kx_group::X25519,
            rustls::NamedGroup::X25519,
        )
        .await;
    })
    .await
    .unwrap();
}

#[test]
fn retry_keeps_ech_grease_and_replaces_only_the_requested_key_share() {
    let config = build_tls_config(None, None);
    let mut client = craft_tls::rustls::ClientConnection::new(
        config,
        ServerName::try_from("localhost").unwrap(),
    )
    .unwrap();
    let mut first = Vec::new();
    client.write_tls(&mut first).unwrap();
    let mut provider = rustls::crypto::aws_lc_rs::default_provider();
    provider.kx_groups = vec![rustls::crypto::aws_lc_rs::kx_group::SECP256R1];
    let cert = CertificateDer::from(include_bytes!("testdata/localhost-cert.der").to_vec());
    let key = rustls_pki_types::PrivatePkcs8KeyDer::from(
        include_bytes!("testdata/localhost-key.der").to_vec(),
    );
    let server_config = rustls::ServerConfig::builder_with_provider(Arc::new(provider))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key.into())
        .unwrap();
    let mut server = rustls::ServerConnection::new(Arc::new(server_config)).unwrap();
    server.read_tls(&mut std::io::Cursor::new(&first)).unwrap();
    server.process_new_packets().unwrap();
    let mut retry = Vec::new();
    server.write_tls(&mut retry).unwrap();
    client.read_tls(&mut std::io::Cursor::new(retry)).unwrap();
    client.process_new_packets().unwrap();
    let mut second = Vec::new();
    client.write_tls(&mut second).unwrap();
    let a = extensions_from_wire(&first);
    let b = extensions_from_wire(&second);
    assert_eq!(a[&0xfe0d], b[&0xfe0d]);
    assert_eq!(a[&10], b[&10]);
    assert_eq!(
        shares(&b)
            .iter()
            .map(|(g, k)| (*g, k.len()))
            .collect::<Vec<_>>(),
        [(23, 65)]
    );
}

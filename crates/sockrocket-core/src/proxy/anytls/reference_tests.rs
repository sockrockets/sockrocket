//! Optional independent interoperability test. The reference executable is a
//! test-only official sing-box download, never a product runtime dependency.
use super::*;
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
#[ignore = "requires SOCKROCKET_REFERENCE_SING_BOX pointing to a checksum-verified official test server"]
async fn reference_anytls_tcp_udp_reuse_and_shutdown() {
    use base64::Engine;
    let executable =
        std::env::var_os("SOCKROCKET_REFERENCE_SING_BOX").expect("reference test executable");
    let dir = std::env::temp_dir().join(format!(
        "sockrocket-anytls-reference-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir(&dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let pem = |label: &str, data: &[u8]| {
        let encoded = base64::engine::general_purpose::STANDARD.encode(data);
        format!(
            "-----BEGIN {label}-----\n{}\n-----END {label}-----\n",
            encoded
                .as_bytes()
                .chunks(64)
                .map(|b| std::str::from_utf8(b).unwrap())
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    let cert = dir.join("cert.pem");
    let key = dir.join("key.pem");
    std::fs::write(
        &cert,
        pem(
            "CERTIFICATE",
            include_bytes!("../testdata/localhost-cert.der"),
        ),
    )
    .unwrap();
    std::fs::write(
        &key,
        pem(
            "PRIVATE KEY",
            include_bytes!("../testdata/localhost-key.der"),
        ),
    )
    .unwrap();
    let port = {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        probe.local_addr().unwrap().port()
    };
    let config = serde_json::json!({"log":{"level":"error"},"inbounds":[{"type":"anytls","listen":"127.0.0.1","listen_port":port,"users":[{"name":"synthetic","password":"integration-only"}],"tls":{"enabled":true,"certificate_path":cert,"key_path":key}}],"outbounds":[{"type":"direct"}]});
    let path = dir.join("server.json");
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    let child = std::process::Command::new(executable)
        .arg("run")
        .arg("-c")
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let _reference = Reference { child, dir };
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
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
    let tls = TlsConfig {
        sni: Some("localhost".into()),
        skip_cert_verify: true,
        alpn: None,
        fingerprint: None,
    };
    let outbound =
        AnyTlsOutbound::new("127.0.0.1", port, "integration-only", Some(&tls), true).unwrap();
    let echo = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = echo.local_addr().unwrap();
    let task = tokio::spawn(async move {
        for _ in 0..3 {
            let (mut stream, _) = echo.accept().await.unwrap();
            let mut bytes = [0; 8192];
            stream.read_exact(&mut bytes).await.unwrap();
            stream.write_all(&bytes).await.unwrap();
            stream.shutdown().await.unwrap();
        }
    });
    for _ in 0..3 {
        let mut stream = outbound.connect("127.0.0.1", address.port()).await.unwrap();
        let bytes = vec![0x69; 8192];
        stream.write_all(&bytes).await.unwrap();
        stream.flush().await.unwrap();
        let mut reply = vec![0; 8192];
        tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut reply))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reply, bytes);
        drop(stream);
        tokio::time::timeout(Duration::from_secs(2), async {
            while outbound.client.idle.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    task.await.unwrap();
    let echo = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = echo.local_addr().unwrap();
    let task = tokio::spawn(async move {
        for _ in 0..3 {
            let mut bytes = [0; 4096];
            let (n, peer) = echo.recv_from(&mut bytes).await.unwrap();
            echo.send_to(&bytes[..n], peer).await.unwrap();
        }
    });
    let udp = outbound
        .connect_datagram("127.0.0.1", address.port())
        .await
        .unwrap();
    for data in [
        b"first".as_slice(),
        b"second packet".as_slice(),
        b"".as_slice(),
    ] {
        udp.send(data).await.unwrap();
        let mut reply = [0; 4096];
        let n = tokio::time::timeout(Duration::from_secs(5), udp.recv(&mut reply))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&reply[..n], data);
    }
    drop(udp);
    task.await.unwrap();
    let weak = Arc::downgrade(&outbound.client);
    drop(outbound);
    tokio::time::timeout(Duration::from_secs(2), async {
        while weak.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

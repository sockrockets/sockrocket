//! TEMPORARY HY2 hello matrix. Targets from `.local/live.env`.
//! Run: cargo test -p sockrocket-core --test hy2_probe -- --ignored --nocapture

mod common;

use std::time::Instant;

use sockrocket_core::config::model::TlsConfig;
use sockrocket_core::proxy::connector::Outbound;
use sockrocket_core::proxy::hysteria2::Hysteria2Outbound;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn probe(
    server: &str,
    port: u16,
    password: &str,
    alpn: Option<&str>,
    sni: &str,
    label: &str,
) -> String {
    let started = Instant::now();
    let result = async {
        let tls = TlsConfig {
            sni: Some(sni.to_string()),
            skip_cert_verify: true,
            alpn: alpn.map(|a| vec![a.to_string()]),
            fingerprint: None,
        };
        let out = Hysteria2Outbound::new(server, port, password, Some(&tls))?;
        let t0 = Instant::now();
        let mut stream = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            out.connect("www.gstatic.com", 80),
        )
        .await
        .map_err(|_| anyhow::anyhow!("connect timeout 20s"))??;
        let connect_ms = t0.elapsed().as_millis() as u32;
        stream
            .write_all(b"HEAD /generate_204 HTTP/1.1\r\nHost: www.gstatic.com\r\nConnection: close\r\n\r\n")
            .await?;
        let mut buf = vec![0u8; 256];
        let n = tokio::time::timeout(std::time::Duration::from_secs(8), stream.read(&mut buf))
            .await
            .map_err(|_| anyhow::anyhow!("first byte timeout 8s"))??;
        let head = String::from_utf8_lossy(&buf[..n])
            .lines()
            .next()
            .unwrap_or("")
            .to_string();
        Ok::<_, anyhow::Error>(format!("handshake+auth+connect={connect_ms}ms resp=\"{head}\""))
    }
    .await;
    match result {
        Ok(d) => format!(
            "[OK]   {label}: {d}, total={}ms",
            started.elapsed().as_millis()
        ),
        Err(e) => format!(
            "[FAIL] {label}: {e:#?}, total={}ms",
            started.elapsed().as_millis()
        ),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "needs LIVE_SERVER / LIVE_PASSWORD in .local/live.env"]
async fn hy2_hello_matrix() {
    let env = common::load_env_file("live.env");
    let server = common::require(&env, "LIVE_SERVER");
    let port: u16 = common::require(&env, "LIVE_PORT")
        .parse()
        .expect("LIVE_PORT");
    let password = common::require(&env, "LIVE_PASSWORD");
    let sni = common::optional(&env, "LIVE_SNI").unwrap_or_else(|| server.clone());

    let cases = [(None, "DEFAULT fallback"), (Some("h3"), "alpn=h3")];
    let mut hs = vec![];
    for (alpn, label) in cases {
        let server = server.clone();
        let password = password.clone();
        let sni = sni.clone();
        hs.push(tokio::spawn(async move {
            probe(&server, port, &password, alpn, &sni, label).await
        }));
    }
    for h in hs {
        println!("{}", h.await.unwrap());
    }
}

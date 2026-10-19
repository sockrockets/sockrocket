//! Live end-to-end ShadowTLS probe. Credentials from `.local/live.env`.
//! Run: cargo test -p sockrocket-core --test shadow_tls_live -- --ignored --nocapture

mod common;

use std::time::Duration;

use sockrocket_core::proxy::connector::Outbound;
use sockrocket_core::proxy::shadow_tls::ShadowTlsOutbound;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
#[ignore = "needs LIVE_* credentials in .local/live.env"]
async fn shadow_tls_live_connectivity() {
    let env = common::load_env_file("live.env");
    let server = common::require(&env, "LIVE_SERVER");
    let port: u16 = common::require(&env, "LIVE_PORT")
        .parse()
        .expect("LIVE_PORT");
    let ss_password = common::require(&env, "LIVE_SS_PASSWORD");
    let st_password = common::require(&env, "LIVE_ST_PASSWORD");
    let st_host = common::require(&env, "LIVE_ST_HOST");
    let cipher = common::optional(&env, "LIVE_SS_CIPHER")
        .unwrap_or_else(|| "2022-blake3-aes-256-gcm".into());

    let outbound = ShadowTlsOutbound::new(
        &server,
        port,
        &cipher,
        &ss_password,
        &st_host,
        &st_password,
        true,
    )
    .expect("outbound config");

    let mut stream = tokio::time::timeout(
        Duration::from_secs(20),
        outbound.connect("www.gstatic.com", 80),
    )
    .await
    .expect("connect timed out")
    .expect("shadow-tls + SS handshake failed");
    println!("connect OK");

    stream
        .write_all(
            b"GET /generate_204 HTTP/1.1\r\nHost: www.gstatic.com\r\nConnection: close\r\n\r\n",
        )
        .await
        .expect("write request");

    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let deadline = tokio::time::sleep(Duration::from_secs(20));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => panic!("no HTTP response within 20s"),
            n = stream.read(&mut chunk) => {
                let n = n.expect("read failed");
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                let head = String::from_utf8_lossy(&buf[..buf.len().min(64)]);
                if head.contains("HTTP/1.") {
                    println!("OK: received {} bytes", buf.len());
                    return;
                }
            }
        }
    }
    panic!(
        "connection closed without an HTTP response: {:?}",
        String::from_utf8_lossy(&buf)
    );
}

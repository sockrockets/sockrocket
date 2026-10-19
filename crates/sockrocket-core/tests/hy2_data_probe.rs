//! TEMPORARY HY2 data probe. Credentials from `.local/live.env`.
//! Run: cargo test -p sockrocket-core --test hy2_data_probe -- --ignored --nocapture

mod common;

use std::time::Instant;

use sockrocket_core::config::model::TlsConfig;
use sockrocket_core::proxy::connector::Outbound;
use sockrocket_core::proxy::hysteria2::Hysteria2Outbound;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs LIVE_SERVER / LIVE_PASSWORD in .local/live.env"]
async fn hy2_data_probe() {
    let env = common::load_env_file("live.env");
    let server = common::require(&env, "LIVE_SERVER");
    let port: u16 = common::require(&env, "LIVE_PORT")
        .parse()
        .expect("LIVE_PORT");
    let password = common::require(&env, "LIVE_PASSWORD");
    let sni = common::optional(&env, "LIVE_SNI").unwrap_or_else(|| server.clone());

    let tls = TlsConfig {
        sni: Some(sni),
        skip_cert_verify: true,
        alpn: Some(vec!["h3".to_string()]),
        fingerprint: None,
    };
    let out = Hysteria2Outbound::new(&server, port, &password, Some(&tls)).unwrap();

    let started = Instant::now();
    let mut stream = out.connect("www.gstatic.com", 80).await.expect("connect");
    eprintln!("connected in {}ms", started.elapsed().as_millis());

    stream
        .write_all(
            b"GET /generate_204 HTTP/1.1\r\nHost: www.gstatic.com\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();

    let mut buf = vec![0u8; 4096];
    let mut total = 0usize;
    for i in 0..5 {
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            stream.read(&mut buf[total..]),
        )
        .await
        {
            Ok(Ok(0)) => {
                eprintln!("read {i}: EOF, total={total}");
                break;
            }
            Ok(Ok(n)) => {
                total += n;
                eprintln!("read {i}: +{n} bytes, total={total}");
                if total > 64 {
                    break;
                }
            }
            Ok(Err(e)) => {
                eprintln!("read {i}: error {e}");
                break;
            }
            Err(_) => {
                eprintln!("read {i}: timeout, total={total}");
                break;
            }
        }
    }
    eprintln!("read total={total} bytes");
}

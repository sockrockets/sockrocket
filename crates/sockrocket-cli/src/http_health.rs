//! Router watchdog probe, independent of firmware curl/nc applets and monitor settings.
use anyhow::{Result, bail};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub async fn check(port: u16) -> Result<()> {
    for host in ["www.gstatic.com", "cp.cloudflare.com", "www.google.com"] {
        if tokio::time::timeout(std::time::Duration::from_secs(5), exchange(port, host))
            .await
            .is_ok_and(|result| result.is_ok())
        {
            return Ok(());
        }
    }
    bail!("HTTP proxy probe failed for all three targets")
}

async fn exchange(port: u16, host: &str) -> Result<()> {
    let mut stream = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    stream.write_all(format!(
        "GET http://{host}/generate_204 HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    ).as_bytes()).await?;
    // Bound the status line; never wait for EOF or allocate a response body.
    let mut line = Vec::with_capacity(128);
    while line.len() < 1024 {
        let byte = stream.read_u8().await?;
        line.push(byte);
        if byte == b'\n' {
            let text = std::str::from_utf8(&line)?;
            let mut fields = text.split_whitespace();
            if matches!(fields.next(), Some("HTTP/1.0" | "HTTP/1.1"))
                && fields.next() == Some("204")
            {
                return Ok(());
            }
            bail!("HTTP proxy did not return 204");
        }
    }
    bail!("HTTP proxy status line too long")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    async fn server(replies: Vec<&'static [u8]>) -> (u16, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            for reply in replies {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = vec![];
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(socket.read_u8().await.unwrap());
                }
                assert!(request.starts_with(b"GET http://"));
                let _ = socket.write_all(reply).await;
            }
        });
        (port, task)
    }

    #[tokio::test]
    async fn blocked_primary_uses_fallback() {
        let (port, task) = server(vec![
            b"HTTP/1.1 502 Bad Gateway\r\n",
            b"HTTP/1.0 204 No Content\r\n",
        ])
        .await;
        check(port).await.unwrap();
        task.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_errors_redirects_and_malformed_responses() {
        let (port, task) = server(vec![
            b"HTTP/1.1 502 Bad Gateway\r\n",
            b"HTTP/1.1 302 Redirect\r\n",
            b"garbage 204 ok\r\n",
        ])
        .await;
        assert!(check(port).await.is_err());
        task.await.unwrap();
    }

    #[tokio::test]
    async fn silent_listener_has_bounded_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let start = tokio::time::Instant::now();
        assert!(check(port).await.is_err());
        assert!(start.elapsed() >= std::time::Duration::from_secs(15));
        assert!(start.elapsed() < std::time::Duration::from_secs(20));
    }
}

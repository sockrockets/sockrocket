//! Bounded diagnostic requests through the running daemon, without firmware tools.
use anyhow::{Result, bail, ensure};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

struct Connection {
    stream: TcpStream,
    deadline: Instant,
}
impl Connection {
    fn remaining(&self) -> Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| anyhow::anyhow!("Diagnostic deadline exceeded"))
    }
    fn send(&mut self, mut bytes: &[u8]) -> Result<()> {
        while !bytes.is_empty() {
            self.stream.set_write_timeout(Some(self.remaining()?))?;
            let n = self.stream.write(bytes)?;
            ensure!(n > 0, "Proxy closed while writing");
            bytes = &bytes[n..];
        }
        Ok(())
    }
    fn read(&mut self, bytes: &mut [u8]) -> Result<usize> {
        self.stream.set_read_timeout(Some(self.remaining()?))?;
        Ok(self.stream.read(bytes)?)
    }
    fn exact(&mut self, mut bytes: &mut [u8]) -> Result<()> {
        while !bytes.is_empty() {
            let n = self.read(bytes)?;
            ensure!(n > 0, "Proxy closed unexpectedly");
            bytes = &mut bytes[n..];
        }
        Ok(())
    }
}

pub fn get(port: u16, host: &str, path: &str, timeout: Duration) -> Result<(u16, Vec<u8>)> {
    ensure!(
        !host.is_empty() && host.len() <= 255 && !host.bytes().any(|b| b <= 32 || b >= 127),
        "Invalid host"
    );
    ensure!(
        path.starts_with('/') && !path.bytes().any(|b| b <= 32 || b >= 127),
        "Invalid path"
    );
    let deadline = Instant::now() + timeout;
    let stream =
        TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), timeout)?;
    let mut c = Connection { stream, deadline };
    c.send(&[5, 1, 0])?;
    let mut greeting = [0; 2];
    c.exact(&mut greeting)?;
    ensure!(
        greeting == [5, 0],
        "SOCKS authentication negotiation failed"
    );
    let mut request = vec![5, 1, 0, 3, host.len() as u8];
    request.extend_from_slice(host.as_bytes());
    request.extend_from_slice(&80u16.to_be_bytes());
    c.send(&request)?;
    let mut reply = [0; 4];
    c.exact(&mut reply)?;
    ensure!(
        reply[0] == 5 && reply[1] == 0 && reply[2] == 0,
        "SOCKS connection rejected"
    );
    let size = match reply[3] {
        1 => 4,
        4 => 16,
        3 => {
            let mut n = [0];
            c.exact(&mut n)?;
            n[0] as usize
        }
        _ => bail!("Invalid SOCKS address type"),
    };
    c.exact(&mut vec![0; size + 2])?;
    c.send(format!("GET {path} HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes())?;
    // Headers and body are separately bounded, including chunked responses.
    let mut headers = Vec::new();
    while !headers.ends_with(b"\r\n\r\n") {
        ensure!(headers.len() < 16384, "HTTP headers too large");
        let mut b = [0];
        c.exact(&mut b)?;
        headers.push(b[0]);
    }
    let text = std::str::from_utf8(&headers)?;
    let mut lines = text.split("\r\n");
    let mut status = lines.next().unwrap_or("").split_whitespace();
    ensure!(
        matches!(status.next(), Some("HTTP/1.0" | "HTTP/1.1")),
        "Invalid HTTP response"
    );
    let code: u16 = status.next().unwrap_or("").parse()?;
    if code == 204 {
        return Ok((code, Vec::new()));
    }
    let mut length = None;
    let mut chunked = false;
    for line in lines {
        if let Some((key, value)) = line.split_once(':') {
            if key.eq_ignore_ascii_case("content-length") {
                ensure!(length.is_none(), "Duplicate Content-Length");
                let n: usize = value.trim().parse()?;
                ensure!(n <= 65536, "HTTP body too large");
                length = Some(n);
            }
            if key.eq_ignore_ascii_case("transfer-encoding") {
                ensure!(
                    value.trim().eq_ignore_ascii_case("chunked"),
                    "Unsupported Transfer-Encoding"
                );
                chunked = true;
            }
        }
    }
    let mut body = Vec::new();
    if chunked {
        ensure!(length.is_none(), "Ambiguous HTTP framing");
        loop {
            let mut line = Vec::new();
            while !line.ends_with(b"\r\n") {
                ensure!(line.len() < 128, "Invalid chunk header");
                let mut b = [0];
                c.exact(&mut b)?;
                line.push(b[0]);
            }
            let n = usize::from_str_radix(
                std::str::from_utf8(&line)?
                    .trim()
                    .split(';')
                    .next()
                    .unwrap_or(""),
                16,
            )?;
            if n == 0 {
                break;
            }
            ensure!(n <= 65536 - body.len(), "HTTP body too large");
            let start = body.len();
            body.resize(start + n, 0);
            c.exact(&mut body[start..])?;
            let mut end = [0; 2];
            c.exact(&mut end)?;
            ensure!(end == *b"\r\n", "Invalid chunk ending");
        }
    } else if let Some(n) = length {
        body.resize(n, 0);
        c.exact(&mut body)?;
    } else {
        let mut buf = [0; 4096];
        loop {
            let n = c.read(&mut buf)?;
            if n == 0 {
                break;
            }
            ensure!(n <= 65536 - body.len(), "HTTP body too large");
            body.extend_from_slice(&buf[..n]);
        }
    }
    Ok((code, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    async fn request(reply: Vec<u8>, atyp: u8) -> Result<(u16, Vec<u8>)> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut hello = [0; 3];
            s.read_exact(&mut hello).await.unwrap();
            assert_eq!(hello, [5, 1, 0]);
            s.write_all(&[5, 0]).await.unwrap();
            let mut connect = [0; 5];
            s.read_exact(&mut connect).await.unwrap();
            assert_eq!(&connect[..4], &[5, 1, 0, 3]);
            let mut target = vec![0; connect[4] as usize + 2];
            s.read_exact(&mut target).await.unwrap();
            assert_eq!(&target[..target.len() - 2], b"example.test");
            assert_eq!(&target[target.len() - 2..], &80u16.to_be_bytes());
            let mut bound = vec![5, 0, 0, atyp];
            match atyp {
                1 => bound.extend_from_slice(&[0; 4]),
                4 => bound.extend_from_slice(&[0; 16]),
                _ => bound.extend_from_slice(&[1, b'x']),
            }
            bound.extend_from_slice(&[0, 0]);
            // Fragmenting the handshake must not be interpreted as EOF.
            for b in bound {
                s.write_all(&[b]).await.unwrap();
            }
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                head.push(s.read_u8().await.unwrap());
            }
            assert!(head.starts_with(b"GET /test HTTP/1.0\r\n"));
            let _ = s.write_all(&reply).await;
        });
        let result = tokio::task::spawn_blocking(move || {
            get(port, "example.test", "/test", Duration::from_secs(2))
        })
        .await
        .unwrap();
        server.await.unwrap();
        result
    }
    #[tokio::test]
    async fn parses_all_socks_address_types_and_http_framing() {
        for atyp in [1, 3, 4] {
            let r = request(
                b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}".to_vec(),
                atyp,
            )
            .await
            .unwrap();
            assert_eq!(r, (200, b"{}".to_vec()));
        }
        assert_eq!(
            request(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2;ext=x\r\n{}\r\n0\r\n\r\n"
                    .to_vec(),
                1
            )
            .await
            .unwrap()
            .1,
            b"{}"
        );
        assert_eq!(
            request(b"HTTP/1.0 200 OK\r\n\r\nbody".to_vec(), 1)
                .await
                .unwrap()
                .1,
            b"body"
        );
        assert_eq!(
            request(b"HTTP/1.1 204 No Content\r\n\r\n".to_vec(), 1)
                .await
                .unwrap()
                .0,
            204
        );
    }
    #[tokio::test]
    async fn rejects_truncation_oversize_and_ambiguous_framing() {
        for response in [
            b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nshort".as_slice(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 65537\r\n\r\n".as_slice(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nTransfer-Encoding: chunked\r\n\r\n"
                .as_slice(),
            b"garbage 200 OK\r\n\r\n".as_slice(),
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nffffffffffffffff\r\n".as_slice(),
        ] {
            assert!(request(response.to_vec(), 1).await.is_err());
        }
    }
    #[tokio::test]
    async fn bounds_total_time_when_proxy_is_silent() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let (_s, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });
        let start = Instant::now();
        assert!(
            tokio::task::spawn_blocking(move || get(
                port,
                "example.test",
                "/test",
                Duration::from_millis(100)
            ))
            .await
            .unwrap()
            .is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        task.abort();
    }
}

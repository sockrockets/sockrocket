use anyhow::{Result, bail};
use socket2::SockRef;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

use super::ProxyStats;
use super::connector::{SharedOutbound, connect_upstream};
use super::relay::relay_bidirectional;

/// HTTP proxy server supporting CONNECT tunnels and plain HTTP forwarding.
pub struct HttpProxyServer {
    listener: TcpListener,
    outbound: SharedOutbound,
    stats: ProxyStats,
}

impl HttpProxyServer {
    pub async fn bind(
        addr: &str,
        port: u16,
        outbound: SharedOutbound,
        stats: ProxyStats,
    ) -> Result<Self> {
        let listener = TcpListener::bind(format!("{}:{}", addr, port)).await?;
        tracing::info!("HTTP proxy server listening on {}:{}", addr, port);
        Ok(Self {
            listener,
            outbound,
            stats,
        })
    }

    pub fn local_port(&self) -> u16 {
        self.listener.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    pub async fn run(self, mut shutdown: watch::Receiver<bool>) -> Result<()> {
        loop {
            tokio::select! {
                biased;
                _ = shutdown.changed() => {
                    tracing::info!("HTTP proxy server shutting down");
                    break;
                }
                result = self.listener.accept() => {
                    let (stream, peer) = result?;
                    stream.set_nodelay(true).ok();
                    let sock = SockRef::from(&stream);
                    sock.set_send_buffer_size(256 * 1024).ok();
                    sock.set_recv_buffer_size(256 * 1024).ok();
                    let outbound = self.outbound.clone();
                    let stats = self.stats.clone();
                    tokio::spawn(async move {
                        stats.add_connection();
                        let result = handle_http(stream, outbound, stats.clone()).await;
                        stats.remove_connection();
                        if let Err(e) = result {
                            tracing::debug!("HTTP proxy connection from {} error: {}", peer, e);
                        }
                    });
                }
            }
        }
        Ok(())
    }
}

const MAX_HEADER_SIZE: usize = 8192;

/// Read HTTP request header. Returns (header_bytes, remaining_bytes_after_header).
async fn read_http_header<R: AsyncRead + Unpin>(stream: &mut R) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 1024];
    // Scan cursor: after a failed scan, the next scan resumes here instead
    // of rescanning the whole buffer (O(n²) over repeated reads). Backed up
    // 3 bytes per round so a \r\n\r split across a read boundary is found.
    let mut cursor = 0;
    loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            bail!("Connection closed before HTTP header complete");
        }
        buf.extend_from_slice(&tmp[..n]);

        // Look for \r\n\r\n
        if let Some(pos) = find_header_end(&buf[cursor..]) {
            let end = cursor + pos + 4;
            let header = buf[..end].to_vec();
            let remaining = buf[end..].to_vec();
            return Ok((header, remaining));
        }
        cursor = buf.len().saturating_sub(3);
        if buf.len() > MAX_HEADER_SIZE {
            bail!("HTTP header exceeds {} bytes", MAX_HEADER_SIZE);
        }
    }
}

/// Find \r\n\r\n in buffer, returns position of first \r.
fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// ASCII case-insensitive `starts_with` for header names — avoids the
/// per-line `to_lowercase()` allocation when rewriting request headers.
fn starts_with_ignore_ascii_case(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

/// Parse "host:port" from CONNECT target or Host header.
fn parse_host_port(target: &str, default_port: u16) -> Result<(String, u16)> {
    // Handle [ipv6]:port
    if let Some(bracket_end) = target.find(']') {
        let host = &target[1..bracket_end];
        let port = if target.len() > bracket_end + 2 && target.as_bytes()[bracket_end + 1] == b':' {
            target[bracket_end + 2..].parse::<u16>()?
        } else {
            default_port
        };
        return Ok((host.to_string(), port));
    }

    // Handle host:port
    if let Some(colon) = target.rfind(':')
        && let Ok(port) = target[colon + 1..].parse::<u16>()
    {
        return Ok((target[..colon].to_string(), port));
    }
    Ok((target.to_string(), default_port))
}

/// Extract the request method, target, and HTTP version from the first line.
fn parse_request_line(header: &[u8]) -> Result<(String, String, String)> {
    let first_line_end = header
        .windows(2)
        .position(|w| w == b"\r\n")
        .unwrap_or(header.len());
    let line = std::str::from_utf8(&header[..first_line_end])?;
    let parts: Vec<&str> = line.splitn(3, ' ').collect();
    if parts.len() < 3 {
        bail!("Invalid HTTP request line: {}", line);
    }
    Ok((
        parts[0].to_string(),
        parts[1].to_string(),
        parts[2].to_string(),
    ))
}

/// Handle a single HTTP proxy connection.
async fn handle_http(
    mut stream: TcpStream,
    outbound: SharedOutbound,
    stats: ProxyStats,
) -> Result<()> {
    let (header, remaining) = read_http_header(&mut stream).await?;
    let (method, target, _version) = parse_request_line(&header)?;

    if method.eq_ignore_ascii_case("CONNECT") {
        // HTTPS tunnel
        let (host, port) = parse_host_port(&target, 443)?;
        tracing::debug!("HTTP CONNECT to {}:{}", host, port);
        let conn_id = stats.record_connection(format!("{}:{}", host, port), "HTTP");

        match connect_upstream(&outbound, &host, port).await {
            Ok(mut remote) => {
                stream
                    .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                    .await?;
                // Forward any data buffered after the header
                if !remaining.is_empty() {
                    remote.write_all(&remaining).await?;
                }
                let (up, down) = relay_bidirectional(&mut stream, &mut remote).await?;
                stats.close_connection(conn_id, up, down);
                tracing::debug!(
                    "HTTP CONNECT {}:{} done: up={} down={}",
                    host,
                    port,
                    up,
                    down
                );
            }
            Err(e) => {
                tracing::warn!("HTTP outbound connect failed for {}:{}: {}", host, port, e);
                stream
                    .write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n")
                    .await?;
                return Err(e);
            }
        }
    } else {
        // Plain HTTP proxy (GET, POST, etc.)
        handle_plain_http(&mut stream, &header, &remaining, &method, &target, outbound).await?;
    }

    Ok(())
}

/// Handle plain (non-CONNECT) HTTP proxy request.
async fn handle_plain_http(
    client: &mut TcpStream,
    header: &[u8],
    remaining: &[u8],
    method: &str,
    target: &str,
    outbound: SharedOutbound,
) -> Result<()> {
    // Parse absolute URL to get host and relative path
    let url = if target.starts_with("http://") {
        target.to_string()
    } else {
        // Not an absolute URL, can't proxy
        client
            .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\nOnly absolute URLs supported\r\n")
            .await?;
        bail!("Non-absolute URL in plain HTTP proxy: {}", target);
    };

    let parsed = url::Url::parse(&url)?;
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("No host in URL"))?
        .to_string();
    let port = parsed.port().unwrap_or(80);
    let path = if parsed.query().is_some() {
        format!("{}?{}", parsed.path(), parsed.query().unwrap())
    } else {
        parsed.path().to_string()
    };

    tracing::debug!("HTTP {} {}:{}{}", method, host, port, path);

    let mut remote = connect_upstream(&outbound, &host, port).await?;

    // Rewrite request: absolute URI → relative path
    let header_str = std::str::from_utf8(header)?;
    let first_line_end = header_str.find("\r\n").unwrap_or(header_str.len());
    let rewritten_first_line = format!("{} {} HTTP/1.1", method, path);
    let rest_of_header = &header_str[first_line_end..];

    // Remove Proxy-Connection header, normalize Connection for the upstream request.
    let mut new_header = rewritten_first_line;
    let mut saw_connection = false;
    for line in rest_of_header.split("\r\n") {
        if line.is_empty() {
            continue;
        }
        if starts_with_ignore_ascii_case(line, "proxy-connection:") {
            continue;
        }
        if starts_with_ignore_ascii_case(line, "connection:") {
            saw_connection = true;
        }
        new_header.push_str("\r\n");
        new_header.push_str(line);
    }
    if !saw_connection {
        new_header.push_str("\r\nConnection: close");
    }
    new_header.push_str("\r\n\r\n");

    remote.write_all(new_header.as_bytes()).await?;

    // Forward any request body that was buffered
    if !remaining.is_empty() {
        remote.write_all(remaining).await?;
    }

    // Relay bidirectionally (handles response + any further request body)
    let _ = relay_bidirectional(client, &mut remote).await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::connector::DirectOutbound;
    use std::sync::Arc;

    #[test]
    fn test_parse_host_port() {
        let (h, p) = parse_host_port("example.com:443", 443).unwrap();
        assert_eq!(h, "example.com");
        assert_eq!(p, 443);

        let (h, p) = parse_host_port("example.com", 80).unwrap();
        assert_eq!(h, "example.com");
        assert_eq!(p, 80);

        let (h, p) = parse_host_port("[::1]:8080", 443).unwrap();
        assert_eq!(h, "::1");
        assert_eq!(p, 8080);
    }

    #[test]
    fn test_parse_request_line() {
        let line = b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com\r\n\r\n";
        let (m, t, v) = parse_request_line(line).unwrap();
        assert_eq!(m, "CONNECT");
        assert_eq!(t, "example.com:443");
        assert_eq!(v, "HTTP/1.1");
    }

    #[test]
    fn test_find_header_end() {
        assert_eq!(find_header_end(b"GET / HTTP/1.1\r\n\r\n"), Some(14));
        assert_eq!(find_header_end(b"GET / HTTP/1.1\r\n"), None);
    }

    #[test]
    fn test_starts_with_ignore_ascii_case() {
        assert!(starts_with_ignore_ascii_case(
            "Proxy-Connection: keep-alive",
            "proxy-connection:"
        ));
        assert!(starts_with_ignore_ascii_case(
            "CONNECTION: close",
            "connection:"
        ));
        assert!(!starts_with_ignore_ascii_case("connect", "connection:"));
        assert!(!starts_with_ignore_ascii_case(
            "X-Connection: close",
            "proxy-connection:"
        ));
    }

    /// Header delivered in fragments (with the \r\n\r\n terminator split
    /// across reads) must be found via the scan cursor, and bytes after the
    /// header must be returned as `remaining`.
    #[tokio::test]
    async fn test_read_http_header_split_across_reads() {
        let (mut tx, mut rx) = tokio::io::duplex(4096);
        let request: &[u8] = b"GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\nBODY";
        let writer = tokio::spawn(async move {
            for chunk in request.chunks(7) {
                tx.write_all(chunk).await.unwrap();
                tokio::task::yield_now().await;
            }
        });

        let (header, mut remaining) = read_http_header(&mut rx).await.unwrap();
        writer.await.unwrap(); // all chunks sent; tx dropped
        // `remaining` holds only the bytes buffered past the header when the
        // terminator was found; the rest is still in the stream.
        rx.read_to_end(&mut remaining).await.unwrap();

        assert_eq!(
            header,
            b"GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n"
        );
        assert_eq!(remaining, b"BODY");
    }

    #[tokio::test]
    async fn test_http_connect_tunnel() {
        // Echo server
        let echo_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let echo_port = echo_listener.local_addr().unwrap().port();
        let _echo = tokio::spawn(async move {
            if let Ok((mut s, _)) = echo_listener.accept().await {
                let mut buf = vec![0u8; 4096];
                loop {
                    match s.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if s.write_all(&buf[..n]).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        });

        let stats = ProxyStats::default();
        let outbound = SharedOutbound(Arc::new(DirectOutbound));
        let server = HttpProxyServer::bind("127.0.0.1", 0, outbound, stats)
            .await
            .unwrap();
        let http_port = server.local_port();

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let _server_handle = tokio::spawn(async move { server.run(shutdown_rx).await });

        // HTTP CONNECT request
        let mut client = TcpStream::connect(format!("127.0.0.1:{}", http_port))
            .await
            .unwrap();

        let connect_req = format!(
            "CONNECT 127.0.0.1:{} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
            echo_port, echo_port
        );
        client.write_all(connect_req.as_bytes()).await.unwrap();

        // Read 200 response
        let mut resp_buf = vec![0u8; 256];
        let n = client.read(&mut resp_buf).await.unwrap();
        let resp = std::str::from_utf8(&resp_buf[..n]).unwrap();
        assert!(resp.contains("200"));

        // Tunnel data
        client.write_all(b"tunnel test").await.unwrap();
        let mut buf = [0u8; 11];
        client.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"tunnel test");

        drop(client);
        let _ = shutdown_tx.send(true);
    }
}

//! Minimal LAN subscription share server.
//!
//! Serves the current node list as subscription content over plain HTTP so
//! other devices on the same network (phone, laptop, ...) can subscribe
//! directly — the data never leaves the local machine except to the LAN
//! peer that presents the right token.
//!
//! Hand-rolled HTTP/1.1 on top of `tokio::net::TcpListener`; deliberately no
//! hyper/axum. Only `GET /sub?token=<TOKEN>&format=clash|v2ray` is served:
//! wrong token → 403, any other path → 404, any other method → 405.

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

use crate::config::export::SubscriptionFormat;
use crate::config::model::Node;

/// Default listen port for the LAN share server (kept away from common ports).
pub const DEFAULT_SHARE_PORT: u16 = 19870;

/// Cap on the HTTP request head we are willing to read.
const MAX_HEAD_BYTES: usize = 16 * 1024;

/// How long a connection may take to send its request before we give up.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Hot-updatable view of what the server should serve. The GUI rewrites this
/// whenever the node list or the selected format changes; each request reads
/// a snapshot under a short-lived lock.
#[derive(Debug, Clone)]
pub struct ShareState {
    pub nodes: Vec<Node>,
    pub format: SubscriptionFormat,
}

pub type SharedShareState = Arc<RwLock<ShareState>>;

pub fn new_shared_state(nodes: Vec<Node>, format: SubscriptionFormat) -> SharedShareState {
    Arc::new(RwLock::new(ShareState { nodes, format }))
}

/// Generate a random subscription token (UUID without dashes, 32 hex chars).
pub fn generate_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Best-effort LAN IPv4 of this machine.
///
/// Uses a UDP "connect" to a public address: no packets are actually sent,
/// the kernel just picks the outgoing interface, and we read back its
/// address. Returns `None` when no usable LAN interface is found.
pub fn local_lan_ip() -> Option<IpAddr> {
    let socket = std::net::UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    socket.connect(("8.8.8.8", 80)).ok()?;
    let ip = socket.local_addr().ok()?.ip();
    if ip.is_loopback() { None } else { Some(ip) }
}

/// A running share server. Drop-in replacement handle: query the bound
/// address/token, or stop it (gracefully via [`ShareServer::stop`], or just
/// signal via [`ShareServer::shutdown`]).
pub struct ShareServer {
    addr: SocketAddr,
    token: String,
    stop_tx: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl ShareServer {
    pub fn bound_addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// Signal the accept loop to stop without waiting for it (safe in `Drop`).
    pub fn shutdown(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
    }

    /// Stop accepting connections and wait for the accept loop to exit.
    pub async fn stop(mut self) {
        self.shutdown();
        let _ = tokio::time::timeout(Duration::from_secs(2), &mut self.task).await;
    }
}

/// Start the share server bound to `bind:port` (use `"0.0.0.0"` for LAN
/// reachability). Pass port `0` to let the OS pick a free port.
pub async fn start_share_server(
    bind: &str,
    port: u16,
    token: String,
    state: SharedShareState,
) -> Result<ShareServer> {
    let listener = TcpListener::bind((bind, port))
        .await
        .with_context(|| format!("failed to bind share server on {}:{}", bind, port))?;
    let addr = listener.local_addr()?;

    let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
    let server_token = token.clone();

    let task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut stop_rx => break,
                accepted = listener.accept() => {
                    match accepted {
                        Ok((stream, peer)) => {
                            let token = server_token.clone();
                            let state = state.clone();
                            tokio::spawn(async move {
                                if let Err(err) =
                                    handle_connection(stream, peer, &token, &state).await
                                {
                                    tracing::debug!(
                                        "share-server: connection from {} ended: {}",
                                        peer,
                                        err
                                    );
                                }
                            });
                        }
                        Err(err) => {
                            tracing::warn!("share-server: accept error: {}", err);
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    }
                }
            }
        }
        tracing::info!("share-server: stopped on {}", addr);
    });

    tracing::info!("share-server: listening on {}", addr);
    Ok(ShareServer {
        addr,
        token,
        stop_tx: Some(stop_tx),
        task,
    })
}

async fn handle_connection(
    mut stream: TcpStream,
    peer: SocketAddr,
    token: &str,
    state: &SharedShareState,
) -> Result<()> {
    let head = match tokio::time::timeout(READ_TIMEOUT, read_head(&mut stream)).await {
        Ok(result) => result?,
        Err(_) => {
            let response = respond(
                408,
                "Request Timeout",
                "text/plain; charset=utf-8",
                "timeout",
            );
            stream.write_all(&response).await?;
            let _ = stream.shutdown().await;
            return Ok(());
        }
    };

    let response = route(&head, peer, token, state);
    stream.write_all(&response).await?;
    let _ = stream.shutdown().await;
    Ok(())
}

/// Read until the end of the HTTP request head (`\r\n\r\n`), capped at
/// [`MAX_HEAD_BYTES`].
async fn read_head(stream: &mut TcpStream) -> Result<String> {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 4096];
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > MAX_HEAD_BYTES {
            anyhow::bail!("request head too large");
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn route(head: &str, peer: SocketAddr, token: &str, state: &SharedShareState) -> Vec<u8> {
    let request_line = head.lines().next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    // Log only the path: the query carries the subscription token and must
    // never end up in log files.
    tracing::info!("share-server: {} {} from {}", method, path, peer.ip());

    if method != "GET" {
        return respond(
            405,
            "Method Not Allowed",
            "text/plain; charset=utf-8",
            "only GET is supported",
        );
    }

    if path != "/sub" {
        return respond(404, "Not Found", "text/plain; charset=utf-8", "not found");
    }

    let params: std::collections::HashMap<String, String> =
        url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();

    if !token_valid(params.get("token").map(String::as_str), token) {
        tracing::warn!("share-server: rejected {} — invalid token", peer.ip());
        return respond(
            403,
            "Forbidden",
            "text/plain; charset=utf-8",
            "invalid token",
        );
    }

    let (nodes, default_format) = match state.read() {
        Ok(guard) => (guard.nodes.clone(), guard.format),
        Err(_) => (Vec::new(), SubscriptionFormat::V2ray),
    };
    let format = params
        .get("format")
        .and_then(|f| SubscriptionFormat::parse(f))
        .unwrap_or(default_format);

    respond(200, "OK", format.content_type(), &format.render(&nodes))
}

/// Constant-time token comparison (length-checked first to avoid oracle on
/// obviously wrong lengths).
fn token_valid(provided: Option<&str>, expected: &str) -> bool {
    match provided {
        Some(p) if p.len() == expected.len() => {
            p.as_bytes()
                .iter()
                .zip(expected.as_bytes())
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0
        }
        _ => false,
    }
}

/// Serialize an HTTP/1.1 response. Always `Connection: close` and no-cache —
/// subscription content must never be cached by intermediaries.
fn respond(status: u16, reason: &str, content_type: &str, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {} {}\r\n\
         Content-Type: {}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store, no-cache, must-revalidate\r\n\
         Pragma: no-cache\r\n\
         Connection: close\r\n\
         \r\n\
         {}",
        status,
        reason,
        content_type,
        body.len(),
        body
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::clash::parse_clash_config;
    use crate::config::model::ProxyProtocol;
    use base64::Engine;
    use base64::engine::general_purpose;

    fn sample_node() -> Node {
        Node {
            name: "Test SS".to_string(),
            server: "1.2.3.4".to_string(),
            port: 8388,
            protocol: ProxyProtocol::Shadowsocks {
                cipher: "aes-256-gcm".to_string(),
                password: "pw".to_string(),
                udp: true,
                shadow_tls: None,
            },
            transport: None,
            latency_ms: None,
            tags: vec![],
            extra: Default::default(),
        }
    }

    /// Fire a raw HTTP request at the server and return the full response.
    async fn raw_request(port: u16, request: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        String::from_utf8_lossy(&buf).into_owned()
    }

    #[test]
    fn token_validation_logic() {
        assert!(token_valid(Some("abc"), "abc"));
        assert!(!token_valid(Some("abd"), "abc"));
        assert!(!token_valid(Some("ab"), "abc"));
        assert!(!token_valid(Some("abcd"), "abc"));
        assert!(!token_valid(None, "abc"));
        assert!(!token_valid(Some(""), "abc"));
    }

    #[test]
    fn generated_tokens_are_random_and_url_safe() {
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b);
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn serves_subscription_and_enforces_token() {
        let state = new_shared_state(vec![sample_node()], SubscriptionFormat::V2ray);
        let server = start_share_server("127.0.0.1", 0, "sekret".to_string(), state)
            .await
            .unwrap();
        let port = server.port();
        assert_ne!(port, 0);

        // Wrong token → 403
        let resp = raw_request(port, "GET /sub?token=wrong HTTP/1.1\r\nHost: x\r\n\r\n").await;
        assert!(resp.starts_with("HTTP/1.1 403"), "got: {}", resp);

        // Missing token → 403
        let resp = raw_request(port, "GET /sub HTTP/1.1\r\nHost: x\r\n\r\n").await;
        assert!(resp.starts_with("HTTP/1.1 403"), "got: {}", resp);

        // Unknown path → 404
        let resp = raw_request(port, "GET /other?token=sekret HTTP/1.1\r\nHost: x\r\n\r\n").await;
        assert!(resp.starts_with("HTTP/1.1 404"), "got: {}", resp);

        // Non-GET method → 405
        let resp = raw_request(port, "POST /sub?token=sekret HTTP/1.1\r\nHost: x\r\n\r\n").await;
        assert!(resp.starts_with("HTTP/1.1 405"), "got: {}", resp);

        // Correct token, default format (v2ray base64)
        let resp = raw_request(port, "GET /sub?token=sekret HTTP/1.1\r\nHost: x\r\n\r\n").await;
        assert!(resp.starts_with("HTTP/1.1 200"), "got: {}", resp);
        assert!(
            resp.contains("Cache-Control: no-store"),
            "no-cache header missing: {}",
            resp
        );
        let body = resp.split("\r\n\r\n").nth(1).unwrap_or_default();
        let decoded =
            String::from_utf8(general_purpose::STANDARD.decode(body.trim()).unwrap()).unwrap();
        assert!(
            decoded.starts_with("ss://"),
            "base64 body should decode to share URIs, got: {}",
            decoded
        );

        // Correct token, explicit clash format → parses back through our parser
        let resp = raw_request(
            port,
            "GET /sub?token=sekret&format=clash HTTP/1.1\r\nHost: x\r\n\r\n",
        )
        .await;
        assert!(resp.starts_with("HTTP/1.1 200"), "got: {}", resp);
        assert!(resp.contains("text/yaml"), "got: {}", resp);
        let body = resp.split("\r\n\r\n").nth(1).unwrap_or_default();
        let parsed = parse_clash_config(body).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].server, "1.2.3.4");

        server.stop().await;
    }

    #[tokio::test]
    async fn state_updates_are_visible_to_later_requests() {
        let state = new_shared_state(Vec::new(), SubscriptionFormat::Clash);
        let server = start_share_server("127.0.0.1", 0, "sekret".to_string(), state.clone())
            .await
            .unwrap();
        let port = server.port();

        // Initially empty
        let resp = raw_request(port, "GET /sub?token=sekret HTTP/1.1\r\nHost: x\r\n\r\n").await;
        let body = resp.split("\r\n\r\n").nth(1).unwrap_or_default();
        assert_eq!(parse_clash_config(body).unwrap().len(), 0);

        // Update shared state → next request sees it
        state.write().unwrap().nodes = vec![sample_node()];
        let resp = raw_request(port, "GET /sub?token=sekret HTTP/1.1\r\nHost: x\r\n\r\n").await;
        let body = resp.split("\r\n\r\n").nth(1).unwrap_or_default();
        assert_eq!(parse_clash_config(body).unwrap().len(), 1);

        server.stop().await;
    }

    #[tokio::test]
    async fn stop_shuts_down_listener_cleanly() {
        let state = new_shared_state(Vec::new(), SubscriptionFormat::V2ray);
        let server = start_share_server("127.0.0.1", 0, "t".to_string(), state)
            .await
            .unwrap();
        let port = server.port();
        server.stop().await;

        // The accept loop has exited. Under heavy parallel test load the OS
        // may instantly hand the freed ephemeral port to another test's
        // listener, so verify behavior — anything that answers must not be
        // *our* server — rather than demanding connection refusal.
        match TcpStream::connect(("127.0.0.1", port)).await {
            Err(_) => {} // listener fully closed — the common case
            Ok(mut stream) => {
                stream
                    .write_all(b"GET /sub?token=t HTTP/1.1\r\n\r\n")
                    .await
                    .unwrap();
                let mut buf = Vec::new();
                stream.read_to_end(&mut buf).await.unwrap();
                let resp = String::from_utf8_lossy(&buf);
                assert!(
                    !resp.starts_with("HTTP/1.1 200"),
                    "stopped server must not keep serving, got: {}",
                    resp
                );
            }
        }
    }
}

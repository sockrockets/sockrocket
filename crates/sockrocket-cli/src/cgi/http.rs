//! Loopback-only authenticated HTTP adapter. Browser requests use firmware CGI.
use super::{dispatch, read_only_action, sockrocket_dir};
use anyhow::{Context, Result};
use serde_json::{Value as J, json};
use std::{fs, sync::Arc, time::Duration};

/// Default port for `sockrocket-cli api` (Web UI ↔ router JSON API).
pub const DEFAULT_API_PORT: u16 = 18188;

pub(super) struct ApiRequest {
    pub(super) method: String,
    pub(super) action: String,
    pub(super) body: String,
    origin: Option<String>,
    authorization: Option<String>,
    host: Option<String>,
}

fn valid_bearer(header: Option<&str>, token: &str) -> bool {
    let Some(value) = header.and_then(|header| header.strip_prefix("Bearer ")) else {
        return false;
    };
    value.len() == token.len()
        && value
            .bytes()
            .zip(token.bytes())
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

fn api_token() -> Result<String> {
    let path = sockrocket_dir().join("api-token");
    fs::create_dir_all(sockrocket_dir())?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&path) {
        Ok(mut file) => {
            use std::io::Write;
            let token = rand::random::<[u8; 32]>()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            file.write_all(token.as_bytes())?;
            file.sync_all()?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    anyhow::ensure!(
        !fs::symlink_metadata(&path)?.file_type().is_symlink(),
        "API token cannot be a symlink"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    let token = fs::read_to_string(path)?;
    anyhow::ensure!(
        token.len() == 64 && token.bytes().all(|c| c.is_ascii_hexdigit()),
        "Invalid API token"
    );
    Ok(token)
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Read one HTTP request. Returns None on malformed/oversized input or EOF.
/// Caps: 16 KB headers, 1 MB body — plenty for save_config payloads.
pub(super) async fn read_request(sock: &mut tokio::net::TcpStream) -> Option<ApiRequest> {
    use tokio::io::AsyncReadExt;

    let mut buf: Vec<u8> = Vec::with_capacity(2048);
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            if pos + 4 > 16 * 1024 {
                return None;
            }
            break pos;
        }
        if buf.len() >= 16 * 1024 {
            return None;
        }
        match sock.read(&mut tmp).await {
            Ok(0) | Err(_) => return None,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    };

    let head = std::str::from_utf8(&buf[..header_end]).ok()?;
    let mut lines = head.split("\r\n");
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_uppercase();
    let target = parts.next()?;
    if target.split('?').next()? != "/api"
        || !matches!(parts.next()?, "HTTP/1.0" | "HTTP/1.1")
        || parts.next().is_some()
        || !matches!(method.as_str(), "GET" | "POST" | "OPTIONS")
    {
        return None;
    }

    // Query string: /api?action=version&...
    let mut action = String::new();
    if let Some((_, qs)) = target.split_once('?') {
        for pair in qs.split('&') {
            if let Some((k, v)) = pair.split_once('=')
                && k == "action"
            {
                action = v.to_string();
            }
        }
    }

    let mut content_len = None;
    let mut origin = None;
    let mut authorization = None;
    let mut host = None;
    for line in lines {
        let (name, value) = line.split_once(':')?;
        let name = name.to_ascii_lowercase();
        let field = match name.as_str() {
            "origin" => Some(&mut origin),
            "authorization" => Some(&mut authorization),
            "host" => Some(&mut host),
            _ => None,
        };
        if let Some(field) = field {
            if field.is_some() {
                return None;
            }
            *field = Some(value.trim().to_owned());
        }
        // This server has no chunked decoder; ambiguous framing is rejected.
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return None;
        }
        if name.eq_ignore_ascii_case("content-length") {
            if content_len.is_some() || !value.trim().bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            content_len = Some(value.trim().parse::<usize>().ok()?);
        }
    }
    let content_len = content_len.unwrap_or(0);
    if content_len > 1024 * 1024 {
        return None;
    }

    let mut body = buf.split_off(header_end + 4);
    while body.len() < content_len {
        match sock.read(&mut tmp).await {
            Ok(0) | Err(_) => return None,
            Ok(n) => body.extend_from_slice(&tmp[..n]),
        }
    }
    body.truncate(content_len);

    Some(ApiRequest {
        method,
        action,
        body: String::from_utf8(body).ok()?,
        origin,
        authorization,
        host,
    })
}

/// Local administration API. The browser uses firmware-authenticated CGI.
pub async fn serve_api(port: u16) -> Result<()> {
    serve_api_authenticated(port, Arc::new(api_token()?)).await
}

pub(super) async fn serve_api_authenticated(port: u16, token: Arc<String>) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;
    use tokio::sync::Semaphore;

    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .with_context(|| format!("bind API port {port}"))?;
    println!("sockrocket API listening on 127.0.0.1:{port} (authenticated)");

    // Cap in-flight CGI handlers. Unbounded accept→spawn_blocking let a
    // Test-all + status polling storm open dozens of threads on a 1-core
    // ARM router and stall the data-plane / DNS.
    let api_slots = Arc::new(Semaphore::new(
        sockrocket_core::resources::ResourceBudget::detect().api_requests,
    ));

    loop {
        let (mut sock, _peer) = match listener.accept().await {
            Ok(x) => x,
            Err(_) => continue,
        };
        // Reject excess connections before spawning. Waiting for a permit
        // inside a task still leaves an unbounded queue of tasks/sockets,
        // which exhausts router memory/FDs while six slow requests run.
        let Ok(slot) = api_slots.clone().try_acquire_owned() else {
            continue;
        };
        let token = token.clone();
        tokio::spawn(async move {
            let _slot = slot;
            let req = match tokio::time::timeout(Duration::from_secs(10), read_request(&mut sock))
                .await
            {
                Ok(Some(r)) => r,
                _ => return,
            };
            let local_host = req.host.as_deref().is_none_or(|host| {
                [format!("127.0.0.1:{port}"), format!("localhost:{port}")]
                    .iter()
                    .any(|allowed| host == allowed)
            });
            if req.origin.is_some() || !local_host {
                let _ = sock
                    .write_all(
                        b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
                return;
            }
            if req.method == "OPTIONS" {
                let _ = sock.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                return;
            }
            if !valid_bearer(req.authorization.as_deref(), &token) {
                let _ = sock.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                return;
            }
            if req.method != "POST" && !read_only_action(&req.action) {
                let _ = sock.write_all(b"HTTP/1.1 405 Method Not Allowed\r\nAllow: POST\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                return;
            }
            if !req.body.is_empty() && serde_json::from_str::<J>(&req.body).is_err() {
                let _ = sock.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                return;
            }
            // dispatch() shells out (speedtest, sockrocket.sh, …) and can take
            // seconds — keep it off the async reactor.
            let out = tokio::task::spawn_blocking(move || {
                let post: J = if req.body.is_empty() {
                    J::Null
                } else {
                    serde_json::from_str(&req.body).unwrap_or(J::Null)
                };
                dispatch(&req.action, &post)
            })
            .await
            .unwrap_or_else(|_| json!({"ok": false, "msg": "internal error"}));
            let body = serde_json::to_string(&out).unwrap_or_else(|_| "{}".into());
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nCache-Control: no-cache\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = sock.write_all(head.as_bytes()).await;
            let _ = sock.write_all(body.as_bytes()).await;
        });
    }
}

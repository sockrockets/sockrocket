use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::connector::SharedOutbound;

/// Failure category of a node probe (latency / speed test).
///
/// Probe functions return bare `anyhow::Error`, which loses structure; this
/// classification lets callers distinguish "the node cannot carry traffic"
/// (Timeout / Unreachable / Tls — show UNREACHABLE, never a latency figure)
/// from protocol-level problems, without string-matching at the call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeFailureKind {
    /// Nothing answered within the deadline (dropped packets, silent server).
    Timeout,
    /// The server could not be reached at all: TCP connect refused/reset,
    /// no route to host, or DNS resolution failure.
    Unreachable,
    /// TCP connected but the TLS/QUIC handshake failed (certificate,
    /// fingerprint, or reality key mismatch).
    Tls,
    /// The server answered but the proxy-protocol exchange failed
    /// (authentication, unexpected response).
    Protocol,
    /// Anything else (mid-stream I/O errors, local misconfiguration).
    Other,
}

impl ProbeFailureKind {
    /// Short English label for logs and tooltips.
    pub fn label(&self) -> &'static str {
        match self {
            ProbeFailureKind::Timeout => "timeout",
            ProbeFailureKind::Unreachable => "unreachable",
            ProbeFailureKind::Tls => "tls handshake failed",
            ProbeFailureKind::Protocol => "protocol error",
            ProbeFailureKind::Other => "unknown error",
        }
    }

    /// Short label for the "unreachable" badge in the GUI.
    pub fn badge_label(&self) -> &'static str {
        match self {
            ProbeFailureKind::Timeout => "Timeout",
            ProbeFailureKind::Unreachable => "Server unreachable",
            ProbeFailureKind::Tls => "TLS handshake failed",
            ProbeFailureKind::Protocol => "Protocol error",
            ProbeFailureKind::Other => "Unknown error",
        }
    }
}

/// Classify a probe error into a [`ProbeFailureKind`].
///
/// Walks the whole `anyhow` cause chain: `std::io::Error` kinds give the most
/// reliable signal, message keywords are the fallback for protocol errors that
/// were converted to strings upstream. Timeout is checked before TLS so a
/// "tls handshake timed out" error reports as Timeout.
pub fn classify_probe_error(err: &anyhow::Error) -> ProbeFailureKind {
    use std::io::ErrorKind;
    for cause in err.chain() {
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            match io.kind() {
                ErrorKind::TimedOut => return ProbeFailureKind::Timeout,
                ErrorKind::ConnectionRefused
                | ErrorKind::ConnectionReset
                | ErrorKind::ConnectionAborted
                | ErrorKind::NotConnected
                | ErrorKind::AddrNotAvailable => return ProbeFailureKind::Unreachable,
                _ => {}
            }
        }
        let msg = cause.to_string().to_lowercase();
        if msg.contains("timed out") || msg.contains("timeout") || msg.contains("deadline") {
            return ProbeFailureKind::Timeout;
        }
        if msg.contains("tls")
            || msg.contains("certificate")
            || msg.contains("reality")
            || msg.contains("quic")
        {
            return ProbeFailureKind::Tls;
        }
        if msg.contains("refused")
            || msg.contains("unreachable")
            || msg.contains("no route")
            || msg.contains("reset by peer")
            || msg.contains("dns")
            || msg.contains("failed to lookup")
            || msg.contains("name or service")
            || msg.contains("nodename nor servname")
        {
            return ProbeFailureKind::Unreachable;
        }
        if msg.contains("auth")
            || msg.contains("protocol")
            || msg.contains("unexpected response")
            || msg.contains("invalid response")
        {
            return ProbeFailureKind::Protocol;
        }
    }
    ProbeFailureKind::Other
}

/// Result of a speed test for a single node.
#[derive(Debug, Clone)]
pub struct SpeedTestResult {
    /// TCP connection latency in ms
    pub latency_ms: u32,
    /// Download speed in KB/s (None if download test failed or was skipped)
    pub download_kbps: Option<u32>,
}

/// Perform a real speed test for a node through its outbound connection.
///
/// 1. Measures TCP + protocol handshake latency by connecting to a test host.
/// 2. Sends an HTTP GET and measures download throughput.
pub async fn speed_test_node(
    outbound: &SharedOutbound,
    timeout_secs: u64,
) -> Result<SpeedTestResult> {
    let timeout = std::time::Duration::from_secs(timeout_secs);

    // Warm up the connection pool / QUIC session before timing anything.
    // Without this, the "latency" below is really the full cold-handshake cost
    // (TCP + TLS1.3 + reality/QUIC negotiation), which overstates what a user
    // actually experiences once the pool is warm. A throwaway connect fills
    // the pool so the timed connect measures warm-path latency. Non-fatal if
    // it fails — the timed connect below will surface the real error.
    if let Ok(Ok(mut warm)) =
        tokio::time::timeout(timeout, outbound.connect("speed.cloudflare.com", 80)).await
    {
        let _ = warm.shutdown().await;
    }

    // Phase 1: Latency — connect through the proxy to speed.cloudflare.com:80
    let start = std::time::Instant::now();
    let mut stream = tokio::time::timeout(timeout, outbound.connect("speed.cloudflare.com", 80))
        .await
        .map_err(|_| anyhow::anyhow!("connection timed out"))??;
    let latency_ms = start.elapsed().as_millis() as u32;

    // Phase 2: Download speed — request a 1 MB test payload from Cloudflare's speed endpoint
    let request = b"GET /__down?bytes=1048576 HTTP/1.1\r\nHost: speed.cloudflare.com\r\nConnection: close\r\n\r\n";
    stream.write_all(request).await?;

    let dl_start = std::time::Instant::now();
    let mut total_bytes: u64 = 0;
    let mut buf = vec![0u8; 8192];
    loop {
        let read_result =
            tokio::time::timeout(std::time::Duration::from_secs(10), stream.read(&mut buf)).await;

        match read_result {
            Ok(Ok(0)) => break,
            Ok(Ok(n)) => total_bytes += n as u64,
            Ok(Err(_)) => break,
            Err(_) => break, // timeout
        }
    }
    let dl_elapsed = dl_start.elapsed();

    // Cloudflare __down returns exactly the requested number of bytes;
    // calculate throughput from the actual bytes received vs elapsed time.
    let download_kbps = if total_bytes > 100 && dl_elapsed.as_millis() > 0 {
        Some(((total_bytes as f64 / 1024.0) / dl_elapsed.as_secs_f64()) as u32)
    } else {
        None
    };

    Ok(SpeedTestResult {
        latency_ms,
        download_kbps,
    })
}

/// Perform a quick latency-only test by connecting through the proxy outbound.
/// This measures real protocol handshake time, not just raw TCP.
pub async fn latency_test_node(outbound: &SharedOutbound, timeout_secs: u64) -> Result<u32> {
    let timeout = std::time::Duration::from_secs(timeout_secs);
    let start = std::time::Instant::now();
    let mut stream = tokio::time::timeout(timeout, outbound.connect("www.gstatic.com", 80))
        .await
        .map_err(|_| anyhow::anyhow!("connection timed out"))??;

    // Send a minimal HTTP request to ensure the tunnel is live
    stream
        .write_all(b"HEAD / HTTP/1.1\r\nHost: www.gstatic.com\r\nConnection: close\r\n\r\n")
        .await?;
    let mut buf = [0u8; 64];
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut buf)).await;

    Ok(start.elapsed().as_millis() as u32)
}

/// Perform a fast TCP-only latency test by connecting directly to the server.
///
/// This measures just the raw TCP handshake latency to the proxy server,
/// similar to what Karing and other proxy clients display as "latency".
/// It does NOT go through the proxy protocol or connect to a target.
pub async fn tcp_latency_test(server: &str, port: u16, timeout_secs: u64) -> Result<u32> {
    let timeout = std::time::Duration::from_secs(timeout_secs);
    let addr = super::transport::format_host_port(server, port);
    let start = std::time::Instant::now();
    let _stream = tokio::time::timeout(timeout, TcpStream::connect(&addr))
        .await
        .map_err(|_| anyhow::anyhow!("TCP connect to {} timed out ({}s)", addr, timeout_secs))??;
    Ok(start.elapsed().as_millis() as u32)
}

/// Perform an HTTP latency test through a proxy outbound with warmup.
///
/// Does two connections: a warmup (to establish QUIC/TLS sessions for
/// protocols like TUIC) and a measurement. This gives a realistic "warm"
/// latency that represents actual browsing performance.
///
/// Typical results: 100-500ms for TUIC/Hysteria2, 50-200ms for TCP protocols.
pub async fn http_latency_test(outbound: &SharedOutbound, timeout_secs: u64) -> Result<u32> {
    let timeout = std::time::Duration::from_secs(timeout_secs);

    // Phase 1: Warmup — establish underlying connections (QUIC session, etc.).
    // Allow up to half the caller budget (min 2s, max 5s) so Reality's
    // hybrid→classic discovery can finish here instead of burning the
    // measurement phase (and so cold router CPUs don't false-timeout).
    let warmup_timeout = std::cmp::min(
        std::cmp::max(timeout / 2, std::time::Duration::from_secs(2)),
        std::time::Duration::from_secs(5),
    );
    match tokio::time::timeout(warmup_timeout, outbound.connect("www.gstatic.com", 80)).await {
        Ok(Ok(mut s)) => {
            // write_all needs its own timeout: on a half-dead QUIC stream
            // (connection object alive, network path gone) a write can block
            // on flow-control credit forever — an unguarded write here wedged
            // the health-check loop permanently (probes just stopped).
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                s.write_all(
                    b"GET /generate_204 HTTP/1.1\r\nHost: www.gstatic.com\r\nConnection: close\r\n\r\n",
                ),
            )
            .await;
            let mut buf = [0u8; 128];
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), s.read(&mut buf)).await;
        }
        Ok(Err(e)) => {
            tracing::debug!("http_latency_test warmup failed (non-fatal): {:#}", e);
        }
        Err(_) => {
            tracing::debug!("http_latency_test warmup timed out (non-fatal)",);
        }
    }

    // Phase 2: Measure — connect again using cached/warm connections.
    // Share one deadline across fallback targets so a slow Reality handshake
    // does not get retried 3× with a full timeout each (which looked like
    // "most nodes timeout" on Merlin batch tests).
    let measure_deadline = tokio::time::Instant::now() + timeout;
    let targets: &[(&str, u16)] = &[
        ("www.gstatic.com", 80),
        ("cp.cloudflare.com", 80),
        ("www.google.com", 80),
    ];
    let start = std::time::Instant::now();
    // Start fallback targets at staggered times without cancelling a slow
    // primary. Splitting the deadline into three short timeouts rejects a
    // working two-second response when the caller allowed five seconds.
    // At most three exchanges run, all bounded by the same deadline; dropping
    // the JoinSet on success/cancellation aborts the remaining exchanges.
    let mut probes = tokio::task::JoinSet::new();
    for (i, &(host, port)) in targets.iter().enumerate() {
        let outbound = outbound.clone();
        let launch_at = measure_deadline - timeout + timeout * i as u32 / targets.len() as u32;
        probes.spawn(async move {
            tokio::time::sleep_until(launch_at).await;
            if i > 0 {
                tracing::debug!("http_latency_test: trying fallback target {host}");
            }
            http_probe_exchange(&outbound, host, port).await
        });
    }
    let mut last_err: Option<anyhow::Error> = None;
    loop {
        let result = match tokio::time::timeout_at(measure_deadline, probes.join_next()).await {
            Ok(Some(Ok(result))) => result,
            Ok(Some(Err(error))) => Err(error.into()),
            Ok(None) => break,
            Err(_) => {
                last_err = Some(anyhow::anyhow!("HTTP probe timed out"));
                break;
            }
        };
        match result {
            Ok(()) => return Ok(start.elapsed().as_millis() as u32),
            Err(e) => {
                // Handshake / protocol failure will repeat on every target
                // through the same outbound — don't burn the rest of the budget.
                let msg = format!("{e:#}");
                let fatal = msg.contains("REALITY")
                    || msg.contains("TLS")
                    || msg.contains("handshake")
                    || msg.contains("authentication")
                    || msg.contains("Ed25519");
                last_err = Some(e);
                if fatal {
                    break;
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("HTTP probe timed out")))
}

async fn http_probe_exchange(outbound: &SharedOutbound, host: &str, port: u16) -> Result<()> {
    let mut stream = outbound.connect(host, port).await?;
    let request =
        format!("GET /generate_204 HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await?;
    stream.flush().await?;

    // Bound memory even if a peer sends an endless or malformed header.
    let mut response = [0u8; 4096];
    let mut used = 0;
    loop {
        let read = stream.read(&mut response[used..]).await?;
        anyhow::ensure!(
            read > 0,
            "HTTP probe response ended before complete headers"
        );
        used += read;
        if let Some(end) = response[..used].windows(4).position(|b| b == b"\r\n\r\n") {
            let line_end = response[..end + 2]
                .windows(2)
                .position(|b| b == b"\r\n")
                .unwrap();
            let line = std::str::from_utf8(&response[..line_end])?;
            let mut fields = line.split_ascii_whitespace();
            anyhow::ensure!(
                matches!(fields.next(), Some("HTTP/1.0" | "HTTP/1.1")),
                "Invalid HTTP probe response"
            );
            let status = fields.next().unwrap_or("");
            anyhow::ensure!(
                status.len() == 3 && status.bytes().all(|b| b.is_ascii_digit()),
                "Invalid HTTP probe response status"
            );
            let status: u16 = status.parse()?;
            anyhow::ensure!(
                status == 204,
                "HTTP probe expected status 204, received {status}"
            );
            return Ok(());
        }
        anyhow::ensure!(
            used < response.len(),
            "HTTP probe response headers too large"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::connector::{BoxProxyStream, Outbound};
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;

    struct HttpProbeOutbound {
        reply: &'static [u8],
        primary_reply: Option<&'static [u8]>,
        primary_connect_stalls: bool,
        reply_delay: std::time::Duration,
        requests: Arc<std::sync::Mutex<Vec<(String, String)>>>,
    }

    impl Outbound for HttpProbeOutbound {
        fn connect(
            &self,
            host: &str,
            _port: u16,
        ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
            let host = host.to_owned();
            let stalls = self.primary_connect_stalls && host == "www.gstatic.com";
            let requests = self.requests.clone();
            let reply_delay = self.reply_delay;
            let reply = if host == "www.gstatic.com" {
                self.primary_reply.unwrap_or(self.reply)
            } else {
                self.reply
            };
            Box::pin(async move {
                if stalls {
                    std::future::pending::<()>().await;
                }
                let (client, mut server) = tokio::io::duplex(8192);
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut byte = [0; 1];
                    while server.read_exact(&mut byte).await.is_ok() {
                        request.push(byte[0]);
                        if request.ends_with(b"\r\n\r\n") {
                            break;
                        }
                    }
                    requests
                        .lock()
                        .unwrap()
                        .push((host, String::from_utf8(request).unwrap()));
                    if reply.is_empty() {
                        // Accept writes but never return a response, like a
                        // tunnel whose peer no longer carries application data.
                        let mut discarded = [0; 1];
                        let _ = server.read(&mut discarded).await;
                    } else {
                        tokio::time::sleep(reply_delay).await;
                        // Fragment the headers to exercise incremental reads.
                        for byte in reply {
                            if server.write_all(&[*byte]).await.is_err() {
                                break;
                            }
                            tokio::task::yield_now().await;
                        }
                    }
                });
                Ok(Box::new(client) as BoxProxyStream)
            })
        }

        fn name(&self) -> &str {
            "mock HTTP probe"
        }
    }

    fn http_probe_outbound(reply: &'static [u8]) -> SharedOutbound {
        SharedOutbound(Arc::new(HttpProbeOutbound {
            reply,
            primary_reply: None,
            primary_connect_stalls: false,
            reply_delay: std::time::Duration::ZERO,
            requests: Default::default(),
        }))
    }

    #[tokio::test(start_paused = true)]
    async fn http_probe_rejects_a_tunnel_that_only_accepts_writes() {
        let outbound = http_probe_outbound(b"");
        let started = tokio::time::Instant::now();
        let error = http_latency_test(&outbound, 5).await.unwrap_err();
        assert_eq!(classify_probe_error(&error), ProbeFailureKind::Timeout);
        assert!(started.elapsed() <= std::time::Duration::from_secs(12));
    }

    #[tokio::test(start_paused = true)]
    async fn http_probe_rejects_eof_invalid_and_error_responses() {
        for reply in [
            &b"HTTP/1.1 204 No Content\r\n"[..],
            &b"not HTTP\r\n\r\n"[..],
            &b"HTTP/1.1 20x Invalid Status\r\n\r\n"[..],
            &b"HTTP/1.1 503 Service Unavailable\r\n\r\n"[..],
            &b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"[..],
            &b"HTTP/1.1 302 Found\r\nLocation: https://login.invalid/\r\n\r\n"[..],
            &[b'x'; 4096][..],
        ] {
            let outbound = http_probe_outbound(reply);
            assert!(
                http_latency_test(&outbound, 5).await.is_err(),
                "accepted a failed HTTP exchange: {reply:?}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn http_probe_accepts_complete_fragmented_headers() {
        for reply in [
            &b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n"[..],
            &b"HTTP/1.0 204 No Content\r\n\r\n"[..],
        ] {
            assert!(
                http_latency_test(&http_probe_outbound(reply), 5)
                    .await
                    .is_ok()
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn http_probe_tries_fallback_after_http_failure_with_matching_host() {
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let outbound = SharedOutbound(Arc::new(HttpProbeOutbound {
            reply: b"HTTP/1.1 204 No Content\r\n\r\n",
            primary_reply: Some(b"HTTP/1.1 503 Service Unavailable\r\n\r\n"),
            primary_connect_stalls: false,
            reply_delay: std::time::Duration::ZERO,
            requests: requests.clone(),
        }));
        assert!(http_latency_test(&outbound, 5).await.is_ok());
        let requests = requests.lock().unwrap();
        assert!(requests.iter().any(|(host, _)| host == "cp.cloudflare.com"));
        for (host, request) in requests.iter() {
            assert!(request.contains(&format!("\r\nHost: {host}\r\n")));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn http_probe_uses_fallback_when_primary_connect_never_finishes() {
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let outbound = SharedOutbound(Arc::new(HttpProbeOutbound {
            reply: b"HTTP/1.1 204 No Content\r\n\r\n",
            primary_reply: None,
            primary_connect_stalls: true,
            reply_delay: std::time::Duration::ZERO,
            requests: requests.clone(),
        }));
        let start = tokio::time::Instant::now();
        http_latency_test(&outbound, 5)
            .await
            .expect("a blackholed primary must leave time for fallback targets");
        assert!(start.elapsed() <= std::time::Duration::from_secs(7));
        assert!(
            requests
                .lock()
                .unwrap()
                .iter()
                .any(|(host, _)| host == "cp.cloudflare.com")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn http_probe_does_not_shorten_a_working_targets_deadline() {
        let outbound = SharedOutbound(Arc::new(HttpProbeOutbound {
            reply: b"HTTP/1.1 204 No Content\r\n\r\n",
            primary_reply: None,
            primary_connect_stalls: false,
            reply_delay: std::time::Duration::from_secs(2),
            requests: Default::default(),
        }));
        http_latency_test(&outbound, 5)
            .await
            .expect("a two-second HTTP response must fit the five-second measurement deadline");
    }

    #[tokio::test(start_paused = true)]
    async fn http_probe_does_not_launch_fallbacks_after_success() {
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let outbound = SharedOutbound(Arc::new(HttpProbeOutbound {
            reply: b"HTTP/1.1 204 No Content\r\n\r\n",
            primary_reply: None,
            primary_connect_stalls: false,
            reply_delay: std::time::Duration::ZERO,
            requests: requests.clone(),
        }));
        http_latency_test(&outbound, 5).await.unwrap();
        tokio::time::advance(std::time::Duration::from_secs(6)).await;
        tokio::task::yield_now().await;
        let requests = requests.lock().unwrap();
        assert_eq!(
            requests.len(),
            2,
            "only warmup and the primary should connect"
        );
        assert!(requests.iter().all(|(host, _)| host == "www.gstatic.com"));
    }

    #[test]
    fn classify_timeout_message() {
        let e = anyhow::anyhow!("connection timed out");
        assert_eq!(classify_probe_error(&e), ProbeFailureKind::Timeout);
    }

    #[test]
    fn classify_tcp_timeout_message() {
        let e = anyhow::anyhow!("TCP connect to 1.2.3.4:443 timed out (5s)");
        assert_eq!(classify_probe_error(&e), ProbeFailureKind::Timeout);
    }

    #[test]
    fn classify_io_refused() {
        let e: anyhow::Error =
            std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "connection refused").into();
        assert_eq!(classify_probe_error(&e), ProbeFailureKind::Unreachable);
    }

    #[test]
    fn classify_io_timed_out_kind() {
        let e: anyhow::Error =
            std::io::Error::new(std::io::ErrorKind::TimedOut, "os error 110").into();
        assert_eq!(classify_probe_error(&e), ProbeFailureKind::Timeout);
    }

    #[test]
    fn classify_tls_handshake() {
        let e = anyhow::anyhow!("tls handshake failed: invalid certificate");
        assert_eq!(classify_probe_error(&e), ProbeFailureKind::Tls);
    }

    #[test]
    fn classify_timeout_beats_tls() {
        // A TLS handshake that never finished is a timeout, not a TLS error.
        let e = anyhow::anyhow!("tls handshake timed out");
        assert_eq!(classify_probe_error(&e), ProbeFailureKind::Timeout);
    }

    #[test]
    fn classify_dns_failure() {
        let e = anyhow::anyhow!("dns error: failed to lookup address information");
        assert_eq!(classify_probe_error(&e), ProbeFailureKind::Unreachable);
    }

    #[test]
    fn classify_protocol_error() {
        let e = anyhow::anyhow!("vmess auth failed");
        assert_eq!(classify_probe_error(&e), ProbeFailureKind::Protocol);
    }

    #[test]
    fn classify_unknown_error() {
        let e = anyhow::anyhow!("something odd happened");
        assert_eq!(classify_probe_error(&e), ProbeFailureKind::Other);
    }

    #[test]
    fn classify_nested_cause() {
        let inner = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset");
        let e = anyhow::Error::new(inner).context("proxy connect failed");
        assert_eq!(classify_probe_error(&e), ProbeFailureKind::Unreachable);
    }

    #[tokio::test]
    async fn test_latency_direct_outbound() {
        // Test with direct outbound — should succeed if internet is available
        let outbound = SharedOutbound::direct();
        match latency_test_node(&outbound, 10).await {
            Ok(ms) => assert!(ms < 10000, "latency too high: {}ms", ms),
            Err(e) => {
                // Network might not be available in CI
                eprintln!("latency test skipped (no network): {}", e);
            }
        }
    }
}

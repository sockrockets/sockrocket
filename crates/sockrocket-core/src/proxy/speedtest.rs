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
    let measure_deadline = std::time::Instant::now() + timeout;
    let targets: &[(&str, u16)] = &[
        ("www.gstatic.com", 80),
        ("cp.cloudflare.com", 80),
        ("dns.google", 80),
    ];
    let start = std::time::Instant::now();
    let mut stream = None;
    let mut last_err: Option<anyhow::Error> = None;
    for (i, &(host, port)) in targets.iter().enumerate() {
        let rem = measure_deadline.saturating_duration_since(std::time::Instant::now());
        if rem < std::time::Duration::from_millis(250) {
            break;
        }
        if i > 0 {
            tracing::debug!("http_latency_test: trying fallback target {host}");
        }
        match tokio::time::timeout(rem, outbound.connect(host, port)).await {
            Ok(Ok(s)) => {
                stream = Some(s);
                break;
            }
            Ok(Err(e)) => {
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
            Err(_) => {
                last_err = Some(anyhow::anyhow!("connection timed out"));
                // Timeout may be destination-specific; try next with remaining.
            }
        }
    }
    let mut stream = match stream {
        Some(s) => s,
        None => {
            return Err(last_err.unwrap_or_else(|| anyhow::anyhow!("connection timed out")));
        }
    };

    // Bounded write (see warmup comment): never hang the caller on a wedged
    // stream — this probe drives the health-check failover loop.
    let write_budget = measure_deadline
        .saturating_duration_since(std::time::Instant::now())
        .min(std::time::Duration::from_secs(5));
    if write_budget < std::time::Duration::from_millis(100) {
        return Err(anyhow::anyhow!("probe write timed out"));
    }
    tokio::time::timeout(
        write_budget,
        stream.write_all(
            b"GET /generate_204 HTTP/1.1\r\nHost: www.gstatic.com\r\nConnection: close\r\n\r\n",
        ),
    )
    .await
    .map_err(|_| anyhow::anyhow!("probe write timed out"))??;
    let mut buf = [0u8; 128];
    let read_budget = measure_deadline
        .saturating_duration_since(std::time::Instant::now())
        .min(std::time::Duration::from_secs(3));
    if read_budget >= std::time::Duration::from_millis(50) {
        let _ = tokio::time::timeout(read_budget, stream.read(&mut buf)).await;
    }

    Ok(start.elapsed().as_millis() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

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

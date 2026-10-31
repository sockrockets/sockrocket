use std::future::Future;
#[cfg(test)]
use std::net::Ipv4Addr;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose;

use super::clash;
use super::model::*;
use super::singbox;
use super::v2ray;

/// Subscription content format (auto-detected)
#[derive(Debug, PartialEq)]
pub enum SubFormat {
    Clash,
    SingBox,
    V2Ray,
    Base64Lines,
    PlainLines,
}

/// Default HTTP timeout for subscription downloads (seconds).
/// First-add and update-subs both use this budget so Merlin Web UI can wait.
pub const SUB_FETCH_TIMEOUT_SECS: u64 = 30;
/// Bound decoded response bytes before parsing or allocating node structures.
pub const MAX_SUBSCRIPTION_BYTES: usize = 8 * 1024 * 1024;

/// Validate before saving or attempting a download. Errors contain no URL.
pub fn validate_subscription_url(input: &str) -> Result<()> {
    anyhow::ensure!(
        !input.is_empty() && !input.chars().any(char::is_whitespace),
        "Subscription URL must be HTTP or HTTPS without whitespace"
    );
    let url = url::Url::parse(input).map_err(|_| anyhow::anyhow!("Invalid subscription URL"))?;
    anyhow::ensure!(
        matches!(url.scheme(), "http" | "https") && url.host_str().is_some(),
        "Subscription URL requires an HTTP or HTTPS host"
    );
    Ok(())
}

/// Fetch a subscription URL and parse it into nodes.
pub async fn fetch_subscription(sub: &Subscription) -> Result<Vec<Node>> {
    fetch_subscription_with(sub, None).await
}

/// Race direct and local SOCKS downloads when a proxy is available.
/// `socks` is `127.0.0.1:<socks_port>` while the daemon is up.
pub async fn fetch_subscription_with(
    sub: &Subscription,
    socks: Option<SocketAddr>,
) -> Result<Vec<Node>> {
    Ok(fetch_subscription_report_with(sub, socks).await?.nodes)
}

/// Safe import diagnostics: counts only, never subscription entries or credentials.
pub struct SubscriptionFetchReport {
    pub nodes: Vec<Node>,
    pub skipped_entries: usize,
    /// Stable informational codes, distinct from failed or skipped entries.
    pub compatibility_notes: Vec<&'static str>,
}

pub async fn fetch_subscription_report(sub: &Subscription) -> Result<SubscriptionFetchReport> {
    fetch_subscription_report_with(sub, None).await
}

pub async fn fetch_subscription_report_with(
    sub: &Subscription,
    socks: Option<SocketAddr>,
) -> Result<SubscriptionFetchReport> {
    validate_subscription_url(&sub.url)?;
    let download_started = tokio::time::Instant::now();
    let (content, mut nodes) = download_subscription(sub, socks).await?;
    tracing::info!(
        elapsed_ms = download_started.elapsed().as_millis() as u64,
        decoded_bytes = content.len(),
        "Subscription download and validation completed"
    );

    let report_started = tokio::time::Instant::now();
    // The URL has to travel with the nodes: a later refresh replaces this
    // source, and a restart has to know which subscription they belong to.
    super::dedup::tag_subscription_nodes(&mut nodes, &sub.url);
    let skipped_entries = subscription_entry_count(&content).saturating_sub(nodes.len());
    let mut compatibility_notes = Vec::new();
    for node in &nodes {
        if !matches!(node.protocol, ProxyProtocol::Hysteria2 { .. }) {
            continue;
        }
        if node.extra.contains_key(v2ray::HY2_MPORT) {
            compatibility_notes.push("hysteria2_mport_fixed_port");
        }
        let note = match v2ray::hysteria2_fingerprint(node) {
            Some("chrome" | "chrome115") => "hysteria2_chrome_quic_profile",
            Some("none" | "") | None => continue,
            Some(_) => "hysteria2_browser_fingerprint_hint",
        };
        if !compatibility_notes.contains(&note) {
            compatibility_notes.push(note);
        }
    }
    compatibility_notes.sort_unstable();
    compatibility_notes.dedup();
    tracing::info!(
        elapsed_ms = report_started.elapsed().as_millis() as u64,
        node_count = nodes.len(),
        skipped_entries,
        "Subscription import diagnostics completed"
    );
    Ok(SubscriptionFetchReport {
        nodes,
        skipped_entries,
        compatibility_notes,
    })
}

const DIRECT_FETCH_TIMEOUT: Duration = Duration::from_secs(12);
const PROXY_FETCH_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
// Four attempts per route: the initial request and up to three retries.
const SUB_FETCH_RETRIES: usize = 3;
const RETRY_BACKOFF: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyErrorKind {
    Transport,
    Http,
    Permanent,
}

#[derive(Debug)]
struct BodyError {
    kind: BodyErrorKind,
    error: anyhow::Error,
}

/// Run at most two routes concurrently, each with bounded retries.
///
/// A route succeeds only after parsing usable nodes, so a fast HTTP 200
/// block page cannot beat a valid subscription from the other route.
/// The losing future is dropped, cancelling its request and retry timer.
async fn download_subscription(
    sub: &Subscription,
    socks: Option<SocketAddr>,
) -> Result<(String, Vec<Node>)> {
    fetch_before_deadline(async {
        let direct = subscription_client(None, socks.is_some()).map_err(|error| BodyError {
            kind: BodyErrorKind::Permanent,
            error,
        })?;
        let direct = retry_subscription_route(|| fetch_valid_subscription(&direct, sub));
        let Some(socks) = socks else {
            return direct.await;
        };
        let proxy = async {
            let proxy = subscription_client(Some(socks), true).map_err(|error| BodyError {
                kind: BodyErrorKind::Permanent,
                error,
            })?;
            retry_subscription_route(|| fetch_valid_subscription(&proxy, sub)).await
        };
        first_successful_route(direct, proxy).await
    })
    .await
    .map_err(|error| error.error)
}

async fn fetch_before_deadline<T>(
    fetch: impl Future<Output = Result<T, BodyError>>,
) -> Result<T, BodyError> {
    tokio::time::timeout(Duration::from_secs(SUB_FETCH_TIMEOUT_SECS), fetch)
        .await
        .unwrap_or_else(|_| {
            Err(BodyError {
                kind: BodyErrorKind::Transport,
                error: anyhow::anyhow!(
                    "Subscription download timed out after {SUB_FETCH_TIMEOUT_SECS}s"
                ),
            })
        })
}

async fn retry_subscription_route<T, F, A>(mut attempt: F) -> Result<T, BodyError>
where
    F: FnMut() -> A,
    A: Future<Output = Result<T, BodyError>>,
{
    for retry in 0..=SUB_FETCH_RETRIES {
        match attempt().await {
            Err(error) if error.kind != BodyErrorKind::Permanent && retry < SUB_FETCH_RETRIES => {
                tokio::time::sleep(RETRY_BACKOFF * (1 << retry)).await;
            }
            result => return result,
        }
    }
    unreachable!("the final attempt always returns")
}

async fn first_successful_route<T, D, P>(direct: D, proxy: P) -> Result<T, BodyError>
where
    D: Future<Output = Result<T, BodyError>>,
    P: Future<Output = Result<T, BodyError>>,
{
    tokio::pin!(direct, proxy);
    tokio::select! {
        result = &mut direct => match result {
            Ok(value) => Ok(value),
            Err(direct_error) => match proxy.await {
                Ok(value) => Ok(value),
                Err(proxy_error) => Err(preferred_route_error(direct_error, proxy_error)),
            },
        },
        result = &mut proxy => match result {
            Ok(value) => Ok(value),
            Err(proxy_error) => match direct.await {
                Ok(value) => Ok(value),
                Err(direct_error) => Err(preferred_route_error(direct_error, proxy_error)),
            },
        }
    }
}

fn preferred_route_error(direct: BodyError, proxy: BodyError) -> BodyError {
    // Preserve the direct HTTP/format error when the proxy also fails,
    // rather than hiding it behind an unavailable local SOCKS listener.
    if direct.kind == BodyErrorKind::Transport {
        proxy
    } else {
        direct
    }
}

async fn fetch_valid_subscription(
    client: &reqwest::Client,
    sub: &Subscription,
) -> Result<(String, Vec<Node>), BodyError> {
    let content = fetch_body(client, &sub.url).await?;
    let nodes = parse_subscription_content(&content, &sub.format).map_err(|error| BodyError {
        kind: BodyErrorKind::Permanent,
        error,
    })?;
    if nodes.is_empty() {
        return Err(BodyError {
            kind: BodyErrorKind::Permanent,
            error: anyhow::anyhow!(
                "Subscription contains no usable nodes; existing nodes were kept"
            ),
        });
    }
    Ok((content, nodes))
}

fn subscription_client(
    socks: Option<SocketAddr>,
    explicit_routes: bool,
) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(if socks.is_some() {
            PROXY_FETCH_TIMEOUT
        } else {
            DIRECT_FETCH_TIMEOUT
        })
        .user_agent("Sockrocket/0.1")
        .redirect(reqwest::redirect::Policy::limited(5));
    if explicit_routes {
        // Environment proxies must not turn the explicit direct route into
        // a second request to the same local proxy. Plain fetch callers keep
        // their existing environment proxy behavior.
        builder = builder.no_proxy();
    }
    if let Some(socks) = socks {
        let proxy = reqwest::Proxy::all(format!("socks5h://{socks}"))
            .context("Failed to configure subscription proxy")?;
        builder = builder.proxy(proxy);
    } else {
        let domestic_first = std::fs::read_to_string("/etc/resolv.conf")
            .is_ok_and(|text| crate::proxy::transport::nameservers_are_self_loop(&text));
        builder = builder.dns_resolver(Arc::new(SubscriptionResolver { domestic_first }));
    }
    builder.build().context("Failed to create HTTP client")
}

async fn fetch_body(client: &reqwest::Client, url: &str) -> Result<String, BodyError> {
    let mut response = match client.get(url).send().await {
        Ok(response) => response,
        Err(error) => {
            return Err(BodyError {
                kind: BodyErrorKind::Transport,
                error: anyhow::Error::from(error.without_url())
                    .context("Failed to fetch subscription"),
            });
        }
    };
    let status = response.status();
    if !status.is_success() {
        return Err(BodyError {
            kind: BodyErrorKind::Http,
            error: anyhow::anyhow!("Subscription returned HTTP {status}"),
        });
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_SUBSCRIPTION_BYTES as u64)
    {
        return Err(BodyError {
            kind: BodyErrorKind::Permanent,
            error: anyhow::anyhow!("Subscription exceeds the 8 MiB size limit"),
        });
    }
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if chunk.len() > MAX_SUBSCRIPTION_BYTES.saturating_sub(body.len()) {
                    return Err(BodyError {
                        kind: BodyErrorKind::Permanent,
                        error: anyhow::anyhow!("Subscription exceeds the 8 MiB size limit"),
                    });
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(error) => {
                return Err(BodyError {
                    kind: BodyErrorKind::Transport,
                    error: anyhow::Error::from(error.without_url())
                        .context("Failed to read subscription body"),
                });
            }
        }
    }
    String::from_utf8(body).map_err(|error| BodyError {
        kind: BodyErrorKind::Permanent,
        error: anyhow::Error::from(error).context("Subscription is not valid UTF-8"),
    })
}

struct SubscriptionResolver {
    domestic_first: bool,
}

impl reqwest::dns::Resolve for SubscriptionResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().trim_end_matches('.').to_owned();
        let domestic_first = self.domestic_first;
        Box::pin(async move {
            let ips = resolve_subscription_host(&host, domestic_first).await;
            if ips.is_empty() {
                return Err(Box::new(std::io::Error::other(
                    "subscription DNS returned no address",
                ))
                    as Box<dyn std::error::Error + Send + Sync>);
            }
            let addrs: reqwest::dns::Addrs =
                Box::new(ips.into_iter().map(|ip| SocketAddr::new(ip, 0)));
            Ok(addrs)
        })
    }
}

async fn resolve_subscription_host(host: &str, domestic_first: bool) -> Vec<IpAddr> {
    if let Ok(ip) = host.parse() {
        return subscription_addresses([ip]);
    }
    if domestic_first && let Some(ip) = domestic_a(host).await {
        return subscription_addresses([ip]);
    }
    let Ok(addrs) = tokio::net::lookup_host((host, 0u16)).await else {
        return Vec::new();
    };
    subscription_addresses(addrs.map(|addr| addr.ip()))
}

async fn domestic_a(host: &str) -> Option<IpAddr> {
    let ali = "223.5.5.5:53".parse().unwrap();
    let tencent = "119.29.29.29:53".parse().unwrap();
    first_subscription_address(
        crate::dns::lookup_a_udp(ali, host),
        crate::dns::lookup_a_udp(tencent, host),
    )
    .await
}

async fn first_subscription_address<F, S>(first: F, second: S) -> Option<IpAddr>
where
    F: Future<Output = Option<IpAddr>>,
    S: Future<Output = Option<IpAddr>>,
{
    let usable = |ip: Option<IpAddr>| subscription_addresses(ip).into_iter().next();
    tokio::pin!(first, second);
    tokio::select! {
        ip = &mut first => match usable(ip) {
            Some(ip) => Some(ip),
            None => usable(second.await),
        },
        ip = &mut second => match usable(ip) {
            Some(ip) => Some(ip),
            None => usable(first.await),
        }
    }
}

fn subscription_addresses(ips: impl IntoIterator<Item = IpAddr>) -> Vec<IpAddr> {
    let mut out = Vec::new();
    for ip in ips {
        let IpAddr::V4(v4) = ip else {
            continue;
        };
        if crate::dns::fakeip::is_fake_ip(v4) || v4.is_unspecified() {
            continue;
        }
        if !out.contains(&ip) {
            out.push(ip);
        }
    }
    out
}

fn subscription_entry_count(content: &str) -> usize {
    let count = |text: &str| {
        text.lines()
            .filter(|line| {
                let line = line.trim();
                !line.starts_with('#') && line.contains("://")
            })
            .count()
    };
    let clash_count = |text: &str| {
        serde_yaml::from_str::<serde_yaml::Value>(text)
            .ok()
            .and_then(|value| value.get("proxies")?.as_sequence().map(Vec::len))
            .unwrap_or(0)
    };
    let content = content.trim_start_matches('\u{feff}').trim();
    match detect_format(content) {
        SubFormat::PlainLines => count(content),
        SubFormat::Clash => clash_count(content),
        SubFormat::Base64Lines => {
            let clean: String = content
                .chars()
                .filter(|c| !c.is_ascii_whitespace())
                .collect();
            let decoded = general_purpose::STANDARD
                .decode(&clean)
                .or_else(|_| general_purpose::STANDARD_NO_PAD.decode(&clean))
                .or_else(|_| general_purpose::URL_SAFE.decode(&clean))
                .or_else(|_| general_purpose::URL_SAFE_NO_PAD.decode(&clean));
            decoded
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .map_or(0, |text| match detect_format(&text) {
                    SubFormat::PlainLines => count(&text),
                    SubFormat::Clash => clash_count(&text),
                    _ => 0,
                })
        }
        _ => 0,
    }
}

/// Parse subscription content with format detection
pub fn parse_subscription_content(content: &str, format_hint: &str) -> Result<Vec<Node>> {
    let content = content.trim_start_matches('\u{feff}').trim();
    if format_hint == "wireguard" || content.starts_with("[Interface]") {
        return super::wireguard::parse_profile(content).map(|node| vec![node]);
    }

    match format_hint {
        "clash" => return clash::parse_clash_config(content),
        "singbox" | "sing-box" => return singbox::parse_singbox_config(content),
        // UI / docs historically labeled base64 share-URI lists as "v2ray".
        // Real V2Ray JSON (`outbounds`) is still accepted via fallback or
        // the explicit `v2ray-json` hint.
        "v2ray" => {
            if let Ok(nodes) = parse_base64_lines(content)
                && !nodes.is_empty()
            {
                return Ok(nodes);
            }
            if let Ok(nodes) = parse_plain_lines(content)
                && !nodes.is_empty()
            {
                return Ok(nodes);
            }
            return v2ray::parse_v2ray_config(content);
        }
        "v2ray-json" => return v2ray::parse_v2ray_config(content),
        "base64" => return parse_base64_lines(content),
        _ => {} // "auto" or unknown → auto-detect
    }

    // Auto-detect format
    let format = detect_format(content);
    tracing::info!("Auto-detected subscription format: {:?}", format);

    match format {
        SubFormat::Clash => clash::parse_clash_config(content),
        SubFormat::SingBox => singbox::parse_singbox_config(content),
        SubFormat::V2Ray => v2ray::parse_v2ray_config(content),
        SubFormat::Base64Lines => parse_base64_lines(content),
        SubFormat::PlainLines => parse_plain_lines(content),
    }
}

/// Detect the format of subscription content
pub fn detect_format(content: &str) -> SubFormat {
    let trimmed = content.trim();

    // Check for Clash YAML
    if clash::is_clash_config(trimmed) {
        return SubFormat::Clash;
    }

    // Check for sing-box JSON (before generic V2Ray check)
    if trimmed.starts_with('{') && singbox::is_singbox_config(trimmed) {
        return SubFormat::SingBox;
    }

    // Check for V2Ray JSON
    if trimmed.starts_with('{') && v2ray::is_v2ray_config(trimmed) {
        return SubFormat::V2Ray;
    }

    // Check if content has proxy URI lines
    let first_line = trimmed.lines().next().unwrap_or("");
    if first_line.contains("://") {
        return SubFormat::PlainLines;
    }

    // Try Base64 decode
    if looks_like_base64(trimmed) {
        return SubFormat::Base64Lines;
    }

    // Default to plain lines
    SubFormat::PlainLines
}

fn looks_like_base64(content: &str) -> bool {
    let clean: String = content.chars().filter(|c| !c.is_whitespace()).collect();

    if clean.is_empty() {
        return false;
    }

    // Base64 characters only
    clean.chars().all(|c| {
        c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=' || c == '-' || c == '_'
    })
}

/// Decode Base64 content and parse each line as a proxy URI
fn parse_base64_lines(content: &str) -> Result<Vec<Node>> {
    // Strip ALL ASCII whitespace: subscriptions are frequently wrapped at
    // 76 chars (MIME style), and the STANDARD decoder rejects any `\n`.
    let clean: String = content
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect();

    // Try standard Base64 first, then URL-safe
    let decoded = general_purpose::STANDARD
        .decode(&clean)
        .or_else(|_| general_purpose::STANDARD_NO_PAD.decode(&clean))
        .or_else(|_| general_purpose::URL_SAFE.decode(&clean))
        .or_else(|_| general_purpose::URL_SAFE_NO_PAD.decode(&clean))
        .context("Failed to Base64 decode subscription content")?;

    let text = String::from_utf8(decoded).context("Invalid UTF-8 after Base64 decode")?;

    if text.trim_start().starts_with("[Interface]") {
        return super::wireguard::parse_profile(&text).map(|node| vec![node]);
    }
    // The decoded content might be Clash YAML, sing-box JSON, or plain URI lines
    if clash::is_clash_config(&text) {
        return clash::parse_clash_config(&text);
    }
    if singbox::is_singbox_config(&text) {
        return singbox::parse_singbox_config(&text);
    }

    parse_plain_lines(&text)
}

/// Parse plain text where each line is a proxy URI
fn parse_plain_lines(content: &str) -> Result<Vec<Node>> {
    let mut nodes = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        match v2ray::parse_proxy_uri(line) {
            Ok(node) => nodes.push(node),
            Err(_) => {
                // Proxy URIs carry credentials; parser errors can echo them
                // too. Neither belongs in diagnostic logs.
                tracing::debug!("Skipping an invalid subscription entry");
            }
        }
    }

    if nodes.is_empty() {
        anyhow::bail!("No valid proxy nodes found in subscription");
    }

    Ok(nodes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn subscription_dns_returns_the_first_usable_answer_in_either_order() {
        let ip: IpAddr = "192.0.2.1".parse().unwrap();
        for reverse in [false, true] {
            let first = async move {
                if reverse {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    None
                } else {
                    Some(ip)
                }
            };
            let second = async move {
                if reverse {
                    Some(ip)
                } else {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    None
                }
            };
            assert_eq!(
                tokio::time::timeout(
                    Duration::from_millis(100),
                    first_subscription_address(first, second)
                )
                .await
                .expect("healthy DNS waited for the slow resolver"),
                Some(ip)
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn subscription_dns_ignores_empty_fake_and_ipv6_answers_before_real_ipv4() {
        let expected: IpAddr = "192.0.2.1".parse().unwrap();
        for unusable in [
            None,
            Some("198.18.0.1".parse().unwrap()),
            Some("2001:db8::1".parse().unwrap()),
        ] {
            let result = first_subscription_address(std::future::ready(unusable), async {
                tokio::time::sleep(Duration::from_millis(20)).await;
                Some(expected)
            })
            .await;
            assert_eq!(result, Some(expected));
        }
        assert_eq!(
            first_subscription_address(std::future::ready(None), std::future::ready(None)).await,
            None
        );
    }

    fn route_error(kind: BodyErrorKind, message: &str) -> BodyError {
        BodyError {
            kind,
            error: anyhow::anyhow!("{message}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_direct_http_error_cannot_cancel_a_healthy_proxy() {
        for delay in [Duration::ZERO, Duration::from_millis(600)] {
            let result = first_successful_route(
                async {
                    tokio::time::sleep(delay).await;
                    Err(route_error(BodyErrorKind::Http, "HTTP 403"))
                },
                async {
                    tokio::time::sleep(Duration::from_millis(700)).await;
                    Ok("proxy")
                },
            )
            .await
            .unwrap();
            assert_eq!(result, "proxy");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_proxy_error_cannot_cancel_a_healthy_direct_route() {
        let result = first_successful_route(
            async {
                tokio::time::sleep(Duration::from_millis(800)).await;
                Ok("direct")
            },
            async { Err(route_error(BodyErrorKind::Transport, "proxy unavailable")) },
        )
        .await
        .unwrap();
        assert_eq!(result, "direct");
    }

    #[tokio::test(start_paused = true)]
    async fn both_failed_routes_preserve_the_direct_http_error_in_either_order() {
        for direct_first in [false, true] {
            let error = first_successful_route::<(), _, _>(
                async {
                    if !direct_first {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    Err(route_error(BodyErrorKind::Http, "HTTP 403"))
                },
                async {
                    if direct_first {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    Err(route_error(BodyErrorKind::Transport, "proxy unavailable"))
                },
            )
            .await
            .unwrap_err();
            assert_eq!(error.error.to_string(), "HTTP 403");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_route_can_succeed_after_exactly_three_retries() {
        let attempts = std::cell::Cell::new(0);
        let started = tokio::time::Instant::now();
        let result = retry_subscription_route(|| {
            let count = attempts.get() + 1;
            attempts.set(count);
            std::future::ready(if count == 4 {
                Ok("fourth attempt")
            } else {
                Err(route_error(BodyErrorKind::Http, "HTTP 503"))
            })
        })
        .await
        .unwrap();
        assert_eq!(result, "fourth attempt");
        assert_eq!(attempts.get(), 4);
        assert_eq!(started.elapsed(), Duration::from_millis(1400));
    }

    #[tokio::test(start_paused = true)]
    async fn failed_routes_stop_after_the_initial_attempt_and_three_retries() {
        for kind in [BodyErrorKind::Transport, BodyErrorKind::Http] {
            let attempts = std::cell::Cell::new(0);
            let error = retry_subscription_route::<(), _, _>(|| {
                attempts.set(attempts.get() + 1);
                std::future::ready(Err(route_error(kind, "fixture failure")))
            })
            .await
            .unwrap_err();
            assert_eq!(attempts.get(), 4);
            assert_eq!(error.kind, kind);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn invalid_content_is_not_downloaded_repeatedly() {
        let attempts = std::cell::Cell::new(0);
        let error = retry_subscription_route::<(), _, _>(|| {
            attempts.set(attempts.get() + 1);
            std::future::ready(Err(route_error(BodyErrorKind::Permanent, "size limit")))
        })
        .await
        .unwrap_err();
        assert_eq!(attempts.get(), 1);
        assert_eq!(error.error.to_string(), "size limit");
    }

    #[tokio::test(start_paused = true)]
    async fn success_cancels_the_other_routes_pending_retries() {
        let attempts = std::cell::Cell::new(0);
        let result = first_successful_route(
            retry_subscription_route(|| {
                attempts.set(attempts.get() + 1);
                std::future::ready(Err(route_error(BodyErrorKind::Http, "HTTP 403")))
            }),
            async {
                tokio::time::sleep(Duration::from_millis(350)).await;
                Ok("proxy")
            },
        )
        .await
        .unwrap();
        assert_eq!(result, "proxy");
        assert_eq!(attempts.get(), 2);
        tokio::time::advance(Duration::from_secs(60)).await;
        assert_eq!(attempts.get(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn the_first_success_cancels_the_other_route_in_either_order() {
        use std::sync::atomic::{AtomicBool, Ordering};
        struct Cancelled(Arc<AtomicBool>);
        impl Drop for Cancelled {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        for direct_wins in [false, true] {
            let cancelled = Arc::new(AtomicBool::new(false));
            let guard = Cancelled(cancelled.clone());
            let winner = async {
                tokio::time::sleep(Duration::from_millis(1)).await;
                Ok("winner")
            };
            let loser = async move {
                let _guard = guard;
                std::future::pending::<Result<&str, BodyError>>().await
            };
            let result = if direct_wins {
                first_successful_route(winner, loser).await
            } else {
                first_successful_route(loser, winner).await
            };
            assert_eq!(result.unwrap(), "winner");
            assert!(cancelled.load(Ordering::SeqCst));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn concurrent_retries_share_one_total_deadline() {
        let started = tokio::time::Instant::now();
        let attempts = std::cell::Cell::new(0);
        let route = || {
            retry_subscription_route(|| {
                attempts.set(attempts.get() + 1);
                async {
                    tokio::time::sleep(Duration::from_secs(12)).await;
                    Err::<(), _>(route_error(BodyErrorKind::Transport, "timeout"))
                }
            })
        };
        let error = fetch_before_deadline(first_successful_route(route(), route()))
            .await
            .unwrap_err();
        assert!(error.error.to_string().contains("timed out after 30s"));
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(SUB_FETCH_TIMEOUT_SECS)
        );
        let count = attempts.get();
        tokio::time::advance(Duration::from_secs(60)).await;
        assert_eq!(attempts.get(), count);
    }

    #[test]
    fn subscription_urls_require_http_host_and_do_not_echo_private_inputs() {
        for url in [
            "https://example.invalid/sub?token=synthetic",
            "http://127.0.0.1:1234/fixture",
            "HTTPS://example.invalid/sub",
        ] {
            assert!(validate_subscription_url(url).is_ok());
        }
        for url in [
            "",
            "https://",
            "not-a-url",
            "file:///synthetic-private-profile",
            "https://example.invalid/private token",
        ] {
            let error = validate_subscription_url(url).unwrap_err().to_string();
            assert!(
                !error.contains("synthetic-private-profile") && !error.contains("private token")
            );
        }
    }

    #[test]
    fn import_diagnostics_count_uri_and_clash_entries_without_config_urls() {
        let lines = "# comment\nss://example.invalid\nunsupported://example.invalid\n";
        assert_eq!(subscription_entry_count(lines), 2);
        assert_eq!(
            subscription_entry_count(&general_purpose::STANDARD.encode(lines)),
            2
        );
        let clash = "proxies:\n  - {name: good, type: ss, server: good.invalid, port: 8388, cipher: aes-128-gcm, password: synthetic}\n  - {name: bad, type: unsupported, server: bad.invalid, port: 443}\n";
        assert_eq!(subscription_entry_count(clash), 2);
        assert_eq!(parse_subscription_content(clash, "auto").unwrap().len(), 1);
        assert_eq!(
            subscription_entry_count(&general_purpose::STANDARD.encode(clash)),
            2
        );
        assert_eq!(
            subscription_entry_count("proxies: []\nurl: https://example.invalid"),
            0
        );
    }

    #[test]
    fn test_detect_clash_format() {
        let content = "proxies:\n  - name: test\n    type: ss";
        assert_eq!(detect_format(content), SubFormat::Clash);
    }

    #[test]
    fn test_detect_v2ray_format() {
        let content = r#"{"outbounds": []}"#;
        assert_eq!(detect_format(content), SubFormat::V2Ray);
    }

    #[test]
    fn test_detect_plain_lines() {
        let content = "ss://abc@1.2.3.4:8388#test\nvmess://xyz";
        assert_eq!(detect_format(content), SubFormat::PlainLines);
    }

    #[test]
    fn test_detect_base64() {
        let plain = "ss://YWVzLTI1Ni1nY206cGFzcw==@1.2.3.4:8388#test\n";
        let encoded = general_purpose::STANDARD.encode(plain);
        assert_eq!(detect_format(&encoded), SubFormat::Base64Lines);
    }

    #[test]
    fn auto_detects_url_safe_base64() {
        let content = "ss://YWVzLTI1Ni1nY206cGFzc3dvcmQ=@192.0.2.1:8388#示例节点🿿";
        for encoded in [
            general_purpose::URL_SAFE.encode(content),
            general_purpose::URL_SAFE_NO_PAD.encode(content),
        ] {
            assert!(encoded.contains('-') || encoded.contains('_'));
            assert_eq!(detect_format(&encoded), SubFormat::Base64Lines);
            let nodes = parse_subscription_content(&encoded, "auto").unwrap();
            assert_eq!(nodes.len(), 1);
        }
    }

    #[test]
    fn test_parse_base64_subscription() {
        use base64::engine::general_purpose;

        let lines = "ss://YWVzLTI1Ni1nY206cGFzc3dvcmQ=@1.2.3.4:8388#SS-1\nss://YWVzLTI1Ni1nY206cGFzc3dvcmQ=@5.6.7.8:8388#SS-2\n";
        let encoded = general_purpose::STANDARD.encode(lines);

        let nodes = parse_subscription_content(&encoded, "auto").unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].name, "SS-1");
        assert_eq!(nodes[1].name, "SS-2");
    }

    #[test]
    fn test_v2ray_hint_accepts_base64_uri_list() {
        use base64::engine::general_purpose;

        // Selecting format=v2ray in the UI must not force V2Ray JSON-only.
        let lines = "ss://YWVzLTI1Ni1nY206cGFzc3dvcmQ=@1.2.3.4:8388#SS-1\nss://YWVzLTI1Ni1nY206cGFzc3dvcmQ=@5.6.7.8:8388#SS-2\n";
        let encoded = general_purpose::STANDARD.encode(lines);
        let nodes = parse_subscription_content(&encoded, "v2ray").unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].name, "SS-1");
    }

    #[test]
    fn test_parse_base64_subscription_with_line_wrapping() {
        use base64::engine::general_purpose;

        // Some servers wrap the base64 blob at 76 chars (MIME style) or
        // emit it with trailing/embedded newlines; decoding must succeed.
        let lines = "ss://YWVzLTI1Ni1nY206cGFzc3dvcmQ=@1.2.3.4:8388#SS-1\nss://YWVzLTI1Ni1nY206cGFzc3dvcmQ=@5.6.7.8:8388#SS-2\n";
        let encoded = general_purpose::STANDARD.encode(lines);
        let wrapped: String = encoded
            .as_bytes()
            .chunks(76)
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect::<Vec<_>>()
            .join("\r\n");

        let nodes = parse_subscription_content(&wrapped, "base64").unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].name, "SS-1");
        assert_eq!(nodes[1].name, "SS-2");
    }

    #[test]
    fn subscription_addresses_keep_real_ipv4_only() {
        let ips = subscription_addresses([
            IpAddr::V4(Ipv4Addr::new(198, 18, 0, 8)),
            IpAddr::V6("2001:db8::1".parse().unwrap()),
            IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)),
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
        ]);
        assert_eq!(
            ips,
            vec![
                IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)),
                IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            ]
        );
    }

    #[test]
    fn test_parse_plain_lines() {
        let content = "ss://YWVzLTI1Ni1nY206cGFzc3dvcmQ=@1.2.3.4:8388#SS-1\n# comment\n\nss://YWVzLTI1Ni1nY206cGFzc3dvcmQ=@5.6.7.8:8388#SS-2\n";
        let nodes = parse_subscription_content(content, "auto").unwrap();
        assert_eq!(nodes.len(), 2);
    }
}

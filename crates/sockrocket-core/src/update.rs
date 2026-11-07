//! Check GitHub Releases for a newer Sockrocket version.
//!
//! Desktop GUI and Merlin CGI share this helper. Download/replace is left to
//! the caller (open browser asset URL on desktop; binary swap on Merlin).

use anyhow::{Context, Result, bail};
use serde::Deserialize;

const RELEASES_LATEST: &str = "https://api.github.com/repos/sockrockets/sockrocket/releases/latest";
const RELEASES_PAGE: &str = "https://github.com/sockrockets/sockrocket/releases/latest";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateAvailability {
    /// Running version matches (or exceeds) the latest release tag.
    UpToDate,
    /// A newer release is available.
    Available,
}

#[derive(Debug, Clone)]
pub struct UpdateCheck {
    pub current: String,
    pub latest: String,
    pub availability: UpdateAvailability,
    /// Release page URL (always set on success).
    pub html_url: String,
    /// Direct download URL for a preferred asset name, if found.
    pub asset_url: Option<String>,
    pub asset_name: Option<String>,
    pub asset_size: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: Option<u64>,
}

/// Guess the desktop GUI asset name for the current host (best-effort).
pub fn preferred_gui_asset() -> Option<&'static str> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    match (os, arch) {
        ("macos", "aarch64") => Some("sockrocket-macos-aarch64.dmg"),
        ("macos", "x86_64") => Some("sockrocket-macos-x86_64.dmg"),
        ("linux", "x86_64") => Some("sockrocket-linux-x86_64.tar.gz"),
        ("linux", "aarch64") => Some("sockrocket-linux-aarch64.tar.gz"),
        ("windows", "x86_64") => Some("sockrocket-windows-x86_64.zip"),
        _ => None,
    }
}

/// Guess the Merlin / musl CLI asset for this router arch.
pub fn preferred_merlin_cli_asset() -> Option<&'static str> {
    let arch = std::env::consts::ARCH;
    match arch {
        "aarch64" => Some("sockrocket-cli-linux-aarch64"),
        "arm" | "armv7" => Some(if cfg!(target_abi = "eabihf") {
            "sockrocket-cli-linux-armv7"
        } else {
            "sockrocket-cli-linux-armv7sf"
        }),
        _ => None,
    }
}

/// Merlin offline package name for a koolcenter platform tag.
pub fn merlin_package_asset(platform: &str) -> Option<String> {
    let p = match platform.trim() {
        "arm386" => "arm",
        other => other,
    };
    match p {
        "arm" | "hnd" | "hnd_v8" | "qca" | "mtk" | "ipq32" | "ipq64" => {
            Some(format!("sockrocket-merlin-{p}.tar.gz"))
        }
        _ => None,
    }
}

/// Compare dotted numeric versions (`1.2.3`). Non-numeric suffixes are ignored
/// after the first non-digit segment. Returns `Ordering` of `a` vs `b`.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |s: &str| -> Vec<u64> {
        s.trim()
            .trim_start_matches('v')
            .trim_start_matches('V')
            .split(|c: char| !c.is_ascii_digit())
            .filter(|p| !p.is_empty())
            .filter_map(|p| p.parse().ok())
            .collect()
    };
    let va = parse(a);
    let vb = parse(b);
    let n = va.len().max(vb.len());
    for i in 0..n {
        let x = va.get(i).copied().unwrap_or(0);
        let y = vb.get(i).copied().unwrap_or(0);
        match x.cmp(&y) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}

/// The public release page remains usable when GitHub limits REST requests.
async fn merlin_web_release(
    client: &reqwest::Client,
    releases: &str,
    asset: &str,
) -> Result<GhRelease> {
    let page = client
        .head(format!("{releases}/latest"))
        .send()
        .await
        .map_err(reqwest::Error::without_url)?
        .error_for_status()
        .map_err(reqwest::Error::without_url)?;
    let html_url = page.url().as_str();
    let tag = html_url
        .strip_prefix(&format!("{releases}/tag/"))
        .context("GitHub did not identify the latest release")?;
    let version = tag.strip_prefix('v').unwrap_or(tag);
    anyhow::ensure!(
        version.split('.').count() == 3
            && version
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())),
        "Invalid GitHub release version"
    );
    let url = format!("{releases}/download/{tag}/{asset}");
    let package = client
        .head(&url)
        .send()
        .await
        .map_err(reqwest::Error::without_url)
        .context("Cannot check release package")?;
    let assets = if package.status() == reqwest::StatusCode::NOT_FOUND {
        Vec::new()
    } else {
        anyhow::ensure!(
            package.status().is_success(),
            "Release package returned HTTP {}",
            package.status()
        );
        let size = package
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok()?.parse::<u64>().ok())
            .filter(|size| *size > 0)
            .context("Release package size unavailable")?;
        vec![GhAsset {
            name: asset.into(),
            browser_download_url: url,
            size: Some(size),
        }]
    };
    Ok(GhRelease {
        tag_name: tag.to_owned(),
        html_url: html_url.to_owned(),
        assets,
    })
}

async fn latest_release(
    client: &reqwest::Client,
    api: &str,
    releases: &str,
    prefer_asset: Option<&str>,
) -> Result<GhRelease> {
    let response = client
        .get(api)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .context("Failed to contact GitHub Releases")?;

    let status = response.status();
    if matches!(
        status,
        reqwest::StatusCode::FORBIDDEN | reqwest::StatusCode::TOO_MANY_REQUESTS
    ) && let Some(asset) = prefer_asset.filter(|name| {
        name.strip_prefix("sockrocket-merlin-")
            .and_then(|s| s.strip_suffix(".tar.gz"))
            .and_then(merlin_package_asset)
            .as_deref()
            == Some(*name)
    }) {
        merlin_web_release(client, releases, asset)
            .await
            .map_err(|error| {
                anyhow::anyhow!(
                    "GitHub API returned HTTP {status}; official release lookup failed: {error}"
                )
            })
    } else {
        if !status.is_success() {
            bail!("GitHub Releases returned HTTP {status}");
        }
        response
            .json()
            .await
            .context("Failed to parse GitHub release JSON")
    }
}

/// Query GitHub for the latest release and compare to `current` (e.g. `0.1.0`).
///
/// `prefer_asset` is an exact asset file name to resolve a direct download URL.
pub async fn check_for_update(current: &str, prefer_asset: Option<&str>) -> Result<UpdateCheck> {
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(20))
        .user_agent(format!(
            "Sockrocket/{} (+https://github.com/sockrockets/sockrocket)",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .context("Failed to create HTTP client")?;

    let release = latest_release(
        &client,
        RELEASES_LATEST,
        "https://github.com/sockrockets/sockrocket/releases",
        prefer_asset,
    )
    .await?;

    let latest = release
        .tag_name
        .trim()
        .trim_start_matches('v')
        .trim_start_matches('V')
        .to_string();
    let current_norm = current
        .trim()
        .trim_start_matches('v')
        .trim_start_matches('V')
        .to_string();

    let availability = match compare_versions(&current_norm, &latest) {
        std::cmp::Ordering::Less => UpdateAvailability::Available,
        _ => UpdateAvailability::UpToDate,
    };

    let mut asset_url = None;
    let mut asset_name = None;
    let mut asset_size = None;
    if let Some(want) = prefer_asset
        && let Some(a) = release.assets.iter().find(|a| a.name == want)
    {
        asset_url = Some(a.browser_download_url.clone());
        asset_name = Some(a.name.clone());
        asset_size = a.size;
    }

    let html_url = if release.html_url.is_empty() {
        RELEASES_PAGE.to_string()
    } else {
        release.html_url
    };

    Ok(UpdateCheck {
        current: current_norm,
        latest,
        availability,
        html_url,
        asset_url,
        asset_name,
        asset_size,
    })
}

/// Download `url` to `dest` (overwrites).
pub async fn download_file(url: &str, dest: &std::path::Path) -> Result<()> {
    download_file_with_proxy_fallback(url, dest, None).await
}

/// Retry failed network downloads through the caller's local SOCKS listener.
/// The proxy resolves hostnames; TLS validation remains enabled.
pub async fn download_file_with_proxy_fallback(
    url: &str,
    dest: &std::path::Path,
    socks: Option<std::net::SocketAddr>,
) -> Result<()> {
    let bytes = match download_bytes(url, None).await {
        Ok(bytes) => bytes,
        Err(direct) => match socks {
            Some(addr) => download_bytes(url, Some(addr)).await.map_err(|proxy| {
                anyhow::anyhow!("Direct download failed: {direct:#}; SOCKS retry failed: {proxy:#}")
            })?,
            None => return Err(direct),
        },
    };
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).context("Failed to create download directory")?;
    }
    std::fs::write(dest, &bytes).context("Failed to write downloaded package")?;
    Ok(())
}

async fn download_bytes(url: &str, socks: Option<std::net::SocketAddr>) -> Result<bytes::Bytes> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(300))
        .user_agent(format!(
            "Sockrocket/{} (+https://github.com/sockrockets/sockrocket)",
            env!("CARGO_PKG_VERSION")
        ));
    if let Some(addr) = socks {
        builder = builder.no_proxy().proxy(
            reqwest::Proxy::all(format!("socks5h://{addr}"))
                .context("Failed to configure update proxy")?,
        );
    }
    let client = builder.build().context("Failed to create HTTP client")?;

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| error.without_url())
        .context("Download request failed")?;
    if !response.status().is_success() {
        bail!("Download returned HTTP {}", response.status());
    }
    response
        .bytes()
        .await
        .map_err(|error| error.without_url())
        .context("Failed to read download body")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn limited_api_resolves_pinned_merlin_package() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let asset = "sockrocket-merlin-hnd.tar.gz";
        for status in ["403 Forbidden", "429 Too Many Requests"] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let root = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                for (expected, reply) in [
                    (
                        "GET /api".to_owned(),
                        format!("{status}\r\nContent-Length: 0"),
                    ),
                    (
                        "HEAD /latest".into(),
                        "302 Found\r\nLocation: /tag/v1.2.3".into(),
                    ),
                    ("HEAD /tag/v1.2.3".into(), "200 OK".into()),
                    (
                        format!("HEAD /download/v1.2.3/{asset}"),
                        "200 OK\r\nContent-Length: 12345".into(),
                    ),
                ] {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut lines = BufReader::new(&mut stream).lines();
                    assert_eq!(
                        lines.next_line().await.unwrap().unwrap(),
                        format!("{expected} HTTP/1.1")
                    );
                    while !lines.next_line().await.unwrap().unwrap().is_empty() {}
                    stream
                        .write_all(
                            format!("HTTP/1.1 {reply}\r\nConnection: close\r\n\r\n").as_bytes(),
                        )
                        .await
                        .unwrap();
                }
            });
            let client = reqwest::Client::builder()
                .no_proxy()
                .timeout(std::time::Duration::from_secs(3))
                .build()
                .unwrap();
            let release = latest_release(&client, &format!("{root}/api"), &root, Some(asset))
                .await
                .unwrap();
            assert_eq!(release.tag_name, "v1.2.3");
            assert_eq!(release.assets[0].size, Some(12345));
            assert_eq!(
                release.assets[0].browser_download_url,
                format!("{root}/download/v1.2.3/{asset}")
            );
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn successful_download_does_not_use_fallback() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let direct = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/package", direct.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut stream, _) = direct.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(stream.read_u8().await.unwrap());
                assert!(request.len() < 8192);
            }
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\npackage",
                )
                .await
                .unwrap();
        });
        let path = std::env::temp_dir().join(format!("sockrocket-update-{}", uuid::Uuid::new_v4()));
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            download_file_with_proxy_fallback(&url, &path, Some(proxy.local_addr().unwrap())),
        )
        .await
        .unwrap()
        .unwrap();
        server.await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"package");
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), proxy.accept())
                .await
                .is_err()
        );
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn failed_download_retries_socks_with_remote_dns() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut greeting = [0; 2];
            stream.read_exact(&mut greeting).await.unwrap();
            assert_eq!(greeting[0], 5);
            let mut methods = vec![0; greeting[1] as usize];
            stream.read_exact(&mut methods).await.unwrap();
            stream.write_all(&[5, 0]).await.unwrap();
            let mut header = [0; 5];
            stream.read_exact(&mut header).await.unwrap();
            assert_eq!(&header[..4], &[5, 1, 0, 3]);
            let mut hostname = vec![0; header[4] as usize];
            stream.read_exact(&mut hostname).await.unwrap();
            assert_eq!(hostname, b"sockrocket-update-test.invalid");
            let mut port = [0; 2];
            stream.read_exact(&mut port).await.unwrap();
            stream
                .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 80])
                .await
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(stream.read_u8().await.unwrap());
                assert!(request.len() < 8192);
            }
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\npackage",
                )
                .await
                .unwrap();
        });
        let path = std::env::temp_dir().join(format!("sockrocket-update-{}", uuid::Uuid::new_v4()));
        tokio::time::timeout(std::time::Duration::from_secs(25), async {
            download_file_with_proxy_fallback(
                "http://sockrocket-update-test.invalid/package",
                &path,
                Some(addr),
            )
            .await
            .unwrap();
            server.await.unwrap();
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"package");
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn failed_download_preserves_existing_file() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let path = std::env::temp_dir().join(format!("sockrocket-update-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, b"existing").unwrap();
        let error = download_file_with_proxy_fallback(
            &format!("http://{addr}/package?token=private-test-value"),
            &path,
            Some(addr),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("SOCKS retry failed"));
        assert!(!format!("{error:#}").contains("private-test-value"));
        assert_eq!(std::fs::read(&path).unwrap(), b"existing");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn compare_versions_basic() {
        use std::cmp::Ordering::*;
        assert_eq!(compare_versions("0.1.0", "0.1.0"), Equal);
        assert_eq!(compare_versions("v0.1.0", "0.1.1"), Less);
        assert_eq!(compare_versions("0.2.0", "0.1.9"), Greater);
        assert_eq!(compare_versions("1.0", "1.0.0"), Equal);
    }

    #[test]
    fn merlin_package_names() {
        assert_eq!(
            merlin_package_asset("hnd_v8").as_deref(),
            Some("sockrocket-merlin-hnd_v8.tar.gz")
        );
        assert_eq!(
            merlin_package_asset("arm386\n"),
            merlin_package_asset("arm")
        );
        assert!(merlin_package_asset("nope").is_none());
    }
}

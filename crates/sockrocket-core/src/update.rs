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
        "arm" | "armv7" => Some("sockrocket-cli-linux-armv7"),
        _ => None,
    }
}

/// Merlin offline package name for a koolcenter platform tag.
pub fn merlin_package_asset(platform: &str) -> Option<String> {
    let p = platform.trim();
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

    let response = client
        .get(RELEASES_LATEST)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .context("Failed to contact GitHub Releases")?;

    let status = response.status();
    if !status.is_success() {
        bail!("GitHub Releases returned HTTP {status}");
    }

    let release: GhRelease = response
        .json()
        .await
        .context("Failed to parse GitHub release JSON")?;

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
    if let Some(want) = prefer_asset
        && let Some(a) = release.assets.iter().find(|a| a.name == want)
    {
        asset_url = Some(a.browser_download_url.clone());
        asset_name = Some(a.name.clone());
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
    })
}

/// Download `url` to `dest` (overwrites).
pub async fn download_file(url: &str, dest: &std::path::Path) -> Result<()> {
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(300))
        .user_agent(format!(
            "Sockrocket/{} (+https://github.com/sockrockets/sockrocket)",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .context("Failed to create HTTP client")?;

    let response = client
        .get(url)
        .send()
        .await
        .context("Download request failed")?;
    if !response.status().is_success() {
        bail!("Download returned HTTP {}", response.status());
    }
    let bytes = response
        .bytes()
        .await
        .context("Failed to read download body")?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(dest, &bytes).with_context(|| format!("Failed to write {}", dest.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(merlin_package_asset("nope").is_none());
    }
}

use anyhow::{Context, Result, bail};

pub const DEFAULT_BYPASS: &str = "localhost,127.0.0.1,::1";

/// Platform-appropriate bypass list for system proxy settings.
/// Windows uses semicolons and `<local>` keyword.
#[cfg(target_os = "windows")]
pub const PLATFORM_BYPASS: &str = "localhost;127.0.0.1;::1;<local>";
#[cfg(not(target_os = "windows"))]
pub const PLATFORM_BYPASS: &str = "localhost,127.0.0.1,::1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemProxyConfig {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub bypass: String,
}

impl SystemProxyConfig {
    fn disabled() -> Self {
        Self {
            enabled: false,
            host: String::new(),
            port: 0,
            bypass: String::new(),
        }
    }
}

impl From<sysproxy::Sysproxy> for SystemProxyConfig {
    fn from(value: sysproxy::Sysproxy) -> Self {
        Self {
            enabled: value.enable,
            host: value.host,
            port: value.port,
            bypass: value.bypass,
        }
    }
}

pub fn system_proxy_supported() -> bool {
    sysproxy::Sysproxy::is_support()
}

pub fn get_system_proxy() -> Result<SystemProxyConfig> {
    ensure_supported()?;
    match sysproxy::Sysproxy::get_system_proxy() {
        Ok(proxy) => Ok(proxy.into()),
        Err(sysproxy::Error::ParseStr) => {
            // sysproxy only understands a bare "IP:port" ProxyServer value.
            // Windows commonly stores other formats instead — a per-protocol
            // list ("http=host:port;https=host:port"), a hostname
            // ("localhost:8080"), or an empty/stale value. None of these are
            // permission problems; read the registry ourselves and parse it
            // tolerantly.
            #[cfg(target_os = "windows")]
            {
                windows::get_system_proxy_tolerant()
            }
            #[cfg(not(target_os = "windows"))]
            {
                tracing::warn!(
                    "get_system_proxy: unparseable system proxy value, treating as disabled"
                );
                Ok(SystemProxyConfig::disabled())
            }
        }
        Err(sysproxy::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            // ProxyEnable / ProxyServer may simply not exist yet (fresh
            // system, or proxy never configured). That means "disabled",
            // not an error.
            Ok(SystemProxyConfig::disabled())
        }
        Err(e) => Err(anyhow::anyhow!("failed to read system proxy: {}", e)),
    }
}

pub fn set_system_proxy(host: &str, port: u16) -> Result<()> {
    set_system_proxy_with_bypass(host, port, PLATFORM_BYPASS)
}

pub fn set_system_proxy_with_bypass(host: &str, port: u16, bypass: &str) -> Result<()> {
    ensure_supported()?;

    let host = host.trim();
    if host.is_empty() {
        bail!("system proxy host cannot be empty");
    }
    // A wildcard listen address is not a usable proxy destination — clients
    // cannot connect to 0.0.0.0 / ::. Point them at loopback instead.
    let host = if host == "0.0.0.0" || host == "::" {
        "127.0.0.1"
    } else {
        host
    };

    // WinINet prefers a configured PAC URL ("AutoConfigURL") over the manual
    // proxy. Clear any leftover PAC URL first, otherwise the manual proxy
    // written below is silently ignored by browsers.
    #[cfg(target_os = "windows")]
    windows::clear_pac_url();

    // WinINet stores the proxy as "host:port" (sysproxy's Windows set path
    // does `format!("{}:{}", host, port)`), so an IPv6 literal must be
    // bracketed or the registry value becomes unparsable. Other platforms
    // pass the host as a separate argument and expect it bare.
    #[cfg(target_os = "windows")]
    let host = windows::bracket_ipv6_host(host);

    let proxy = sysproxy::Sysproxy {
        enable: true,
        host: host.to_string(),
        port,
        bypass: bypass.to_string(),
    };

    // NOTE: sysproxy's Windows set path only writes registry values; the
    // "failed to parse string" error can only come from *reading* the proxy
    // (handled in get_system_proxy above), never from setting it. Errors
    // here are registry write failures, so only permission-related ones get
    // the Administrator hint.
    proxy.set_system_proxy().map_err(|e| {
        let msg = format!("{}", e);
        if msg.contains("denied") || msg.contains("permission") || msg.contains("access") {
            anyhow::anyhow!(
                "failed to set system proxy ({}:{}) — {}\nTry running as Administrator.",
                host,
                port,
                e
            )
        } else {
            anyhow::anyhow!("failed to set system proxy ({}:{}) — {}", host, port, e)
        }
    })
}

pub fn clear_system_proxy() -> Result<()> {
    ensure_supported()?;

    sysproxy::Sysproxy {
        enable: false,
        host: String::new(),
        port: 0,
        bypass: String::new(),
    }
    .set_system_proxy()
    .context("failed to clear system proxy")
}

fn ensure_supported() -> Result<()> {
    if system_proxy_supported() {
        Ok(())
    } else {
        bail!("system proxy is not supported on this platform")
    }
}

/// Windows helpers: tolerant registry reading and PAC cleanup.
///
/// The sysproxy crate (0.3.0) parses `ProxyServer` with
/// `SocketAddr::from_str`, which rejects every format except a bare
/// `IP:port` literal and reports "failed to parse string". Windows itself
/// and other proxy clients routinely store per-protocol or hostname values,
/// so we read and parse the registry directly instead.
#[cfg(target_os = "windows")]
mod windows {
    use super::SystemProxyConfig;
    use anyhow::{Context, Result};
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};

    const SUB_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Internet Settings";

    pub fn get_system_proxy_tolerant() -> Result<SystemProxyConfig> {
        let key = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(SUB_KEY, KEY_READ)
            .context("failed to open Internet Settings registry key")?;

        let enabled = key.get_value::<u32, _>("ProxyEnable").unwrap_or(0) == 1;
        let server: String = key.get_value("ProxyServer").unwrap_or_default();
        let bypass: String = key.get_value("ProxyOverride").unwrap_or_default();

        let (host, port) = parse_proxy_server(&server).unwrap_or_default();
        if enabled && host.is_empty() {
            tracing::warn!(
                "system proxy is enabled but ProxyServer {:?} could not be parsed",
                server
            );
        }
        Ok(SystemProxyConfig {
            enabled,
            host,
            port,
            bypass,
        })
    }

    /// Parse a Windows `ProxyServer` value into (host, port).
    ///
    /// Accepted forms:
    /// - `"host:port"` (IP literal or hostname, IPv6 in brackets)
    /// - per-protocol `"http=host:port;https=host:port;..."` — the https
    ///   entry is preferred, then http, then the first entry
    fn parse_proxy_server(server: &str) -> Option<(String, u16)> {
        let server = server.trim();
        if server.is_empty() {
            return None;
        }

        if server.contains('=') {
            let entry = |scheme: &str| {
                server.split(';').find_map(|part| {
                    let (key, value) = part.split_once('=')?;
                    key.trim()
                        .eq_ignore_ascii_case(scheme)
                        .then(|| value.trim())
                })
            };
            let value = entry("https").or_else(|| entry("http")).or_else(|| {
                server
                    .split(';')
                    .find_map(|part| part.split_once('=').map(|(_, value)| value.trim()))
            })?;
            return parse_host_port(value);
        }

        parse_host_port(server)
    }

    fn parse_host_port(value: &str) -> Option<(String, u16)> {
        let (host, port) = value.rsplit_once(':')?;
        let host = host.trim().trim_matches(|c| c == '[' || c == ']');
        let port: u16 = port.trim().parse().ok()?;
        if host.is_empty() {
            return None;
        }
        Some((host.to_string(), port))
    }

    /// Bracket an IPv6 proxy host so the `host:port` ProxyServer value stays
    /// parsable; IPv4 literals and hostnames pass through unchanged.
    pub fn bracket_ipv6_host(host: &str) -> std::borrow::Cow<'_, str> {
        if host.parse::<std::net::Ipv6Addr>().is_ok() {
            std::borrow::Cow::from(format!("[{}]", host))
        } else {
            std::borrow::Cow::from(host)
        }
    }

    /// Remove any PAC ("AutoConfigURL") value so the manual proxy actually
    /// takes effect. Best-effort: a missing value is fine, other failures
    /// are only logged.
    pub fn clear_pac_url() {
        let Ok(key) =
            RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(SUB_KEY, KEY_SET_VALUE)
        else {
            return;
        };
        match key.delete_value("AutoConfigURL") {
            Ok(()) => {
                tracing::info!("cleared AutoConfigURL (PAC) so the manual proxy takes effect")
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!("failed to clear AutoConfigURL (PAC): {}", e),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_plain_ip_host_port() {
            assert_eq!(
                parse_proxy_server("127.0.0.1:7890"),
                Some(("127.0.0.1".to_string(), 7890))
            );
        }

        #[test]
        fn parses_hostname_host_port() {
            assert_eq!(
                parse_proxy_server("localhost:8080"),
                Some(("localhost".to_string(), 8080))
            );
        }

        #[test]
        fn parses_bracketed_ipv6() {
            assert_eq!(
                parse_proxy_server("[::1]:8080"),
                Some(("::1".to_string(), 8080))
            );
        }

        #[test]
        fn parses_per_protocol_and_prefers_https() {
            assert_eq!(
                parse_proxy_server("http=127.0.0.1:8888;https=127.0.0.1:8889;socks=127.0.0.1:1080"),
                Some(("127.0.0.1".to_string(), 8889))
            );
        }

        #[test]
        fn brackets_ipv6_host_only() {
            assert_eq!(bracket_ipv6_host("::1"), "[::1]");
            assert_eq!(bracket_ipv6_host("2001:db8::1"), "[2001:db8::1]");
            assert_eq!(bracket_ipv6_host("127.0.0.1"), "127.0.0.1");
            assert_eq!(bracket_ipv6_host("localhost"), "localhost");
        }

        #[test]
        fn rejects_empty_and_garbage() {
            assert_eq!(parse_proxy_server(""), None);
            assert_eq!(parse_proxy_server(":0"), None);
            assert_eq!(parse_proxy_server("no-port"), None);
            assert_eq!(parse_proxy_server("host:abc"), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_bypass_has_local_targets() {
        assert!(DEFAULT_BYPASS.contains("localhost"));
        assert!(DEFAULT_BYPASS.contains("127.0.0.1"));
        assert!(DEFAULT_BYPASS.contains("::1"));
    }

    #[test]
    fn sysproxy_conversion_preserves_fields() {
        let raw = sysproxy::Sysproxy {
            enable: true,
            host: "127.0.0.1".into(),
            port: 1080,
            bypass: "localhost".into(),
        };

        let proxy: SystemProxyConfig = raw.into();
        assert!(proxy.enabled);
        assert_eq!(proxy.host, "127.0.0.1");
        assert_eq!(proxy.port, 1080);
        assert_eq!(proxy.bypass, "localhost");
    }
}

//! Persisted DNS policy. Endpoint URLs never appear in validation errors.
use std::net::{IpAddr, SocketAddr};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use crate::dns::{DnsConfig, DnsGroup, DnsServer};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AddressStrategy {
    #[default]
    PreferIpv4,
    PreferIpv6,
    Ipv4Only,
    Ipv6Only,
}

/// Merlin policy; desktop routing retains its own platform capability checks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ipv6Mode {
    #[default]
    Compatibility,
    DualStack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, try_from = "DnsSettingsInput")]
pub struct DnsSettings {
    pub direct_servers: Vec<String>,
    pub proxy_servers: Vec<String>,
    pub bootstrap_servers: Vec<SocketAddr>,
    pub direct_domains: Vec<String>,
    pub strategy: AddressStrategy,
    pub cache_enabled: bool,
    pub cache_capacity: usize,
    pub cache_ttl: u32,
    pub timeout_secs: u64,
}

// Separate input avoids accepting a syntactically valid but unusable policy.
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct DnsSettingsInput {
    direct_servers: Vec<String>,
    proxy_servers: Vec<String>,
    bootstrap_servers: Vec<SocketAddr>,
    direct_domains: Vec<String>,
    strategy: AddressStrategy,
    cache_enabled: bool,
    cache_capacity: usize,
    cache_ttl: u32,
    timeout_secs: u64,
}

impl Default for DnsSettings {
    fn default() -> Self {
        Self {
            direct_servers: vec!["udp://223.5.5.5:53".into(), "udp://119.29.29.29:53".into()],
            proxy_servers: vec![
                "tls://8.8.8.8:853?server_name=dns.google".into(),
                "tls://1.1.1.1:853?server_name=cloudflare-dns.com".into(),
            ],
            bootstrap_servers: vec![
                "223.5.5.5:53".parse().unwrap(),
                "119.29.29.29:53".parse().unwrap(),
            ],
            direct_domains: Vec::new(),
            strategy: AddressStrategy::default(),
            cache_enabled: true,
            cache_capacity: 1024,
            cache_ttl: 0,
            timeout_secs: 5,
        }
    }
}
impl Default for DnsSettingsInput {
    fn default() -> Self {
        let s = DnsSettings::default();
        Self {
            direct_servers: s.direct_servers,
            proxy_servers: s.proxy_servers,
            bootstrap_servers: s.bootstrap_servers,
            direct_domains: s.direct_domains,
            strategy: s.strategy,
            cache_enabled: s.cache_enabled,
            cache_capacity: s.cache_capacity,
            cache_ttl: s.cache_ttl,
            timeout_secs: s.timeout_secs,
        }
    }
}
impl TryFrom<DnsSettingsInput> for DnsSettings {
    type Error = String;
    fn try_from(s: DnsSettingsInput) -> std::result::Result<Self, String> {
        let s = Self {
            direct_servers: s.direct_servers,
            proxy_servers: s.proxy_servers,
            bootstrap_servers: s.bootstrap_servers,
            direct_domains: s.direct_domains,
            strategy: s.strategy,
            cache_enabled: s.cache_enabled,
            cache_capacity: s.cache_capacity,
            cache_ttl: s.cache_ttl,
            timeout_secs: s.timeout_secs,
        };
        s.validate().map_err(|e| e.to_string())?;
        Ok(s)
    }
}

/// No local listeners, unspecified addresses, multicast or broadcast endpoints.
pub fn valid_resolver_address(address: SocketAddr) -> bool {
    if let IpAddr::V6(ip) = address.ip()
        && let Some(ip) = ip.to_ipv4_mapped()
    {
        return valid_resolver_address((ip, address.port()).into());
    }
    address.port() > 0
        && !address.ip().is_unspecified()
        && !address.ip().is_loopback()
        && !address.ip().is_multicast()
        && !matches!(address.ip(), IpAddr::V4(ip) if ip.is_broadcast())
}

pub(crate) fn endpoint(value: &str, proxy: bool, bootstrap: &[SocketAddr]) -> Result<DnsServer> {
    ensure!(value.len() <= 2048, "DNS endpoint is too long");
    let url = url::Url::parse(value).map_err(|_| anyhow::anyhow!("Invalid DNS endpoint URL"))?;
    ensure!(
        matches!(url.scheme(), "udp" | "tcp" | "tls" | "https"),
        "DNS transport must be udp, tcp, tls or https"
    );
    ensure!(
        !proxy || url.scheme() != "udp",
        "Proxy DNS requires tcp, tls or https"
    );
    ensure!(
        url.username().is_empty() && url.password().is_none() && url.fragment().is_none(),
        "DNS endpoint must not contain credentials or a fragment"
    );
    let host = url.host_str().unwrap_or("").trim_matches(['[', ']']);
    ensure!(
        valid_domain(host) || host.parse::<IpAddr>().is_ok(),
        "Invalid DNS endpoint hostname"
    );
    let port = url.port().unwrap_or(match url.scheme() {
        "https" => 443,
        "tls" => 853,
        _ => 53,
    });
    ensure!(port > 0, "DNS port must be 1-65535");
    let mut server_name = host.to_owned();
    if url.scheme() == "tls" {
        let pairs = url.query_pairs().collect::<Vec<_>>();
        ensure!(
            pairs.len() <= 1
                && pairs
                    .iter()
                    .all(|(key, value)| key == "server_name" && valid_domain(value)),
            "TLS DNS supports only a valid server_name parameter"
        );
        if let Some((_, name)) = pairs.first() {
            server_name = name.to_string();
        }
    }
    if !matches!(url.scheme(), "https" | "tls") {
        ensure!(
            matches!(url.path(), "" | "/") && url.query().is_none(),
            "Only HTTPS DNS supports a path or query"
        );
    }
    if url.scheme() == "tls" {
        ensure!(
            matches!(url.path(), "" | "/"),
            "TLS DNS must not contain a path"
        );
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        ensure!(
            valid_resolver_address((ip, port).into()),
            "DNS endpoint must be a non-local unicast address"
        );
        if url.scheme() == "udp" {
            return Ok(DnsServer::Udp((ip, port).into()));
        }
        if url.scheme() == "tcp" {
            return Ok(if proxy {
                DnsServer::ProxyTcp((ip, port).into())
            } else {
                DnsServer::Tcp((ip, port).into())
            });
        }
        if url.scheme() == "tls" {
            return Ok(if proxy {
                DnsServer::ProxyTls((ip, port).into(), server_name)
            } else {
                DnsServer::Tls((ip, port).into(), server_name)
            });
        }
    } else {
        ensure!(url.scheme() != "udp", "UDP DNS requires an IP address");
        ensure!(
            !bootstrap.is_empty(),
            "Named DNS endpoints require bootstrap DNS"
        );
    }
    Ok(DnsServer::Endpoint {
        url: value.into(),
        proxy,
        bootstrap: bootstrap.to_vec(),
    })
}

pub fn valid_domain(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.split('.').all(|s| {
            !s.is_empty()
                && s.len() <= 63
                && !s.starts_with('-')
                && !s.ends_with('-')
                && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
}

impl DnsSettings {
    pub fn from_json(value: serde_json::Value) -> Result<Self> {
        let input: DnsSettingsInput = serde_json::from_value(value).map_err(|_| {
            anyhow::anyhow!("DNS fields have an invalid type or unsupported option")
        })?;
        input.try_into().map_err(anyhow::Error::msg)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=8).contains(&self.direct_servers.len()),
            "dns.direct_servers requires 1-8 endpoints"
        );
        ensure!(
            (1..=8).contains(&self.proxy_servers.len()),
            "dns.proxy_servers requires 1-8 endpoints"
        );
        ensure!(
            (1..=4).contains(&self.bootstrap_servers.len())
                && self
                    .bootstrap_servers
                    .iter()
                    .all(|s| valid_resolver_address(*s)),
            "dns.bootstrap_servers requires 1-4 non-local IP:port endpoints"
        );
        ensure!(
            (16..=16384).contains(&self.cache_capacity),
            "dns.cache_capacity must be 16-16384"
        );
        ensure!(
            self.cache_ttl <= 86400,
            "dns.cache_ttl must be 0-86400 seconds"
        );
        ensure!(
            (1..=12).contains(&self.timeout_secs),
            "dns.timeout_secs must be 1-12 seconds"
        );
        ensure!(
            self.direct_domains.len() <= 2048
                && self.direct_domains.iter().all(|s| valid_domain(s)),
            "dns.direct_domains contains an invalid domain or exceeds 2048 entries"
        );
        for (field, servers, proxy) in [
            ("direct_servers", &self.direct_servers, false),
            ("proxy_servers", &self.proxy_servers, true),
        ] {
            for (index, server) in servers.iter().enumerate() {
                endpoint(server, proxy, &self.bootstrap_servers)
                    .map_err(|e| anyhow::anyhow!("dns.{field}[{}]: {e}", index + 1))?;
            }
        }
        Ok(())
    }

    pub fn resolver_config(&self, node_hosts: impl Iterator<Item = String>) -> Result<DnsConfig> {
        self.validate()?;
        let mut config = crate::dns::china_split_dns_config();
        config.reject_local_addresses = true;
        let compile = |servers: &[String], proxy| {
            servers
                .iter()
                .map(|s| endpoint(s, proxy, &self.bootstrap_servers))
                .collect::<Result<Vec<_>>>()
        };
        config.fallback = compile(&self.direct_servers, false)?;
        config.servers = config.fallback.clone();
        config.proxy_servers = Some(compile(&self.proxy_servers, true)?);
        config.strategy = self.strategy;
        config.cache_enabled = self.cache_enabled;
        config.cache_capacity = self.cache_capacity;
        config.cache_ttl = self.cache_ttl;
        config.timeout_secs = self.timeout_secs;
        config.bootstrap_servers = Some(self.bootstrap_servers.clone());
        config.node_hosts = node_hosts
            .map(|s| s.trim_end_matches('.').to_ascii_lowercase())
            .collect();
        for domain in self
            .direct_domains
            .iter()
            .cloned()
            .chain(config.node_hosts.clone())
        {
            if domain.parse::<IpAddr>().is_err() {
                config.domain_rules.insert(
                    domain.trim_end_matches('.').to_ascii_lowercase(),
                    DnsGroup::Fallback,
                );
            }
        }
        Ok(config)
    }

    /// Used by CLI, API and GUI import. Explicit typed policy wins.
    pub fn from_yaml(value: &serde_yaml::Value) -> Result<Self> {
        if let Some(dns) = value.get("dns")
            && !dns.is_null()
        {
            return serde_yaml::from_value(dns.clone()).map_err(|_| {
                anyhow::anyhow!("Invalid dns configuration; check DNS fields and endpoints")
            });
        }
        let mut settings = Self::default();
        if let Some(server) = value.get("dns_direct_server") {
            let ip = server
                .as_str()
                .and_then(|s| s.parse::<IpAddr>().ok())
                .ok_or_else(|| anyhow::anyhow!("Invalid dns_direct_server"))?;
            settings.direct_servers = vec![format!("udp://{}", SocketAddr::new(ip, 53))];
        }
        if let Some(domains) = value.get("dns_direct_domains") {
            settings.direct_domains = serde_yaml::from_value(domains.clone())
                .map_err(|_| anyhow::anyhow!("dns_direct_domains must be a list of domains"))?;
        }
        settings.direct_domains = settings
            .direct_domains
            .into_iter()
            .map(|s| s.trim_end_matches('.').to_ascii_lowercase())
            .collect();
        settings.validate()?;
        Ok(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_migration_explicit_override_and_private_errors() {
        let value = serde_yaml::from_str(
            "dns_direct_server: 2001:db8::53\ndns_direct_domains: [example.test]",
        )
        .unwrap();
        let s = DnsSettings::from_yaml(&value).unwrap();
        assert_eq!(s.direct_servers, ["udp://[2001:db8::53]:53"]);
        assert_eq!(s.direct_domains, ["example.test"]);
        let explicit = serde_yaml::from_str("dns: {}\ndns_direct_server: invalid").unwrap();
        assert_eq!(
            DnsSettings::from_yaml(&explicit).unwrap(),
            DnsSettings::default()
        );
        let mut s = DnsSettings::default();
        s.direct_servers =
            vec!["https://secret:credential@resolver.example.test/dns-query?token=private".into()];
        let error = s.validate().unwrap_err().to_string();
        assert!(!error.contains("secret") && !error.contains("private"));
    }
    #[test]
    fn validates_routes_limits_and_recursion() {
        for server in [
            "udp://127.0.0.1:53",
            "udp://[::1]:53",
            "http://example.test",
            "udp://0.0.0.0:53",
            "udp://224.0.0.1:53",
        ] {
            let mut s = DnsSettings::default();
            s.direct_servers = vec![server.into()];
            assert!(s.validate().is_err());
        }
        let mut s = DnsSettings::default();
        s.proxy_servers = vec!["udp://192.0.2.53:53".into()];
        assert!(s.validate().is_err());
        assert!(serde_json::from_str::<DnsSettings>(r#"{"timeout_secs":0}"#).is_err());
        assert!(serde_json::from_str::<DnsSettings>(r#"{"cache_capcity":1}"#).is_err());
        assert_eq!(
            serde_json::from_str::<DnsSettings>("{}").unwrap(),
            DnsSettings::default()
        );
        let cfg = DnsSettings::default()
            .resolver_config(["node.example.test".into()].into_iter())
            .unwrap();
        assert_eq!(cfg.domain_rules["node.example.test"], DnsGroup::Fallback);
    }
}

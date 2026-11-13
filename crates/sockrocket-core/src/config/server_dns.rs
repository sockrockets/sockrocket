//! Subscription-scoped proxy endpoint DNS. Never used for destination DNS.
use super::{
    dns::{endpoint, valid_resolver_address},
    model::Node,
};
use crate::dns::{DnsConfig, DnsServer};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

pub const KEY: &str = "proxy_server_dns";
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerDns {
    pub servers: Vec<String>,
    pub bootstrap: Vec<String>,
    #[serde(default)]
    pub ipv6: bool,
}
impl ServerDns {
    pub fn config(&self) -> Result<DnsConfig> {
        ensure!(
            (1..=16).contains(&self.servers.len()),
            "Proxy server DNS requires 1-16 servers"
        );
        ensure!(
            self.bootstrap.len() <= 4,
            "Too many proxy DNS bootstrap servers"
        );
        let bootstrap = self
            .bootstrap
            .iter()
            .map(|v| {
                let addr = v
                    .parse()
                    .map_err(|_| anyhow::anyhow!("Invalid proxy DNS bootstrap address"))?;
                ensure!(
                    valid_resolver_address(addr),
                    "Invalid proxy DNS bootstrap address"
                );
                Ok(addr)
            })
            .collect::<Result<Vec<_>>>()?;
        let servers = self
            .servers
            .iter()
            .map(|v| parse_server(v, &bootstrap))
            .collect::<Result<Vec<_>>>()?;
        Ok(DnsConfig {
            servers,
            fallback: vec![],
            bootstrap_servers: Some(bootstrap),
            reject_local_addresses: true,
            strategy: if self.ipv6 {
                super::dns::AddressStrategy::PreferIpv4
            } else {
                super::dns::AddressStrategy::Ipv4Only
            },
            ..DnsConfig::default()
        })
    }
    pub fn from_node(node: &Node) -> Result<Option<Self>> {
        node.extra
            .get(KEY)
            .map(|s| {
                let policy: Self = serde_json::from_slice(s.as_bytes())
                    .map_err(|_| anyhow::anyhow!("Invalid proxy server DNS policy"))?;
                policy.config()?;
                Ok(policy)
            })
            .transpose()
    }
}
fn parse_server(value: &str, bootstrap: &[std::net::SocketAddr]) -> Result<DnsServer> {
    let value = if value.contains("://") {
        value.to_owned()
    } else {
        match value.parse::<std::net::IpAddr>() {
            Ok(ip) => format!(
                "udp://{}",
                if ip.is_ipv6() {
                    format!("[{ip}]")
                } else {
                    ip.to_string()
                }
            ),
            Err(_) => format!("udp://{value}"),
        }
    };
    endpoint(&value, false, bootstrap)
        .map_err(|_| anyhow::anyhow!("Invalid or unsupported proxy server DNS endpoint"))
}
/// Keep the policy on each imported node so subscriptions cannot overwrite one another.
pub fn import(root: &serde_yaml::Value) -> Result<Option<ServerDns>> {
    let Some(dns) = root.get("dns") else {
        return Ok(None);
    };
    if let Some(value) = dns.get("proxy-server-nameserver") {
        return parse_policy(dns, value, "proxy-server-nameserver");
    }
    if dns.get("enable").and_then(|v| v.as_bool()) == Some(false) {
        return Ok(None);
    }
    let Some(value) = dns.get("nameserver") else {
        return Ok(None);
    };
    // General DNS lists may include Mihomo-only transports. Inherit a list
    // only when we can use it in full, keeping older subscriptions importable.
    match parse_policy(dns, value, "nameserver") {
        Ok(policy) => Ok(policy),
        Err(_) => {
            tracing::warn!("Unsupported Clash nameserver settings; using existing node DNS");
            Ok(None)
        }
    }
}

fn parse_policy(
    dns: &serde_yaml::Value,
    value: &serde_yaml::Value,
    field: &str,
) -> Result<Option<ServerDns>> {
    let servers: Vec<String> = serde_yaml::from_value(value.clone())
        .map_err(|_| anyhow::anyhow!("{field} must be a list of strings"))?;
    if servers.is_empty() {
        return Ok(None);
    }
    let defaults: Vec<String> = dns
        .get("default-nameserver")
        .map(|v| serde_yaml::from_value(v.clone()))
        .transpose()
        .map_err(|_| anyhow::anyhow!("default-nameserver must be a list of strings"))?
        .unwrap_or_else(|| vec!["223.5.5.5".into(), "119.29.29.29".into()]);
    // Bootstrap resolvers must be IP based; encrypted IP endpoints can resolve
    // nodes directly but are not converted silently into plaintext bootstrap.
    let mut bootstrap = Vec::new();
    for v in defaults {
        if let Ok(DnsServer::Udp(addr)) = parse_server(&v, &[]) {
            bootstrap.push(addr.to_string());
        }
    }
    let policy = ServerDns {
        servers,
        bootstrap,
        ipv6: dns
            .get("ipv6")
            .and_then(serde_yaml::Value::as_bool)
            .unwrap_or(false),
    };
    policy.config()?;
    Ok(Some(policy))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(text: &str) -> Result<Option<ServerDns>> {
        import(&serde_yaml::from_str(text)?)
    }
    #[test]
    fn missing_and_empty_preserve_legacy_resolution() {
        assert!(parse("proxies: []").unwrap().is_none());
        assert!(
            parse("dns: {proxy-server-nameserver: []}")
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn inherits_enabled_clash_nameservers_for_endpoint_dns() {
        let yaml = r#"
dns:
  enable: true
  ipv6: false
  enhanced-mode: fake-ip
  default-nameserver: [192.0.2.53]
  nameserver: [https://first.example.test/dns-query, https://second.example.test/dns-query]
  fallback: [https://other.example.test/dns-query]
proxies:
- {name: sample, type: vmess, server: hk.example.test, port: 443, uuid: 00000000-0000-0000-0000-000000000001, alterId: 0, cipher: auto}
"#;
        let nodes = crate::config::clash::parse_clash_config(yaml).unwrap();
        let policy = ServerDns::from_node(&nodes[0]).unwrap().unwrap();
        assert_eq!(
            policy.servers,
            [
                "https://first.example.test/dns-query",
                "https://second.example.test/dns-query"
            ]
        );
        assert_eq!(policy.bootstrap, ["192.0.2.53:53"]);
        assert!(!policy.ipv6);
    }

    #[test]
    fn explicit_policy_takes_precedence_over_general_dns() {
        let p = parse("dns: {proxy-server-nameserver: [192.0.2.1], nameserver: [192.0.2.2]}")
            .unwrap()
            .unwrap();
        assert_eq!(p.servers, ["192.0.2.1"]);
        assert!(
            parse("dns: {proxy-server-nameserver: [], nameserver: [192.0.2.2]}")
                .unwrap()
                .is_none()
        );
        assert!(parse("dns: {proxy-server-nameserver: broken, nameserver: [192.0.2.2]}").is_err());
    }

    #[test]
    fn disabled_or_unsupported_general_dns_preserves_legacy_import() {
        for dns in [
            "{enable: false, nameserver: [192.0.2.53]}",
            "{nameserver: []}",
            "{nameserver: [192.0.2.53, 'https://dns.example.test/dns-query#group']}",
            "{nameserver: ['https://dns.example.test/dns-query'], default-nameserver: ['tls://192.0.2.53']}",
        ] {
            assert!(parse(&format!("dns: {dns}")).unwrap().is_none());
        }
    }
    #[test]
    fn parses_transports_and_bootstrap_without_changing_doh_paths() {
        let p = parse("dns:\n  proxy-server-nameserver: [1.1.1.1, 'tcp://1.1.1.1', 'tls://dns.example.test', 'https://dns.example.test:8447/dns-query/synthetic?key=test']\n  default-nameserver: [223.5.5.5, 'https://223.5.5.5/dns-query']").unwrap().unwrap();
        assert_eq!(p.config().unwrap().servers.len(), 4);
        assert_eq!(p.bootstrap, ["223.5.5.5:53"]);
        assert!(p.servers[3].ends_with("/dns-query/synthetic?key=test"));
    }
    #[test]
    fn invalid_policies_fail_without_disclosing_private_urls() {
        for value in [
            "'https://user:private@dns.example.test/token'",
            "'quic://dns.example.test/private'",
            "123",
            "'https://127.0.0.1/private'",
        ] {
            let error = parse(&format!("dns: {{proxy-server-nameserver: [{value}]}}"))
                .err()
                .unwrap()
                .to_string();
            assert!(
                !error.contains("private")
                    && !error.contains("token")
                    && !error.contains("dns.example")
            );
        }
        assert!(parse("dns: {proxy-server-nameserver: wrong}").is_err());
    }
    #[test]
    fn imported_policy_survives_save_and_separates_subscription_identity() {
        let yaml = "dns:\n  proxy-server-nameserver: ['https://dns.example.test/dns-query']\nproxies:\n- {name: synthetic, type: anytls, server: vpn.example.test, port: 443, password: synthetic}\n";
        let a = crate::config::clash::parse_clash_config(yaml)
            .unwrap()
            .remove(0);
        let b = crate::config::clash::parse_clash_config(
            &yaml.replace("dns.example.test", "other.example.test"),
        )
        .unwrap()
        .remove(0);
        assert_ne!(
            crate::config::dedup::node_fingerprint(&a),
            crate::config::dedup::node_fingerprint(&b)
        );
        let saved = serde_yaml::to_string(&a).unwrap();
        let restored: Node = serde_yaml::from_str(&saved).unwrap();
        assert_eq!(a.extra[KEY], restored.extra[KEY]);
        assert_eq!(a.server, "vpn.example.test");
    }
}

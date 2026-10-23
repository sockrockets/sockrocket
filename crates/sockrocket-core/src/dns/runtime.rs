//! Per-request policy context and independent node bootstrap configuration.
use super::*;
use std::collections::HashSet;
use std::future::Future;
use std::sync::{OnceLock, RwLock as StdRwLock};

tokio::task_local! { static REQUEST_DNS: DnsResolver; }
pub(crate) async fn with_resolver<F: Future>(
    resolver: Option<DnsResolver>,
    future: F,
) -> F::Output {
    match resolver {
        Some(resolver) => REQUEST_DNS.scope(resolver, future).await,
        None => future.await,
    }
}
pub(crate) async fn scoped_resolve(host: &str) -> Option<Result<Vec<IpAddr>>> {
    let resolver = REQUEST_DNS.try_with(Clone::clone).ok()?;
    let canonical = host.trim_end_matches('.').to_ascii_lowercase();
    if resolver.config.node_hosts.contains(&canonical)
        && let Some(bootstrap) = &resolver.bootstrap
    {
        return Some(bootstrap.resolve_addresses(host).await);
    }
    if crate::proxy::routing::tun_ipv6() {
        return Some(resolver.resolve_with_qtype(host, 28).await);
    }
    Some(resolver.resolve_addresses(host).await)
}

struct Bootstrap {
    servers: Vec<SocketAddr>,
    hosts: HashSet<String>,
    resolver: DnsResolver,
    strategy: crate::config::dns::AddressStrategy,
    timeout_secs: u64,
}
static BOOTSTRAP: OnceLock<StdRwLock<Option<Bootstrap>>> = OnceLock::new();

/// Atomically replace node bootstrap policy; never use the local OS resolver
/// as a fallback once a user has configured independent bootstrap servers.
pub fn configure_bootstrap(
    settings: &crate::config::dns::DnsSettings,
    hosts: impl Iterator<Item = String>,
) -> Result<()> {
    settings.validate()?;
    let hosts = hosts
        .map(|s| s.trim_end_matches('.').to_ascii_lowercase())
        .collect();
    let lock = BOOTSTRAP.get_or_init(|| StdRwLock::new(None));
    let mut policy = lock.write().unwrap();
    if let Some(policy) = policy.as_mut()
        && policy.servers == settings.bootstrap_servers
        && policy.strategy == settings.strategy
        && policy.timeout_secs == settings.timeout_secs
    {
        policy.hosts = hosts;
        return Ok(());
    }
    let resolver = DnsResolver::with_config(DnsConfig {
        reject_local_addresses: true,
        servers: settings
            .bootstrap_servers
            .iter()
            .copied()
            .map(DnsServer::Udp)
            .collect(),
        fallback: vec![],
        cache_ttl: 0,
        timeout_secs: settings.timeout_secs,
        strategy: crate::config::dns::AddressStrategy::PreferIpv4,
        ..Default::default()
    });
    *policy = Some(Bootstrap {
        servers: settings.bootstrap_servers.clone(),
        hosts,
        resolver,
        strategy: settings.strategy,
        timeout_secs: settings.timeout_secs,
    });
    Ok(())
}
/// True unless the user replaced the default bootstrap servers; only the
/// default list may fall back to public or OS resolvers.
pub(crate) fn bootstrap_is_default() -> bool {
    BOOTSTRAP
        .get()
        .and_then(|lock| lock.read().ok())
        .and_then(|policy| {
            policy
                .as_ref()
                .map(|p| p.servers == crate::config::dns::DnsSettings::default().bootstrap_servers)
        })
        .unwrap_or(true)
}
pub(crate) fn bootstrap_resolver(host: &str) -> Option<(DnsResolver, bool)> {
    let policy = BOOTSTRAP.get()?.read().ok()?;
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    policy
        .as_ref()
        .map(|p| (p.resolver.clone(), p.hosts.contains(&host)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_matches_node_hosts_case_insensitively() {
        let settings = crate::config::dns::DnsSettings::default();
        configure_bootstrap(&settings, std::iter::once("Node.Example".to_string())).unwrap();
        assert!(bootstrap_resolver("node.example.").unwrap().1);
        assert!(!bootstrap_resolver("other.example").unwrap().1);
    }
}

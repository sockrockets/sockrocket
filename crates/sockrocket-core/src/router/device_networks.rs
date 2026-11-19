//! Kernel CN bypass source export, including IPv6 neighbors of DHCP devices.
use crate::config::model::{DeviceRuleGroup, PausedDeviceSources, RoutingRule};
use ipnet::IpNet;
use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
};

pub fn active_device_networks_with_aliases(
    rules: &[RoutingRule],
    links: &[DeviceRuleGroup],
    paused: &PausedDeviceSources,
    aliases: &BTreeMap<Ipv6Addr, Ipv4Addr>,
) -> BTreeSet<String> {
    let mut sources = super::active_device_networks(rules, links, paused);
    if aliases.is_empty() {
        return sources;
    }
    let active_v4: Vec<_> = sources
        .iter()
        .filter_map(|s| s.parse::<ipnet::Ipv4Net>().ok())
        .collect();
    let explicit_v6: Vec<_> = super::device_policy_sources(rules, links)
        .filter_map(|s| s.parse::<ipnet::Ipv6Net>().ok())
        .collect();
    for (v6, v4) in aliases {
        if active_v4.iter().any(|s| s.contains(v4))
            && !explicit_v6.iter().any(|s| s.contains(v6))
            && !paused
                .0
                .iter()
                .any(|s: &IpNet| s.contains(&IpAddr::V6(*v6)))
        {
            sources.insert(format!("{v6}/128"));
        }
    }
    sources
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inherited_ipv6_kernel_sources_respect_pauses_and_explicit_v6() {
        let group = |source: &str| DeviceRuleGroup {
            source: source.to_owned(),
            group: "device".to_owned(),
            priority: 0,
            target: String::new(),
        };
        let links = [group("192.0.2.0/24"), group("2001:db8:3::/64")];
        let paused =
            PausedDeviceSources::try_from(vec!["192.0.2.2".to_owned(), "2001:db8:2::1".to_owned()])
                .unwrap();
        let aliases = [
            ("2001:db8:1::1", "192.0.2.1"),
            ("2001:db8:1::2", "192.0.2.2"),
            ("2001:db8:2::1", "192.0.2.1"),
            ("2001:db8:3::1", "192.0.2.1"),
        ]
        .into_iter()
        .map(|(a, b)| (a.parse().unwrap(), b.parse().unwrap()))
        .collect();
        let set = active_device_networks_with_aliases(&[], &links, &paused, &aliases);
        assert!(set.contains("2001:db8:1::1/128"));
        assert!(!set.contains("2001:db8:1::2/128"));
        assert!(!set.contains("2001:db8:2::1/128"));
        assert!(set.contains("2001:db8:3::/64"));
        assert!(!set.contains("2001:db8:3::1/128"));
    }
}

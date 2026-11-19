//! Join current dnsmasq IPv4 leases to individual IPv6 neighbors on the primary LAN.
use anyhow::Result;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    net::{Ipv4Addr, Ipv6Addr},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_ROWS: usize = 8192;

fn mac(s: &str) -> Option<[u8; 6]> {
    let mut bytes = [0; 6];
    let mut parts = s.split(':');
    for b in &mut bytes {
        let part = parts.next()?;
        if part.len() != 2 {
            return None;
        }
        *b = u8::from_str_radix(part, 16).ok()?;
    }
    (parts.next().is_none() && bytes[0] & 1 == 0 && bytes != [0; 6]).then_some(bytes)
}

fn usable(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    first & 0xe000 == 0x2000 || first & 0xfe00 == 0xfc00
}

fn insert_unique<K: Ord, V: Copy + Eq>(map: &mut BTreeMap<K, Option<V>>, key: K, value: V) {
    let old = map.entry(key).or_insert(Some(value));
    if *old != Some(value) {
        *old = None;
    }
}

fn discover(
    leases: &str,
    arp: &str,
    neighbors: &str,
    iface: &str,
    now: u64,
) -> BTreeMap<Ipv6Addr, Ipv4Addr> {
    // A recycled IP or two simultaneous leases for one MAC cannot identify a
    // single device policy. Only publish unambiguous, unexpired associations.
    let mut by_ip = BTreeMap::new();
    let mut by_mac = BTreeMap::new();
    for line in leases.lines().take(MAX_ROWS) {
        let mut parts = line.split_ascii_whitespace();
        let expiry = parts.next().and_then(|s| s.parse::<u64>().ok());
        let hw = parts.next().and_then(mac);
        let ip = parts.next().and_then(|s| s.parse::<Ipv4Addr>().ok());
        if expiry.is_some_and(|e| e == 0 || e > now)
            && let (Some(hw), Some(ip)) = (hw, ip)
        {
            insert_unique(&mut by_ip, ip, hw);
            insert_unique(&mut by_mac, hw, ip);
        }
    }
    let mut conflicts = BTreeSet::new();
    for line in arp.lines().take(MAX_ROWS) {
        let fields: Vec<_> = line.split_ascii_whitespace().collect();
        if let [addr, _, flags, hw, _, dev] = fields.as_slice()
            && let Ok(ip) = addr.parse::<Ipv4Addr>()
            && u32::from_str_radix(flags.trim_start_matches("0x"), 16).is_ok_and(|f| f & 2 != 0)
            && (*dev != iface || by_ip.get(&ip) != Some(&mac(hw)))
        {
            conflicts.insert(ip);
        }
    }
    let mut found = BTreeMap::new();
    for line in neighbors.lines().take(MAX_ROWS) {
        let fields: Vec<_> = line.split_ascii_whitespace().collect();
        let Some(ip) = fields
            .first()
            .and_then(|s| s.parse::<Ipv6Addr>().ok())
            .filter(|ip| usable(*ip))
        else {
            continue;
        };
        if !matches!(
            fields.last(),
            Some(&("REACHABLE" | "STALE" | "DELAY" | "PROBE" | "PERMANENT"))
        ) || fields.contains(&"router")
            || fields.windows(2).any(|w| w[0] == "dev" && w[1] != iface)
        {
            continue;
        }
        let hw = fields
            .windows(2)
            .find(|w| w[0] == "lladdr")
            .and_then(|w| mac(w[1]));
        if let Some(hw) = hw {
            // Conflicting observations for a single IPv6 address invalidate it,
            // including when only one of the MACs has a known DHCP lease.
            insert_unique(&mut found, ip, hw);
        }
    }
    found
        .into_iter()
        .filter_map(|(v6, hw)| {
            let hw = hw?;
            let v4 = (*by_mac.get(&hw)?)?;
            (by_ip.get(&v4) == Some(&Some(hw)) && !conflicts.contains(&v4)).then_some((v6, v4))
        })
        .collect()
}

type IdentityMap = BTreeMap<Ipv6Addr, Ipv4Addr>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MapVersion {
    modified: std::time::Duration,
    inode: u64,
}

pub(crate) fn version() -> std::io::Result<Option<MapVersion>> {
    let metadata = match fs::metadata(super::runtime_dir().join("device-ipv6-map")) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    #[cfg(unix)]
    let inode = std::os::unix::fs::MetadataExt::ino(&metadata);
    #[cfg(not(unix))]
    let inode = 0;
    Ok(Some(MapVersion {
        modified: metadata
            .modified()?
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default(),
        inode,
    }))
}

pub(crate) fn revision() -> Option<String> {
    version().ok().map(|stamp| {
        stamp.map_or_else(
            || "absent".to_owned(),
            |v| format!("{}-{}", v.inode, v.modified.as_nanos()),
        )
    })
}

pub(crate) fn read() -> IdentityMap {
    try_read().unwrap_or_default()
}

pub(crate) fn try_read() -> std::io::Result<IdentityMap> {
    let contents = match fs::read_to_string(super::runtime_dir().join("device-ipv6-map")) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e),
    };
    Ok(contents
        .lines()
        .take(MAX_ROWS)
        .filter_map(|s| {
            let (v6, v4) = s.split_once(' ')?;
            let v6 = v6.parse().ok()?;
            usable(v6).then_some((v6, v4.parse().ok()?))
        })
        .collect())
}

fn sample(iface: &str) -> Result<BTreeMap<Ipv6Addr, Ipv4Addr>> {
    anyhow::ensure!(
        !iface.is_empty()
            && iface.len() < 16
            && iface
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b)),
        "invalid LAN interface"
    );
    let leases = fs::read_to_string("/var/lib/misc/dnsmasq.leases")?;
    let arp = fs::read_to_string("/proc/net/arp")?;
    let neighbors = Command::new("ip")
        .args(["-6", "neigh", "show", "dev", iface])
        .output()?;
    anyhow::ensure!(neighbors.status.success(), "cannot read LAN IPv6 neighbors");
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    Ok(discover(
        &leases,
        &arp,
        std::str::from_utf8(&neighbors.stdout)?,
        iface,
        now,
    ))
}

pub(crate) fn refresh(iface: &str) -> Result<()> {
    publish_sample(&super::runtime_dir(), sample(iface))
}

fn publish_sample(
    dir: &std::path::Path,
    sample: Result<BTreeMap<Ipv6Addr, Ipv4Addr>>,
) -> Result<()> {
    fs::create_dir_all(dir)?;
    let last_good = dir.join("device-ipv6-last-good");
    let observed = sample.is_ok();
    let map = match sample {
        Ok(map) => map,
        Err(_) => {
            // Transient netlink/lease read failures must not withdraw a valid
            // device policy immediately. Never retain old ownership indefinitely.
            if fs::metadata(&last_good)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age.as_secs() < 120)
            {
                return Ok(());
            }
            BTreeMap::new()
        }
    };
    let mut text = String::new();
    for (v6, v4) in map {
        use std::fmt::Write;
        writeln!(text, "{v6} {v4}")?;
    }
    let path = dir.join("device-ipv6-map");
    if fs::read(&path).ok().as_deref() != Some(text.as_bytes()) {
        sockrocket_core::config::storage::atomic_write_private(&path, &text, false)?;
    }
    if observed {
        // Only extend the grace period once this observation is the published map.
        fs::write(last_good, [])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_sampling_expires_and_empty_success_withdraws_immediately() {
        let tmp = std::env::temp_dir().join(format!(
            "sockrocket-ipv6-identity-{:032x}",
            rand::random::<u128>()
        ));
        fs::create_dir(&tmp).unwrap();
        let dir = tmp.as_path();
        let known =
            BTreeMap::from([("2001:db8::1".parse().unwrap(), "192.0.2.1".parse().unwrap())]);
        let path = dir.join("device-ipv6-map");
        publish_sample(dir, Ok(known.clone())).unwrap();
        publish_sample(dir, Err(anyhow::anyhow!("neighbor sample unavailable"))).unwrap();
        assert!(!fs::read(&path).unwrap().is_empty());
        fs::OpenOptions::new()
            .write(true)
            .open(dir.join("device-ipv6-last-good"))
            .unwrap()
            .set_modified(UNIX_EPOCH)
            .unwrap();
        publish_sample(dir, Err(anyhow::anyhow!("still unavailable"))).unwrap();
        assert!(fs::read(&path).unwrap().is_empty());
        publish_sample(dir, Ok(known)).unwrap();
        publish_sample(dir, Ok(BTreeMap::new())).unwrap();
        assert!(fs::read(&path).unwrap().is_empty());
        fs::remove_dir_all(tmp).unwrap();
    }
    #[test]
    fn failed_publication_does_not_extend_previous_identity_lifetime() {
        let dir = std::env::temp_dir().join(format!(
            "sockrocket-ipv6-publish-{:032x}",
            rand::random::<u128>()
        ));
        fs::create_dir(&dir).unwrap();
        let last_good = dir.join("device-ipv6-last-good");
        fs::File::create(&last_good)
            .unwrap()
            .set_modified(UNIX_EPOCH)
            .unwrap();
        let previous = fs::metadata(&last_good).unwrap().modified().unwrap();
        // A directory cannot be replaced by the new regular-file snapshot.
        fs::create_dir(dir.join("device-ipv6-map")).unwrap();
        assert!(publish_sample(&dir, Ok(BTreeMap::new())).is_err());
        assert_eq!(
            fs::metadata(last_good).unwrap().modified().unwrap(),
            previous
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn identifies_only_current_unambiguous_lan_lease_and_neighbor_pairs() {
        let leases = "0 02:00:00:00:00:01 192.0.2.1 a\n500 02:00:00:00:00:02 192.0.2.2 b\n100 02:00:00:00:00:03 192.0.2.3 c";
        let n = "2001:db8::10 lladdr 02:00:00:00:00:01 STALE\n2001:db8::11 dev br0 lladdr 02:00:00:00:00:01 REACHABLE\n2001:db8::20 lladdr 02:00:00:00:00:02 REACHABLE\n2001:db8::30 lladdr 02:00:00:00:00:03 REACHABLE\n2001:db8::40 dev br1 lladdr 02:00:00:00:00:01 REACHABLE\n2001:db8::50 lladdr 02:00:00:00:00:01 FAILED\nfe80::1 lladdr 02:00:00:00:00:01 REACHABLE";
        let arp = "192.0.2.2 0x1 0x2 02:00:00:00:00:01 * br0";
        let map = discover(leases, arp, n, "br0", 101);
        assert_eq!(map.len(), 2);
        for ip in ["2001:db8::10", "2001:db8::11"] {
            assert_eq!(
                map[&ip.parse::<Ipv6Addr>().unwrap()],
                Ipv4Addr::new(192, 0, 2, 1)
            );
        }
        assert!(discover(leases, arp, "", "br0", 101).is_empty());
        let ambiguous = format!("{leases}\n0 02:00:00:00:00:01 192.0.2.9 d");
        assert!(discover(&ambiguous, arp, n, "br0", 101).is_empty());
    }
}

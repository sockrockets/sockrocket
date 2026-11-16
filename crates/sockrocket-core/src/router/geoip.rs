use std::net::IpAddr;
use std::ops::Range;

pub(super) fn parse_cidr(cidr: &str) -> Option<(IpAddr, u8)> {
    let (address, prefix) = cidr.split_once('/')?;
    let address: IpAddr = address.parse().ok()?;
    let prefix: u8 = prefix.parse().ok()?;
    (prefix <= if address.is_ipv4() { 32 } else { 128 }).then_some((address, prefix))
}

pub(super) fn private_networks() -> impl Iterator<Item = (IpAddr, u8)> {
    const CIDRS: &[&str] = &[
        "10.0.0.0/8",
        "172.16.0.0/12",
        "192.168.0.0/16",
        "127.0.0.0/8",
        "169.254.0.0/16",
        "100.64.0.0/10",
        "::1/128",
        "fc00::/7",
        "fe80::/10",
    ];
    CIDRS
        .iter()
        .map(|cidr| parse_cidr(cidr).expect("valid built-in private CIDR"))
}

/// A lightweight GeoIP database for country-level lookups.
/// Uses in-memory CIDR→country mappings (can be loaded from file).
pub struct GeoIpDb {
    v4_entries: GeoTable,
    v6_entries: GeoTable,
}

#[derive(Debug, Clone)]
struct GeoEntry {
    addr: u128, // stored as u128 for both v4 and v6
    prefix_len: u8,
    country: String,
}

/// One compact sorted array, plus a range for each populated prefix length.
/// No per-address nodes or heap allocation on lookup.
#[derive(Default)]
struct GeoTable {
    entries: Vec<GeoEntry>,
    prefixes: Vec<(u128, Range<usize>)>,
}

fn prefix_mask(total_bits: u32, prefix_len: u8) -> u128 {
    u128::MAX
        .checked_shl(total_bits - u32::from(prefix_len))
        .unwrap_or(0)
}

impl GeoTable {
    fn push(&mut self, entry: GeoEntry) {
        self.entries.push(entry);
        // Preserve lookup-before-build behavior, including additions after build.
        self.prefixes.clear();
    }

    fn build(&mut self, total_bits: u32) {
        // Stable sorting keeps the first configured country for duplicate CIDRs.
        self.entries
            .sort_by_key(|entry| (std::cmp::Reverse(entry.prefix_len), entry.addr));
        self.prefixes.clear();
        let mut start = 0;
        while start < self.entries.len() {
            let prefix = self.entries[start].prefix_len;
            let end =
                start + self.entries[start..].partition_point(|entry| entry.prefix_len == prefix);
            self.prefixes
                .push((prefix_mask(total_bits, prefix), start..end));
            start = end;
        }
    }

    fn lookup(&self, ip: u128, total_bits: u32) -> Option<&str> {
        if self.prefixes.is_empty() {
            return self
                .entries
                .iter()
                .find(|entry| ip & prefix_mask(total_bits, entry.prefix_len) == entry.addr)
                .map(|entry| entry.country.as_str());
        }
        for (mask, range) in &self.prefixes {
            let network = ip & mask;
            let entries = &self.entries[range.clone()];
            let index = entries.partition_point(|entry| entry.addr < network);
            if let Some(entry) = entries.get(index).filter(|entry| entry.addr == network) {
                return Some(&entry.country);
            }
        }
        None
    }
}

impl GeoIpDb {
    /// Create an empty GeoIP database.
    pub fn new() -> Self {
        Self {
            v4_entries: GeoTable::default(),
            v6_entries: GeoTable::default(),
        }
    }

    /// Add a CIDR→country mapping.
    pub fn add_entry(&mut self, cidr: &str, country: &str) {
        if let Some((address, prefix)) = parse_cidr(cidr) {
            self.add_network(address, prefix, country);
        }
    }

    pub(super) fn add_network(&mut self, addr: IpAddr, prefix_len: u8, country: &str) {
        let total_bits = if addr.is_ipv4() { 32 } else { 128 };
        if u32::from(prefix_len) > total_bits {
            return;
        }
        let entry = GeoEntry {
            addr: ip_to_u128(addr) & prefix_mask(total_bits, prefix_len),
            prefix_len,
            country: country.to_uppercase(),
        };

        match addr {
            IpAddr::V4(_) => self.v4_entries.push(entry),
            IpAddr::V6(_) => self.v6_entries.push(entry),
        }
    }

    /// Sort entries for efficient lookup (longest prefix first).
    pub fn build(&mut self) {
        self.v4_entries.build(32);
        self.v6_entries.build(128);
    }

    /// True when `ip` is classified as mainland China (`CN`).
    pub fn is_cn(&self, ip: IpAddr) -> bool {
        matches!(self.lookup(ip), Some(c) if c.eq_ignore_ascii_case("CN"))
    }

    /// Lookup the country code for an IP address.
    pub fn lookup(&self, ip: IpAddr) -> Option<&str> {
        match ip {
            IpAddr::V4(_) => self.v4_entries.lookup(ip_to_u128(ip), 32),
            IpAddr::V6(_) => self.v6_entries.lookup(ip_to_u128(ip), 128),
        }
    }

    /// Load a simple text format: each line is "CIDR COUNTRY_CODE".
    pub fn load_from_text(text: &str) -> Self {
        let mut db = Self::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                db.add_entry(parts[0], parts[1]);
            }
        }
        db.build();
        db
    }

    /// Create a built-in database with common private/reserved ranges.
    pub fn builtin() -> Self {
        let mut db = Self::with_private_ranges();
        db.build();
        db
    }

    pub(super) fn with_private_ranges() -> Self {
        let mut db = Self::new();
        for (address, prefix) in private_networks() {
            db.add_network(address, prefix, "PRIVATE");
        }
        db
    }

    pub fn entry_count(&self) -> usize {
        self.v4_entries.entries.len() + self.v6_entries.entries.len()
    }
}

impl Default for GeoIpDb {
    fn default() -> Self {
        Self::new()
    }
}

fn ip_to_u128(ip: IpAddr) -> u128 {
    match ip {
        IpAddr::V4(v4) => {
            // Store IPv4 in the upper bits of a 32-bit space
            // but we shift to align with mask calculations
            u32::from(v4) as u128
        }
        IpAddr::V6(v6) => u128::from(v6),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_lookup_boundaries_and_rebuild() {
        let mut db = GeoIpDb::load_from_text(
            "0.0.0.0/0 DEFAULT4\n::/0 DEFAULT6\n192.0.2.99/24 FIRST\n192.0.2.0/24 SECOND\n192.0.2.255/32 HOST\n2001:db8::abcd/32 V6\n2001:db8::1/128 HOST6",
        );
        for (address, expected) in [
            ("192.0.1.255", "DEFAULT4"),
            ("192.0.2.0", "FIRST"),
            ("192.0.2.254", "FIRST"),
            ("192.0.2.255", "HOST"),
            ("192.0.3.0", "DEFAULT4"),
            ("2001:db8::", "V6"),
            ("2001:db8::1", "HOST6"),
            ("2001:db9::", "DEFAULT6"),
        ] {
            assert_eq!(db.lookup(address.parse().unwrap()), Some(expected));
        }
        let count = db.entry_count();
        db.add_entry("192.0.2.1/33", "INVALID");
        db.add_entry("::1/129", "INVALID");
        assert_eq!(db.entry_count(), count);
        db.add_entry("192.0.2.10/32", "NEW");
        // Before rebuilding, preserve the previous insertion/scan semantics.
        assert_eq!(db.lookup("192.0.2.10".parse().unwrap()), Some("FIRST"));
        db.build();
        db.build();
        assert_eq!(db.lookup("192.0.2.10".parse().unwrap()), Some("NEW"));
        assert_eq!(db.lookup("192.0.2.99".parse().unwrap()), Some("FIRST"));
    }

    #[test]
    fn indexed_lookup_matches_independent_linear_reference() {
        // Deterministic mixed prefixes, host bits, duplicates, and misses.
        let mut seed = 0x719d_a341_1278_adefu128;
        let mut next = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            seed
        };
        for v6 in [false, true] {
            let bits = if v6 { 128 } else { 32 };
            let mut db = GeoIpDb::new();
            let mut reference = Vec::new();
            for i in 0..600 {
                let raw = next();
                let ip = if v6 {
                    IpAddr::V6(std::net::Ipv6Addr::from(raw))
                } else {
                    IpAddr::V4(std::net::Ipv4Addr::from(raw as u32))
                };
                let prefix = (next() % (bits + 1)) as u8;
                let country = format!("C{i}");
                db.add_entry(&format!("{ip}/{prefix}"), &country);
                reference.push((ip_to_u128(ip), prefix, country));
            }
            reference.sort_by_key(|entry| std::cmp::Reverse(entry.1));
            db.build();
            for _ in 0..4000 {
                let raw = next();
                let ip = if v6 {
                    IpAddr::V6(std::net::Ipv6Addr::from(raw))
                } else {
                    IpAddr::V4(std::net::Ipv4Addr::from(raw as u32))
                };
                let value = ip_to_u128(ip);
                let expected = reference
                    .iter()
                    .find(|(network, prefix, _)| {
                        let shift = bits as u32 - u32::from(*prefix);
                        if shift == 128 {
                            true
                        } else {
                            value >> shift == network >> shift
                        }
                    })
                    .map(|entry| entry.2.as_str());
                assert_eq!(db.lookup(ip), expected, "{ip}");
            }
        }
    }

    #[test]
    fn test_builtin_private() {
        let db = GeoIpDb::builtin();

        assert_eq!(db.lookup("192.168.1.1".parse().unwrap()), Some("PRIVATE"));
        assert_eq!(db.lookup("10.0.0.1".parse().unwrap()), Some("PRIVATE"));
        assert_eq!(db.lookup("172.16.5.1".parse().unwrap()), Some("PRIVATE"));
        assert_eq!(db.lookup("127.0.0.1".parse().unwrap()), Some("PRIVATE"));
        assert_eq!(db.lookup("::1".parse().unwrap()), Some("PRIVATE"));

        // Public IPs should not match
        assert_eq!(db.lookup("8.8.8.8".parse().unwrap()), None);
        assert_eq!(db.lookup("1.1.1.1".parse().unwrap()), None);
    }

    #[test]
    fn test_china_geoip_is_cn() {
        let db = crate::router::china_geoip_db();
        assert!(db.is_cn("114.114.114.114".parse().unwrap()));
        assert!(!db.is_cn("8.8.8.8".parse().unwrap()));
        assert!(
            !db.is_cn("10.0.0.1".parse().unwrap()),
            "PRIVATE must not count as CN for resolve-then-geoip"
        );
    }

    #[test]
    fn test_load_from_text() {
        let text = r#"
# GeoIP test data
1.0.0.0/8 CN
8.8.8.0/24 US
2001:db8::/32 TEST
"#;
        let db = GeoIpDb::load_from_text(text);
        assert_eq!(db.entry_count(), 3);

        assert_eq!(db.lookup("1.2.3.4".parse().unwrap()), Some("CN"));
        assert_eq!(db.lookup("8.8.8.8".parse().unwrap()), Some("US"));
        assert_eq!(db.lookup("8.8.9.1".parse().unwrap()), None);
        assert_eq!(db.lookup("2001:db8::1".parse().unwrap()), Some("TEST"));
    }

    #[test]
    fn test_longest_prefix_match() {
        let text = r#"
1.0.0.0/8 CN
1.1.1.0/24 US
"#;
        let db = GeoIpDb::load_from_text(text);

        // 1.1.1.1 matches both, but /24 is more specific
        assert_eq!(db.lookup("1.1.1.1".parse().unwrap()), Some("US"));
        // 1.2.3.4 only matches /8
        assert_eq!(db.lookup("1.2.3.4".parse().unwrap()), Some("CN"));
    }
}

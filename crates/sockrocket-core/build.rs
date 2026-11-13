use std::env;
use std::fs;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::PathBuf;

fn pack_prefixes<const N: usize>(cidrs: &[&str], octets: impl Fn(&str) -> [u8; N]) -> Vec<u8> {
    let entries: Vec<_> = cidrs
        .iter()
        .map(|cidr| {
            let (address, prefix) = cidr.split_once('/').expect("invalid built-in CIDR");
            let prefix: u8 = prefix.parse().expect("invalid prefix length");
            assert!(usize::from(prefix) <= N * 8);
            (octets(address), prefix)
        })
        .collect();
    // All addresses share a fixed record width. Omit only trailing octets
    // that are zero in every entry, even for non-canonical input addresses.
    let width = entries
        .iter()
        .filter_map(|(address, _)| address.iter().rposition(|&octet| octet != 0))
        .max()
        .map_or(0, |last| last + 1);
    let mut packed = Vec::with_capacity(1 + entries.len() * (width + 1));
    packed.push(width as u8);
    for (address, prefix) in entries {
        packed.extend_from_slice(&address[..width]);
        packed.push(prefix);
    }
    packed
}

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let v4 = include!("data/china_ipv4_cidrs.inc.rs");
    let v6 = include!("data/china_ipv6_cidrs.inc.rs");
    println!("cargo:rerun-if-changed=data/china_ipv4_cidrs.inc.rs");
    println!("cargo:rerun-if-changed=data/china_ipv6_cidrs.inc.rs");

    // Preserve the public text API for callers that borrow the original CIDRs.
    fs::write(out.join("china_ipv4.txt"), v4.join("\n") + "\n").unwrap();
    fs::write(
        out.join("china_ipv4.bin"),
        pack_prefixes(&v4, |s| s.parse::<Ipv4Addr>().unwrap().octets()),
    )
    .unwrap();
    fs::write(
        out.join("china_ipv6.bin"),
        pack_prefixes(&v6, |s| s.parse::<Ipv6Addr>().unwrap().octets()),
    )
    .unwrap();
}

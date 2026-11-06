//! Standalone, dependency-free lookup benchmark. Run from the repository root:
//! SOCKROCKET_GEOIP_SOURCE="$PWD/crates/sockrocket-core/src/router/geoip.rs" \
//!   rustc --edition 2024 -O tools/benchmark-geoip.rs -o /tmp/benchmark-geoip
//! /tmp/benchmark-geoip
//! For a before/after comparison, compile the same harness against a saved
//! pre-change geoip.rs, then alternate both binaries. This measures CPU lookup
//! cost, not DNS network latency or end-to-end proxy throughput.
#![allow(dead_code)]
mod geoip {
    include!(env!("SOCKROCKET_GEOIP_SOURCE"));
}
use geoip::GeoIpDb;
use std::{hint::black_box, net::IpAddr, time::Instant};

fn measure(label: &str, db: &GeoIpDb, queries: &[IpAddr]) {
    let mut matched = 0usize;
    for ip in queries {
        black_box(db.lookup(black_box(*ip)));
    }
    let start = Instant::now();
    let count = 200_000;
    for i in 0..count {
        matched += black_box(db.lookup(black_box(queries[i % queries.len()]))).is_some() as usize;
    }
    println!(
        "{label}: entries={} ns/lookup={:.1} matches={matched}",
        db.entry_count(),
        start.elapsed().as_nanos() as f64 / count as f64
    );
}
fn main() {
    let mut china = GeoIpDb::builtin();
    let v4: &[&str] = &include!("../crates/sockrocket-core/data/china_ipv4_cidrs.inc.rs");
    let v6: &[&str] = &include!("../crates/sockrocket-core/data/china_ipv6_cidrs.inc.rs");
    for cidr in v4.iter().chain(v6) {
        china.add_entry(cidr, "CN");
    }
    china.build();
    let queries: Vec<IpAddr> = [
        "114.114.114.114",
        "8.8.8.8",
        "1.1.8.8",
        "192.168.1.1",
        "240e::1",
        "2001:4860:4860::8888",
        "223.5.5.5",
        "9.9.9.9",
    ]
    .iter()
    .map(|ip| ip.parse().unwrap())
    .collect();
    measure("builtin-mixed", &china, &queries);
    let mut large = GeoIpDb::new();
    for i in 0..8192u32 {
        large.add_entry(
            &format!("{}/24", std::net::Ipv4Addr::from(0x0b000000 + (i << 8))),
            "TEST",
        );
    }
    large.build();
    let queries: Vec<IpAddr> = (0..1024u32)
        .map(|i| {
            let base = if i % 2 == 0 { 0x0b000000 } else { 0x0c000000 };
            IpAddr::V4(std::net::Ipv4Addr::from(
                base + (((i * 7919) % 8192) << 8) + 127,
            ))
        })
        .collect();
    measure("8192-mixed-hit-miss", &large, &queries);
}

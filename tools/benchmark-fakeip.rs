//! Fake-IP request-path latency with a nearly full persisted mapping table.
//! Run this identical harness as a Cargo example against before/after core versions.
//! Run the binaries alternately; all fixture files live under std::env::temp_dir.
//! Only allocation latency is timed; persist() is a separate correctness barrier.
//! This is a cold persistence-boundary test, not a proxy-throughput benchmark.
use sockrocket_core::FakeIpPool;
use std::{fmt::Write, fs, net::Ipv4Addr, time::Instant};
fn main() {
    let directory =
        std::env::temp_dir().join(format!("sockrocket-fakeip-bench-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let path = directory.join("synthetic.store");
    let mut fixture = String::from("# sockrocket fake-ip store v1\nnext=8193\n");
    for i in 0..8191u32 {
        writeln!(
            fixture,
            "{} fixture-{i}.invalid",
            Ipv4Addr::from(0xc6120002 + i)
        )
        .unwrap();
    }
    let mut times = Vec::new();
    for trial in 0..22 {
        fs::write(&path, &fixture).unwrap();
        let pool = FakeIpPool::load_or_new(&path);
        let start = Instant::now();
        let ip = pool.allocate("new.synthetic.invalid");
        let elapsed = start.elapsed().as_nanos();
        assert_eq!(pool.lookup(ip).as_deref(), Some("new.synthetic.invalid"));
        pool.persist().unwrap();
        drop(pool);
        let restored = FakeIpPool::load_or_new(&path);
        assert_eq!(
            restored.lookup(ip).as_deref(),
            Some("new.synthetic.invalid")
        );
        assert_eq!(restored.allocate("new.synthetic.invalid"), ip);
        drop(restored);
        if trial >= 2 {
            times.push(elapsed);
        }
    }
    times.sort_unstable();
    println!(
        "samples={} median_us={:.3} p95_us={:.3} mapping_roundtrips=22",
        times.len(),
        (times[9] + times[10]) as f64 / 2000.0,
        times[18] as f64 / 1000.0
    );
    fs::remove_dir_all(directory).unwrap();
}

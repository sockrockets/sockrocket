//! Sweep-test every node in the local GUI state for real availability:
//! create_outbound → connect → HTTP/1.1 GET www.gstatic.com/generate_204 →
//! expect an HTTP response. Lazy-verifying protocols (VMess) are only proven
//! by an actual round trip, so connect() alone is never enough.
//!
//! Run with:
//!   cargo test -p sockrocket-core --test node_sweep -- --ignored --nocapture

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use sockrocket_core::config::model::AppConfig;
use sockrocket_core::proxy::connector::Outbound;
use sockrocket_core::proxy::factory::create_outbound;

const CONCURRENCY: usize = 8;
const PER_NODE_TIMEOUT: Duration = Duration::from_secs(25);

async fn probe_node(node: sockrocket_core::config::model::Node) -> (String, Result<u128, String>) {
    let name = node.name.clone();
    let start = Instant::now();
    let result = tokio::time::timeout(PER_NODE_TIMEOUT, async {
        let outbound = create_outbound(&node).map_err(|e| format!("config: {e:#}"))?;
        let mut stream = outbound
            .connect("www.gstatic.com", 80)
            .await
            .map_err(|e| format!("connect: {e:#}"))?;
        stream
            .write_all(
                b"GET /generate_204 HTTP/1.1\r\nHost: www.gstatic.com\r\nConnection: close\r\n\r\n",
            )
            .await
            .map_err(|e| format!("write: {e:#}"))?;
        // Bounded read: first bytes must look like an HTTP response.
        let mut buf = [0u8; 64];
        let mut got = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_secs(3), stream.read(&mut buf)).await {
                Ok(Ok(0)) => break,
                Ok(Ok(n)) => {
                    got.extend_from_slice(&buf[..n]);
                    if got.len() >= 12 {
                        break;
                    }
                }
                Ok(Err(e)) => return Err(format!("read: {e:#}")),
                Err(_) => return Err("read: timed out".into()),
            }
        }
        let head = String::from_utf8_lossy(&got);
        if head.contains("HTTP/1.") {
            Ok(())
        } else {
            Err(format!("bad response: {:?}", &head[..head.len().min(40)]))
        }
    })
    .await;
    let ms = start.elapsed().as_millis();
    match result {
        Ok(Ok(())) => (name, Ok(ms)),
        Ok(Err(e)) => (name, Err(e)),
        Err(_) => (name, Err("overall timeout".into())),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "sweeps all real nodes; needs network"]
async fn sweep_all_nodes() {
    let state_path = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .expect("HOME")
        .join(".config/sockrocket/gui-state.yaml");
    let content = std::fs::read_to_string(&state_path).expect("read gui-state.yaml");
    let config: AppConfig = serde_yaml::from_str(&content).expect("parse gui-state.yaml");
    println!(
        "loaded {} nodes from {}",
        config.nodes.len(),
        state_path.display()
    );

    let done = Arc::new(AtomicUsize::new(0));
    let ok_count = Arc::new(AtomicUsize::new(0));
    let total = config.nodes.len();

    let mut set = tokio::task::JoinSet::new();
    let sem = Arc::new(tokio::sync::Semaphore::new(CONCURRENCY));
    for node in config.nodes {
        let permit = sem.clone().acquire_owned().await.unwrap();
        let done = done.clone();
        let ok_count = ok_count.clone();
        set.spawn(async move {
            let _permit = permit;
            let (name, result) = probe_node(node).await;
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            match &result {
                Ok(ms) => {
                    ok_count.fetch_add(1, Ordering::Relaxed);
                    println!("[{n}/{total}] OK   {ms:>6}ms  {name}");
                }
                Err(e) => println!("[{n}/{total}] FAIL          {name}: {e}"),
            }
            (name, result)
        });
    }

    let mut failures = Vec::new();
    let mut latencies = Vec::new();
    while let Some(res) = set.join_next().await {
        let (name, result) = res.expect("probe task panicked");
        match result {
            Ok(ms) => latencies.push((ms, name)),
            Err(e) => failures.push((name, e)),
        }
    }

    latencies.sort();
    println!("\n===== SUMMARY =====");
    println!(
        "total: {}, ok: {}, failed: {}",
        total,
        latencies.len(),
        failures.len()
    );
    if !latencies.is_empty() {
        println!(
            "latency: min {}ms / median {}ms / max {}ms",
            latencies.first().unwrap().0,
            latencies[latencies.len() / 2].0,
            latencies.last().unwrap().0
        );
    }
    println!("\nFastest 10:");
    for (ms, name) in latencies.iter().take(10) {
        println!("  {ms:>6}ms  {name}");
    }
    println!("\nFailures by cause:");
    let mut causes: std::collections::HashMap<String, Vec<String>> = Default::default();
    for (name, e) in &failures {
        let key = e.chars().take(60).collect::<String>();
        causes.entry(key).or_default().push(name.clone());
    }
    for (cause, names) in &causes {
        println!("  [{}x] {} -> {}", names.len(), cause, names.join(", "));
    }
}

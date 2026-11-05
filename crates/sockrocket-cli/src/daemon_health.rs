//! Fresh watchdog evidence from the running daemon, independent of local listeners.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
struct Sample {
    identity: String,
    at: u64,
    ok: bool,
    domestic_http_ok: bool,
}
fn path(pid: u32) -> PathBuf {
    std::env::temp_dir().join(format!("sockrocket_watchdog_{pid}.json"))
}
fn identity(pid: u32) -> Result<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let start = stat
        .rsplit_once(')')
        .and_then(|(_, s)| s.split_whitespace().nth(19))
        .ok_or_else(|| anyhow::anyhow!("Invalid process identity"))?;
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    Ok(format!("{}:{start}", boot.trim()))
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn validate(s: &Sample, id: &str, at: u64) -> Result<()> {
    ensure!(s.identity == id, "Health sample belongs to another process");
    ensure!(s.at <= at && at - s.at <= 90, "Health sample is stale");
    ensure!(s.ok, "Daemon outbound probe failed");
    Ok(())
}
pub fn check(pid: u32) -> Result<()> {
    let id = identity(pid)?;
    let bytes = std::fs::read(path(pid))?;
    ensure!(bytes.len() <= 4096, "Invalid health sample");
    validate(&serde_json::from_slice(&bytes)?, &id, now())
}
/// Domestic HTTP reachability through the daemon's actual routing/DNS policy.
/// Kept separate from proxy health; never used to withdraw LAN routing.
async fn domestic_http_check(outbound: &sockrocket_core::SharedOutbound) -> bool {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for host in ["www.baidu.com", "www.qq.com"] {
        let probe = async {
            let mut stream = outbound.connect(host, 80).await?;
            stream
                .write_all(
                    format!("HEAD / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n")
                        .as_bytes(),
                )
                .await?;
            let mut line = Vec::new();
            while line.len() < 1024 {
                let b = stream.read_u8().await?;
                line.push(b);
                if b == b'\n' {
                    break;
                }
            }
            let mut words = std::str::from_utf8(&line)?.split_whitespace();
            ensure!(
                matches!(words.next(), Some("HTTP/1.0" | "HTTP/1.1")),
                "Invalid domestic response"
            );
            let code: u16 = words.next().unwrap_or("").parse()?;
            ensure!((200..500).contains(&code), "Domestic HTTP request failed");
            Ok::<_, anyhow::Error>(())
        };
        if tokio::time::timeout(std::time::Duration::from_secs(5), probe)
            .await
            .is_ok_and(|r| r.is_ok())
        {
            return true;
        }
    }
    false
}
pub fn summary(pid: u32) -> serde_json::Value {
    let result = (|| -> Result<_> {
        let id = identity(pid)?;
        let bytes = std::fs::read(path(pid))?;
        ensure!(bytes.len() <= 4096, "Invalid health sample");
        let sample: Sample = serde_json::from_slice(&bytes)?;
        let age = now()
            .checked_sub(sample.at)
            .ok_or_else(|| anyhow::anyhow!("Invalid sample time"))?;
        ensure!(sample.identity == id && age <= 90, "Stale sample");
        Ok(
            serde_json::json!({"outbound_ok":sample.ok,"domestic_http_ok":sample.domestic_http_ok,"age_secs":age}),
        )
    })();
    result.unwrap_or(serde_json::Value::Null)
}
pub fn start(outbound: sockrocket_core::SharedOutbound) {
    let pid = std::process::id();
    let Ok(id) = identity(pid) else {
        return;
    }; // Linux router watchdog only.
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            // The outer deadline also bounds warmup and fallback targets.
            let ok = tokio::time::timeout(
                std::time::Duration::from_secs(20),
                sockrocket_core::http_latency_test(&outbound, 12),
            )
            .await
            .is_ok_and(|result| result.is_ok());
            let domestic_http_ok = domestic_http_check(&outbound).await;
            let sample = Sample {
                identity: id.clone(),
                at: now(),
                ok,
                domestic_http_ok,
            };
            if let Ok(bytes) = serde_json::to_vec(&sample) {
                let tmp = path(pid).with_extension("new");
                if std::fs::write(&tmp, bytes).is_ok() {
                    let _ = std::fs::rename(tmp, path(pid));
                }
            }
        }
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn never_accepts_stale_reused_pid_or_failed_probe() {
        let mut s = Sample {
            identity: "boot:start".into(),
            at: 100,
            ok: true,
            domestic_http_ok: true,
        };
        assert!(validate(&s, "boot:start", 190).is_ok());
        assert!(validate(&s, "boot:start", 191).is_err());
        assert!(validate(&s, "boot:start", 99).is_err());
        assert!(validate(&s, "other:start", 101).is_err());
        s.ok = false;
        assert!(validate(&s, "boot:start", 101).is_err());
    }
}

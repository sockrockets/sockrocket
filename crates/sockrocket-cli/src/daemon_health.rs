//! Fresh watchdog evidence from the running daemon, independent of local listeners.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
struct Sample {
    identity: String,
    at: u64,
    ok: bool,
    direct_dns_ok: bool,
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
/// A separate domestic direct-path check; never used to withdraw LAN routing.
async fn direct_dns_check() -> bool {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for server in ["223.5.5.5", "119.29.29.29"] {
        let probe = async {
            let mut stream = sockrocket_core::SharedOutbound::direct()
                .connect(server, 53)
                .await?;
            let query = b"\x73\x72\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00\x03www\x05baidu\x03com\x00\x00\x01\x00\x01";
            stream
                .write_all(&(query.len() as u16).to_be_bytes())
                .await?;
            stream.write_all(query).await?;
            let n = stream.read_u16().await? as usize;
            ensure!((12..=4096).contains(&n), "Invalid direct DNS response size");
            let mut reply = vec![0; n];
            stream.read_exact(&mut reply).await?;
            ensure!(
                reply[..2] == query[..2]
                    && reply[2] & 0x80 != 0
                    && reply[3] & 0x0f == 0
                    && u16::from_be_bytes([reply[6], reply[7]]) > 0,
                "Invalid direct DNS response"
            );
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
            serde_json::json!({"outbound_ok":sample.ok,"direct_dns_ok":sample.direct_dns_ok,"age_secs":age}),
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
            let direct_dns_ok = direct_dns_check().await;
            let sample = Sample {
                identity: id.clone(),
                at: now(),
                ok,
                direct_dns_ok,
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
            direct_dns_ok: true,
        };
        assert!(validate(&s, "boot:start", 190).is_ok());
        assert!(validate(&s, "boot:start", 191).is_err());
        assert!(validate(&s, "boot:start", 99).is_err());
        assert!(validate(&s, "other:start", 101).is_err());
        s.ok = false;
        assert!(validate(&s, "boot:start", 101).is_err());
    }
}

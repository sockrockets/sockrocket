//! Durable private configuration replacement shared by CLI and desktop.
use anyhow::Result;
use std::fs;

/// Shared cross-process lock for configuration read/merge/commit transactions.
pub struct MutationGuard {
    #[cfg(any(unix, windows))]
    _file: fs::File,
    #[cfg(not(any(unix, windows)))]
    _guard: std::sync::MutexGuard<'static, ()>,
}
impl MutationGuard {
    pub fn acquire(path: &std::path::Path) -> Result<Self> {
        #[cfg(unix)]
        {
            use std::os::{
                fd::AsRawFd,
                unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
            };
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| std::path::Path::new("."));
            let file = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(parent.join(".config.lock"))?;
            let metadata = file.metadata()?;
            anyhow::ensure!(
                metadata.is_file() && metadata.nlink() == 1,
                "Configuration lock is not a private regular file"
            );
            let owner = fs::metadata(parent)?;
            if metadata.uid() != owner.uid() && unsafe { libc::geteuid() } == 0 {
                anyhow::ensure!(
                    unsafe { libc::fchown(file.as_raw_fd(), owner.uid(), owner.gid()) } == 0,
                    "Cannot preserve configuration lock ownership"
                );
            }
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
            loop {
                if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                    return Ok(Self { _file: file });
                }
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::WouldBlock {
                    return Err(error.into());
                }
                anyhow::ensure!(
                    std::time::Instant::now() < deadline,
                    "Configuration transaction busy"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| std::path::Path::new("."));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
            loop {
                match fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .share_mode(0)
                    .open(parent.join(".config.lock"))
                {
                    Ok(file) => return Ok(Self { _file: file }),
                    Err(error) if matches!(error.raw_os_error(), Some(32 | 33)) => {
                        anyhow::ensure!(
                            std::time::Instant::now() < deadline,
                            "Configuration transaction busy"
                        );
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
            Ok(Self {
                _guard: LOCK
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Configuration lock poisoned"))?,
            })
        }
    }
}

/// Fetches happen outside this transaction. Re-read current sources before
/// merging results so a source deleted during download cannot reappear.
pub fn commit_subscription_refresh(
    path: &std::path::Path,
    refreshed: Vec<(String, Vec<super::model::Node>)>,
) -> Result<super::model::AppConfig> {
    let _guard = MutationGuard::acquire(path)?;
    let mut current = super::model::AppConfig::from_yaml_str(&fs::read_to_string(path)?)?;
    if super::dedup::apply_subscription_refresh(&mut current, refreshed) {
        write_nodes_and_subscriptions(path, &current)?;
    }
    Ok(current)
}

pub fn atomic_write_private(
    path: &std::path::Path,
    text: &str,
    backup_existing: bool,
) -> Result<()> {
    use std::io::Write;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = path.with_extension(format!(
        "yaml.tmp.{}.{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        // An elevated desktop shares the interactive user's config directory.
        // Keep its private files readable by that user after returning to a
        // non-elevated launch; router-owned directories remain root-owned.
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let parent = fs::metadata(path.parent().unwrap())?;
            let owner = (parent.uid(), parent.gid());
            if file.metadata()?.uid() == 0 && owner.0 != 0 {
                use std::os::fd::AsRawFd;
                anyhow::ensure!(
                    unsafe { libc::fchown(file.as_raw_fd(), owner.0, owner.1) } == 0,
                    "Cannot preserve private configuration ownership"
                );
            }
        }
        let backup = path.with_extension("yaml.bak");
        if backup_existing && path.exists() {
            // Backups require the same atomic replacement guarantees as
            // the primary; copying directly can truncate the only recovery
            // snapshot on a crash, and can follow a stale backup symlink.
            let previous = fs::read_to_string(path)?;
            atomic_write_private(&backup, &previous, false)?;
        }
        fs::rename(&tmp, path)?;
        #[cfg(unix)]
        {
            fs::File::open(path.parent().unwrap())?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}

/// Replace `nodes`, `subscriptions`, and `active_node` in an existing config
/// file, leaving every other key (router DNS, mode, comments-as-keys) in place.
pub fn write_nodes_and_subscriptions(
    path: &std::path::Path,
    config: &crate::config::model::AppConfig,
) -> Result<()> {
    let content = fs::read_to_string(path)?;
    let mut value: serde_yaml::Value = serde_yaml::from_str(&content)?;
    let Some(map) = value.as_mapping_mut() else {
        anyhow::bail!("config root is not a mapping");
    };
    map.insert(
        serde_yaml::Value::String("nodes".into()),
        serde_yaml::to_value(&config.nodes)?,
    );
    map.insert(
        serde_yaml::Value::String("subscriptions".into()),
        serde_yaml::to_value(&config.subscriptions)?,
    );
    map.insert(
        serde_yaml::Value::String("active_node".into()),
        serde_yaml::to_value(config.active_node)?,
    );
    atomic_write_private(path, &serde_yaml::to_string(&value)?, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn subscription_and_nodes_survive_private_file_reopen() {
        use crate::config::model::{AppConfig, Node, ProxyProtocol, Subscription};
        let dir =
            std::env::temp_dir().join(format!("sockrocket-roundtrip-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.yaml");
        let mut config = AppConfig::default();
        config.subscriptions.push(Subscription {
            name: "Example".into(),
            url: "https://example.com/sub/demo".into(),
            format: "auto".into(),
            last_updated: None,
            refresh_interval_hours: 24,
        });
        config.nodes.push(Node {
            name: "Example A".into(),
            server: "192.0.2.1".into(),
            port: 443,
            protocol: ProxyProtocol::Shadowsocks {
                cipher: "aes-128-gcm".into(),
                password: "test-only".into(),
                udp: true,
                shadow_tls: None,
            },
            transport: None,
            latency_ms: None,
            tags: vec![],
            extra: Default::default(),
        });
        config.active_node = Some(0);
        atomic_write_private(&path, &serde_yaml::to_string(&config).unwrap(), true).unwrap();
        let reopened: AppConfig =
            serde_yaml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(reopened.subscriptions[0].url, config.subscriptions[0].url);
        assert_eq!(reopened.nodes[0].server, "192.0.2.1");
        assert_eq!(reopened.active_node, Some(0));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn replacement_is_complete_private_and_keeps_last_version() {
        let dir = std::env::temp_dir().join(format!("sockrocket-storage-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.yaml");
        atomic_write_private(&path, "nodes: []\n", true).unwrap();
        atomic_write_private(&path, "nodes: []\nactive_node: null\n", true).unwrap();
        assert_eq!(
            fs::read_to_string(path.with_extension("yaml.bak")).unwrap(),
            "nodes: []\n"
        );
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "nodes: []\nactive_node: null\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o077, 0);
        }
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn subscription_update_preserves_unrelated_keys() {
        use crate::config::model::{AppConfig, Subscription};
        let dir =
            std::env::temp_dir().join(format!("sockrocket-sub-persist-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        fs::write(
            &path,
            "dns_port: 5300\nmode: \"tun\"\nsubscriptions: []\nnodes: []\nactive_node: null\n",
        )
        .unwrap();
        let mut config = AppConfig::default();
        config.subscriptions.push(Subscription {
            name: "Example".into(),
            url: "https://example.com/sub/demo".into(),
            format: "auto".into(),
            last_updated: Some(1),
            refresh_interval_hours: 24,
        });
        config.nodes.push(node_for_persist());
        config.active_node = Some(0);
        write_nodes_and_subscriptions(&path, &config).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("dns_port:"));
        assert!(text.contains("5300"));
        assert!(text.contains("mode:"));
        assert!(text.contains("https://example.com/sub/demo"));
        assert!(text.contains("192.0.2.1"));
        fs::remove_dir_all(dir).unwrap();
    }

    fn node_for_persist() -> crate::config::model::Node {
        use crate::config::model::{Node, ProxyProtocol};
        Node {
            name: "Example A".into(),
            server: "192.0.2.1".into(),
            port: 443,
            protocol: ProxyProtocol::Shadowsocks {
                cipher: "aes-128-gcm".into(),
                password: "test-only".into(),
                udp: true,
                shadow_tls: None,
            },
            transport: None,
            latency_ms: None,
            tags: vec![],
            extra: Default::default(),
        }
    }
}

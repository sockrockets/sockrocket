//! Durable private configuration replacement shared by CLI and desktop.
use anyhow::Result;
use std::fs;

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
                std::os::unix::fs::chown(&tmp, Some(owner.0), Some(owner.1))?;
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
        serde_yaml::to_value(&config.active_node)?,
    );
    atomic_write_private(path, &serde_yaml::to_string(&value)?, true)
}

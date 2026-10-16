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
        let backup = path.with_extension("yaml.bak");
        if backup_existing && path.exists() {
            fs::copy(path, &backup)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&backup, fs::Permissions::from_mode(0o600))?;
            }
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

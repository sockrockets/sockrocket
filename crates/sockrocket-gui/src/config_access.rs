//! Compatibility repair for private files created by an older elevated GUI.
use anyhow::Result;
use std::path::Path;

#[cfg(target_os = "macos")]
pub fn repair_owned_files() -> Result<()> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("Missing home directory"))?;
    repair_owned_files_at(Path::new(&home))
}

#[cfg(target_os = "macos")]
fn repair_owned_files_at(home: &Path) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let owner = std::fs::metadata(home)?;
    anyhow::ensure!(owner.uid() != 0, "Interactive user home is required");
    let dir = home.join("Library/Application Support/sockrocket");
    let metadata = std::fs::symlink_metadata(&dir)?;
    anyhow::ensure!(
        metadata.is_dir() && (metadata.uid() == 0 || metadata.uid() == owner.uid()),
        "Unexpected configuration directory"
    );
    if metadata.uid() == 0 {
        std::os::unix::fs::chown(&dir, Some(owner.uid()), Some(owner.gid()))?;
    }
    for name in [
        "gui-state.yaml",
        "gui-state.yaml.bak",
        "system-proxy-owner.yaml",
        "system-proxy-owner.lock",
        "tun-routes.yaml",
        "tun-routes.lock",
    ] {
        let path = dir.join(name);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        anyhow::ensure!(
            metadata.is_file() && (metadata.uid() == 0 || metadata.uid() == owner.uid()),
            "Unexpected configuration file"
        );
        if metadata.uid() == 0 {
            std::os::unix::fs::chown(&path, Some(owner.uid()), Some(owner.gid()))?;
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn ensure_readable(path: &Path) -> Result<()> {
    // Only the known legacy ownership failure warrants an OS authorization
    // dialog. Missing/corrupt files and arbitrary access errors do not.
    use std::os::unix::fs::MetadataExt;
    if !matches!(std::fs::read(path), Err(ref error) if error.kind() == std::io::ErrorKind::PermissionDenied)
    {
        return Ok(());
    }
    let metadata = std::fs::symlink_metadata(path)?;
    anyhow::ensure!(
        metadata.is_file() && metadata.uid() == 0,
        "Configuration is not a legacy administrator-owned file"
    );
    let quote = |text: &str| format!("'{}'", text.replace('\'', "'\\''"));
    let home = std::env::var("HOME")?;
    let executable = std::env::current_exe()?;
    let command = format!(
        "HOME={} {} --repair-config-permissions",
        quote(&home),
        quote(&executable.to_string_lossy())
    );
    let script = format!(
        "do shell script \"{}\" with administrator privileges",
        command.replace('\\', "\\\\").replace('"', "\\\"")
    );
    let status = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", &script])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    anyhow::ensure!(
        status.success(),
        "Configuration repair authorization was cancelled or failed"
    );
    std::fs::File::open(path)?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn ensure_readable(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    fn fixture() -> std::path::PathBuf {
        let home = std::env::temp_dir().join(format!("sockrocket-access-{}", std::process::id()));
        let home = home.join(format!(
            "{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(home.join("Library/Application Support/sockrocket")).unwrap();
        home
    }
    #[test]
    fn permission_repair_preserves_configuration_and_backup_bytes() {
        let home = fixture();
        let dir = home.join("Library/Application Support/sockrocket");
        for name in ["gui-state.yaml", "gui-state.yaml.bak"] {
            std::fs::write(dir.join(name), b"private synthetic fixture").unwrap();
        }
        repair_owned_files_at(&home).unwrap();
        for name in ["gui-state.yaml", "gui-state.yaml.bak"] {
            assert_eq!(
                std::fs::read(dir.join(name)).unwrap(),
                b"private synthetic fixture"
            );
            assert_eq!(
                std::fs::metadata(dir.join(name))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o077,
                0
            );
        }
        std::fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn permission_repair_rejects_symlink_without_changing_target() {
        let home = fixture();
        let target = home.join("unrelated.yaml");
        std::fs::write(&target, b"unchanged").unwrap();
        let old_mode = std::fs::metadata(&target).unwrap().permissions().mode();
        symlink(
            &target,
            home.join("Library/Application Support/sockrocket/gui-state.yaml"),
        )
        .unwrap();
        assert!(repair_owned_files_at(&home).is_err());
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode(),
            old_mode
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"unchanged");
        std::fs::remove_dir_all(home).unwrap();
    }
}

//! Private, bounded watchdog storage. Never trust pre-created paths in /tmp.
use super::Sample;
use anyhow::{Result, ensure};
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

const MAX_SAMPLE: u64 = 4096;

fn root() -> PathBuf {
    // A separate directory per effective UID keeps elevated and ordinary
    // processes independent. The shared temp parent is never chmod/chowned.
    std::env::temp_dir().join(format!("sockrocket-watchdog-{}", unsafe {
        libc::geteuid()
    }))
}
fn directory(root: &Path, create: bool) -> Result<()> {
    if create {
        match fs::DirBuilder::new().mode(0o700).create(root) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    let metadata = fs::symlink_metadata(root)?;
    ensure!(
        metadata.is_dir(),
        "Watchdog directory is not a real directory"
    );
    ensure!(
        metadata.uid() == unsafe { libc::geteuid() },
        "Watchdog directory owner differs"
    );
    ensure!(
        metadata.mode() & 0o077 == 0,
        "Watchdog directory is not private"
    );
    Ok(())
}
fn path(root: &Path, pid: u32) -> PathBuf {
    root.join(format!("{pid}.json"))
}
fn read_at(root: &Path, pid: u32) -> Result<Sample> {
    directory(root, false)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path(root, pid))?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.nlink() == 1,
        "Watchdog sample is not a private regular file"
    );
    ensure!(
        metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o077 == 0,
        "Watchdog sample permissions are not private"
    );
    ensure!(metadata.len() <= MAX_SAMPLE, "Watchdog sample is too large");
    let mut bytes = Vec::new();
    file.take(MAX_SAMPLE + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_SAMPLE,
        "Watchdog sample is too large"
    );
    Ok(serde_json::from_slice(&bytes)?)
}
fn write_at(root: &Path, pid: u32, sample: &Sample) -> Result<()> {
    directory(root, true)?;
    let json = serde_json::to_string(sample)?;
    ensure!(
        json.len() as u64 <= MAX_SAMPLE,
        "Watchdog sample is too large"
    );
    // The shared writer exclusively creates a 0600 temporary file and renames
    // it atomically; an existing final symlink is replaced, never followed.
    sockrocket_core::config::storage::atomic_write_private(&path(root, pid), &json, false)
}
pub(super) fn read(pid: u32) -> Result<Sample> {
    read_at(&root(), pid)
}
pub(super) fn write(pid: u32, sample: &Sample) -> Result<()> {
    write_at(&root(), pid, sample)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "sockrocket-watchdog-test-{:032x}",
                rand::random::<u128>()
            ));
            fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
            Self(root)
        }
        fn sample() -> Sample {
            Sample {
                identity: "synthetic-process".into(),
                at: 100,
                ok: true,
                domestic_http_ok: true,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn round_trip_has_private_permissions_and_atomic_replacement() {
        let f = Fixture::new();
        let store = f.0.join("store");
        write_at(&store, 42, &Fixture::sample()).unwrap();
        assert!(read_at(&store, 42).unwrap().ok);
        assert_eq!(fs::metadata(&store).unwrap().mode() & 0o777, 0o700);
        assert_eq!(
            fs::metadata(path(&store, 42)).unwrap().mode() & 0o777,
            0o600
        );
        let mut changed = Fixture::sample();
        changed.ok = false;
        write_at(&store, 42, &changed).unwrap();
        assert!(!read_at(&store, 42).unwrap().ok);
    }
    #[test]
    fn rejects_symlinked_and_permissive_directories() {
        let f = Fixture::new();
        let target = f.0.join("target");
        fs::create_dir(&target).unwrap();
        let link = f.0.join("store");
        symlink(&target, &link).unwrap();
        assert!(write_at(&link, 42, &Fixture::sample()).is_err());
        assert!(read_at(&link, 42).is_err());
        assert_eq!(fs::read_dir(&target).unwrap().count(), 0);
        fs::set_permissions(&target, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(write_at(&target, 42, &Fixture::sample()).is_err());
        assert_eq!(fs::metadata(&target).unwrap().mode() & 0o777, 0o777);
    }
    #[test]
    fn symlinks_never_read_or_overwrite_their_targets() {
        let f = Fixture::new();
        let store = f.0.join("store");
        directory(&store, true).unwrap();
        let target = f.0.join("keep");
        fs::write(&target, b"do not change").unwrap();
        let sample = path(&store, 42);
        symlink(&target, &sample).unwrap();
        symlink(&target, sample.with_extension("new")).unwrap();
        assert!(read_at(&store, 42).is_err());
        write_at(&store, 42, &Fixture::sample()).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"do not change");
        assert!(read_at(&store, 42).unwrap().ok);
    }
    #[test]
    fn rejects_oversized_and_hardlinked_samples() {
        let f = Fixture::new();
        let store = f.0.join("store");
        write_at(&store, 42, &Fixture::sample()).unwrap();
        let sample = path(&store, 42);
        fs::hard_link(&sample, f.0.join("copy")).unwrap();
        assert!(read_at(&store, 42).is_err());
        fs::remove_file(f.0.join("copy")).unwrap();
        OpenOptions::new()
            .write(true)
            .open(&sample)
            .unwrap()
            .set_len(1024 * 1024 * 1024)
            .unwrap();
        assert!(read_at(&store, 42).is_err());
    }
}

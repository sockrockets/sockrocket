//! Cross-process configuration transactions and durable atomic replacement.
use super::*;

pub(super) struct MutationGuard {
    #[cfg(unix)]
    _file: fs::File,
    #[cfg(not(unix))]
    _guard: std::sync::MutexGuard<'static, ()>,
}
impl MutationGuard {
    pub fn acquire() -> Result<Self> {
        #[cfg(unix)]
        {
            use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
            unsafe extern "C" {
                fn flock(fd: i32, operation: i32) -> i32;
            }
            let file = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .open(sockrocket_dir().join(".config.lock"))?;
            let start = std::time::Instant::now();
            loop {
                if unsafe { flock(file.as_raw_fd(), 2 | 4) } == 0 {
                    return Ok(Self { _file: file });
                }
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::WouldBlock {
                    return Err(error.into());
                }
                anyhow::ensure!(
                    start.elapsed() < Duration::from_secs(15),
                    "Configuration transaction busy"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        #[cfg(not(unix))]
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

pub(super) fn atomic_write(text: &str) -> Result<()> {
    sockrocket_core::config::storage::atomic_write_private(&conf_path(), text, true)
}

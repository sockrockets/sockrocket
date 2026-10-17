//! Durable ownership record for desktop system-proxy changes.
use std::path::Path;
use std::sync::Mutex;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::system_proxy::{self, SystemProxyConfig};

static TRANSACTION: Mutex<()> = Mutex::new(());

#[derive(Serialize, Deserialize)]
struct Journal {
    owner_pid: u32,
    previous: SystemProxyConfig,
    applied: SystemProxyConfig,
}

fn read(path: &Path) -> Result<Option<Journal>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(serde_yaml::from_str(&text)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn same_proxy(a: &SystemProxyConfig, b: &SystemProxyConfig) -> bool {
    let bypass = |text: &str| {
        let mut entries: Vec<_> = text
            .split([',', ';'])
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_lowercase)
            .collect();
        entries.sort();
        entries
    };
    a.enabled == b.enabled
        && (!a.enabled
            || (a.host.eq_ignore_ascii_case(&b.host)
                && a.port == b.port
                && bypass(&a.bypass) == bypass(&b.bypass)))
}

/// Write recovery state before changing OS settings. A failed write prevents
/// taking ownership, rather than leaving an unrecorded proxy behind.
pub fn enable(path: &Path, host: &str, port: u16) -> Result<()> {
    let _lock = TRANSACTION
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let _process_lock = journal_lock(path)?;
    let current = system_proxy::get_system_proxy()?;
    let previous = match read(path)? {
        Some(journal) if same_proxy(&current, &journal.applied) => {
            anyhow::ensure!(
                journal.owner_pid == std::process::id(),
                "System proxy is managed by another running instance"
            );
            journal.previous
        }
        _ => current,
    };
    let host = if matches!(host, "0.0.0.0" | "::") {
        "127.0.0.1"
    } else {
        host.trim()
    };
    anyhow::ensure!(!host.is_empty() && port != 0, "Invalid proxy endpoint");
    let journal = Journal {
        owner_pid: std::process::id(),
        previous,
        applied: SystemProxyConfig {
            enabled: true,
            host: host.into(),
            port,
            bypass: system_proxy::PLATFORM_BYPASS.into(),
        },
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::config::storage::atomic_write_private(path, &serde_yaml::to_string(&journal)?, false)?;
    // Keep the journal even on OS failure: a platform backend may have applied
    // only part of its changes, and the next recovery must retain evidence.
    system_proxy::set_system_proxy(host, port)?;
    anyhow::ensure!(
        same_proxy(&system_proxy::get_system_proxy()?, &journal.applied),
        "System proxy change was not confirmed; recovery record retained"
    );
    Ok(())
}

/// Restore only while the observed settings still match our recorded values.
/// A user/other application change takes precedence over our old snapshot.
pub fn restore(path: &Path) -> Result<()> {
    restore_owned(path, Some(std::process::id()))
}

fn restore_owned(path: &Path, expected_owner: Option<u32>) -> Result<()> {
    let _lock = TRANSACTION
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let _process_lock = journal_lock(path)?;
    let Some(journal) = read(path)? else {
        return Ok(());
    };
    if expected_owner.is_some_and(|owner| journal.owner_pid != owner) {
        return Ok(());
    }
    if expected_owner.is_none() && listener_alive(&journal) {
        return Ok(());
    }
    let current = system_proxy::get_system_proxy()?;
    restore_record(path, &current, journal, |previous| {
        if previous.enabled {
            system_proxy::set_system_proxy_with_bypass(
                &previous.host,
                previous.port,
                &previous.bypass,
            )?;
            if let Err(error) = system_proxy::flush_system_dns_cache() {
                tracing::warn!("DNS invalidation after proxy restoration failed: {error}");
            }
        } else {
            system_proxy::clear_system_proxy()?;
        }
        anyhow::ensure!(
            same_proxy(&system_proxy::get_system_proxy()?, previous),
            "System proxy restoration was not confirmed; recovery record retained"
        );
        Ok(())
    })
}

fn restore_record(
    path: &Path,
    current: &SystemProxyConfig,
    journal: Journal,
    apply: impl FnOnce(&SystemProxyConfig) -> Result<()>,
) -> Result<()> {
    if same_proxy(current, &journal.applied) {
        apply(&journal.previous)?;
    }
    std::fs::remove_file(path)?;
    Ok(())
}

/// Startup recovery must not interfere with a proxy that is still listening
/// (for example another running instance).
pub fn recover_if_stale(path: &Path) -> Result<()> {
    restore_owned(path, None)
}

/// Called only after the independent guardian observes its owner's pipe close.
pub(crate) fn restore_after_exit(path: &Path, owner: u32) -> Result<()> {
    restore_owned(path, Some(owner))
}

fn listener_alive(journal: &Journal) -> bool {
    let host = journal.applied.host.as_str();
    if !matches!(host, "127.0.0.1" | "localhost" | "::1") {
        return true;
    }
    let ip = if host == "::1" {
        std::net::Ipv6Addr::LOCALHOST.into()
    } else {
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
    };
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::new(ip, journal.applied.port),
        std::time::Duration::from_millis(150),
    )
    .is_ok()
}

pub(crate) fn journal_lock(path: &Path) -> Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path.with_extension("lock"))?;
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "Recovery lock is not a regular file"
    );
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = file.metadata()?;
        anyhow::ensure!(
            metadata.nlink() == 1,
            "Recovery lock has unexpected hard links"
        );
        if let Some(parent) = path.parent() {
            let owner = std::fs::metadata(parent)?;
            if metadata.uid() != owner.uid() && unsafe { libc::geteuid() } == 0 {
                // Use the open descriptor, not a pathname an unprivileged
                // process could replace between validation and ownership repair.
                let result = unsafe { libc::fchown(file.as_raw_fd(), owner.uid(), owner.gid()) };
                if result != 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
            }
        }
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(25))
            }
            Err(error) => return Err(anyhow::anyhow!("Proxy recovery lock unavailable: {error}")),
        }
    }
}

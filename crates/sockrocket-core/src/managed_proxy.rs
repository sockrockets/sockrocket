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
    system_proxy::set_system_proxy(host, port)
}

/// Restore only while the observed settings still match our recorded values.
/// A user/other application change takes precedence over our old snapshot.
pub fn restore(path: &Path) -> Result<()> {
    restore_owned(path, false)
}

fn restore_owned(path: &Path, recover_stale: bool) -> Result<()> {
    let _lock = TRANSACTION
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Some(journal) = read(path)? else {
        return Ok(());
    };
    if !recover_stale && journal.owner_pid != std::process::id() {
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
    let Some(journal) = read(path)? else {
        return Ok(());
    };
    let host = journal.applied.host.as_str();
    if !matches!(host, "127.0.0.1" | "localhost" | "::1") {
        return Ok(());
    }
    let ip: std::net::IpAddr = if host == "::1" {
        "::1".parse()?
    } else {
        "127.0.0.1".parse()?
    };
    if std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::new(ip, journal.applied.port),
        std::time::Duration::from_millis(150),
    )
    .is_err()
    {
        restore_owned(path, true)?;
    }
    Ok(())
}

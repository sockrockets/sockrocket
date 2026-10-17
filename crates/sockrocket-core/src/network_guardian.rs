//! A separate process survives abrupt termination of the desktop owner.
//! No network endpoint, subscription or credential is passed on the command line.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;

const FLAG: &str = "--network-guardian";
static GUARDIAN: Mutex<Option<Handle>> = Mutex::new(None);
struct Handle {
    child: Child,
    _pipe: ChildStdin,
}
#[derive(Serialize, Deserialize)]
struct Recovery {
    owner: u32,
    proxy: PathBuf,
    routes: PathBuf,
}

/// Must succeed before taking ownership of system networking settings.
pub fn ensure(proxy: PathBuf, routes: PathBuf) -> Result<()> {
    let mut guard = GUARDIAN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(handle) = guard.as_mut() {
        if handle.child.try_wait()?.is_none() {
            return Ok(());
        }
    }
    *guard = None;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg(FLAG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000 | 0x00000200);
    }
    let mut child = command.spawn().context("Start network recovery guardian")?;
    let mut pipe = child.stdin.take().context("Guardian input unavailable")?;
    let recovery = Recovery {
        owner: std::process::id(),
        proxy,
        routes,
    };
    if let Err(error) = (|| -> Result<()> {
        serde_json::to_writer(&mut pipe, &recovery)?;
        pipe.write_all(b"\n")?;
        pipe.flush()?;
        let output = child.stdout.take().context("Guardian output unavailable")?;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = output;
            let mut reply = [0u8; 6];
            let result = reader
                .read_exact(&mut reply)
                .map(|_| String::from_utf8_lossy(&reply).into_owned());
            let _ = tx.send(result);
        });
        let reply = rx.recv_timeout(std::time::Duration::from_secs(5))??;
        anyhow::ensure!(
            reply == "READY\n",
            "Network recovery guardian did not become ready"
        );
        Ok(())
    })() {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    *guard = Some(Handle { child, _pipe: pipe });
    Ok(())
}

/// Intercept before GUI/runtime initialization. The pipe is the lifetime token:
/// the OS closes it on normal exit, abort and SIGKILL, without a panic callback.
pub fn run_if_requested() -> Option<Result<()>> {
    if std::env::args_os().nth(1).as_deref() != Some(std::ffi::OsStr::new(FLAG)) {
        return None;
    }
    Some(run())
}
fn run() -> Result<()> {
    let mut input = std::io::BufReader::new(std::io::stdin());
    let mut line = String::new();
    (&mut input).take(65536).read_line(&mut line)?;
    anyhow::ensure!(line.ends_with('\n'), "Invalid guardian initialization");
    let recovery: Recovery = serde_json::from_str(&line)?;
    anyhow::ensure!(recovery.owner != 0, "Invalid guardian owner");
    std::io::stdout().write_all(b"READY\n")?;
    std::io::stdout().flush()?;
    std::io::copy(&mut input, &mut std::io::sink())?;
    crate::set_route_journal_path(recovery.routes);
    crate::proxy::tun::restore_routes_after_exit(recovery.owner);
    let mut last_error = None;
    for _ in 0..3 {
        match crate::managed_proxy::restore_after_exit(&recovery.proxy, recovery.owner) {
            Ok(()) => {
                last_error = None;
                break;
            }
            Err(error) => last_error = Some(error),
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    let _ = crate::flush_system_dns_cache();
    if let Some(error) = last_error {
        return Err(error);
    }
    Ok(())
}

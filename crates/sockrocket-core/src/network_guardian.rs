//! A separate process survives abrupt termination of the desktop owner.
//! No network endpoint, subscription or credential is passed on the command line.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;

const FLAG: &str = "--network-guardian";
static GUARDIAN: Mutex<Option<Handle>> = Mutex::new(None);
struct Handle {
    child: Child,
    _pipe: ChildStdin,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    output: Option<ChildStdout>,
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
    if let Some(handle) = guard.as_mut()
        && handle.child.try_wait()?.is_none()
    {
        return Ok(());
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
    let initialization = (|| -> Result<ChildStdout> {
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
            let _ = tx.send((result, reader));
        });
        let (reply, output) = rx.recv_timeout(std::time::Duration::from_secs(5))?;
        let reply = reply?;
        anyhow::ensure!(
            reply == "READY\n",
            "Network recovery guardian did not become ready"
        );
        Ok(output)
    })();
    let output = match initialization {
        Ok(output) => output,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    *guard = Some(Handle {
        child,
        _pipe: pipe,
        output: Some(output),
    });
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn share_macos_authorization(form: [u8; 32]) -> Result<()> {
    let mut handle = GUARDIAN.lock().unwrap_or_else(|e| e.into_inner());
    let handle = handle.as_mut().context("Network guardian unavailable")?;
    serde_json::to_writer(&mut handle._pipe, &form)?;
    handle._pipe.write_all(b"\n")?;
    handle._pipe.flush()?;
    let mut output = handle
        .output
        .take()
        .context("Guardian acknowledgment unavailable")?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reply = [0u8; 7];
        let result = output.read_exact(&mut reply).map(|_| reply);
        let _ = tx.send((result, output));
    });
    let received = rx.recv_timeout(std::time::Duration::from_secs(5));
    let (result, output) = match received {
        Ok(value) => value,
        Err(error) => {
            let _ = handle.child.kill();
            let _ = handle.child.wait();
            return Err(error.into());
        }
    };
    handle.output = Some(output);
    anyhow::ensure!(
        result? == *b"AUTHOK\n",
        "Recovery authorization was not acknowledged"
    );
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
    #[cfg(target_os = "macos")]
    {
        loop {
            let mut update = String::new();
            let n = (&mut input).take(1024).read_line(&mut update)?;
            if n == 0 {
                break;
            }
            anyhow::ensure!(
                update.ends_with('\n'),
                "Invalid recovery authorization update"
            );
            let form: [u8; 32] = serde_json::from_str(&update)?;
            crate::managed_proxy::macos::import(form)?;
            std::io::stdout().write_all(b"AUTHOK\n")?;
            std::io::stdout().flush()?;
        }
    }
    #[cfg(not(target_os = "macos"))]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_guardian_cannot_restore_new_proxy_owner() {
        let dir =
            std::env::temp_dir().join(format!("sockrocket-guardian-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("proxy.yaml");
        let proxy = crate::SystemProxyConfig {
            enabled: true,
            host: "127.0.0.1".into(),
            port: 1088,
            bypass: String::new(),
        };
        let value = serde_json::json!({"owner_pid": 42, "previous": proxy, "applied": proxy});
        std::fs::write(&path, serde_yaml::to_string(&value).unwrap()).unwrap();
        crate::managed_proxy::restore_after_exit(&path, 41).unwrap();
        assert!(path.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn abrupt_process_death_closes_lifetime_pipe() {
        use std::time::Duration;
        let mut child = Command::new("sh")
            .args(["-c", "exec sleep 30"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut pipe = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut byte = [0];
            let _ = tx.send(pipe.read(&mut byte));
        });
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap(), 0);
    }
}

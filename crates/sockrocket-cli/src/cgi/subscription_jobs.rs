//! Cross-process subscription jobs. Firmware CGI may exit immediately after
//! launch; the worker survives it, while polling stays independent of the
//! configuration transaction and the proxy lifecycle.
use super::*;
use std::{
    cell::RefCell,
    process::Stdio,
    time::{SystemTime, UNIX_EPOCH},
};
thread_local! {static CURRENT:RefCell<Option<String>>=const{RefCell::new(None)};}
fn root() -> PathBuf {
    sockrocket_dir().join(".subscription-jobs")
}
fn valid(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}
fn path(id: &str) -> PathBuf {
    root().join(format!("{id}.json"))
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn read(path: &std::path::Path) -> Result<J> {
    anyhow::ensure!(
        !fs::symlink_metadata(path)?.file_type().is_symlink(),
        "Job state cannot be a symlink"
    );
    anyhow::ensure!(
        fs::metadata(path)?.len() <= 2 * 1024 * 1024,
        "Job state too large"
    );
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
fn write(path: &std::path::Path, state: &J) -> Result<()> {
    sockrocket_core::config::storage::atomic_write_private(
        path,
        &serde_json::to_string(state)?,
        false,
    )
}
fn directory() -> Result<()> {
    fs::create_dir_all(root())?;
    anyhow::ensure!(
        !fs::symlink_metadata(root())?.file_type().is_symlink(),
        "Job directory cannot be a symlink"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root(), fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn alive(pid: u32, id: &str) -> bool {
    #[cfg(target_os = "linux")]
    {
        // A recycled PID must not keep a dead job permanently running.
        let Ok(args) = fs::read(format!("/proc/{pid}/cmdline")) else {
            return false;
        };
        let fields: Vec<_> = args.split(|b| *b == 0).collect();
        fields
            .windows(2)
            .any(|pair| pair[0] == b"--subscription-worker" && pair[1] == id.as_bytes())
    }
    #[cfg(not(unix))]
    let _ = id;
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "command="])
            .output()
            .is_ok_and(|output| {
                let args = String::from_utf8_lossy(&output.stdout);
                output.status.success()
                    && args
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .windows(2)
                        .any(|pair| pair == ["--subscription-worker", id])
            })
    }
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut core::ffi::c_void;
            fn GetExitCodeProcess(handle: *mut core::ffi::c_void, code: *mut u32) -> i32;
            fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
        }
        // Query-only handle; never terminates or changes another process.
        unsafe {
            let handle = OpenProcess(0x1000, 0, pid);
            if handle.is_null() {
                return false;
            }
            let mut code = 0;
            let running = GetExitCodeProcess(handle, &mut code) != 0 && code == 259;
            CloseHandle(handle);
            running
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}
fn load(id: &str) -> Result<J> {
    anyhow::ensure!(valid(id), "Invalid subscription job ID");
    let mut state = read(&path(id))?;
    if state["state"] == "running" {
        let pid = state["pid"].as_u64().unwrap_or(0) as u32;
        let age = now().saturating_sub(state["created"].as_u64().unwrap_or(0));
        if (pid > 0 && !alive(pid, id)) || (pid == 0 && age > 30) {
            state["state"] = json!("finished");
            state["result"] = json!({"ok":false,"msg":"Subscription worker exited; saved subscriptions and nodes were retained"});
            let _ = fs::remove_file(root().join(format!("{id}.input")));
            write(&path(id), &state)?;
        }
    }
    Ok(state)
}
fn active() -> Option<String> {
    read(&root().join("active.json"))
        .ok()?
        .get("id")?
        .as_str()
        .filter(|id| valid(id))
        .map(str::to_owned)
}
fn trim_history() {
    let Ok(entries) = fs::read_dir(root()) else {
        return;
    };
    let mut files: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let id = name.strip_suffix(".json")?;
            valid(id).then(|| {
                (
                    entry.metadata().ok().and_then(|m| m.modified().ok()),
                    id.to_string(),
                )
            })
        })
        .collect();
    files.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    for (_, id) in files.into_iter().skip(7) {
        let _ = fs::remove_file(path(&id));
        let _ = fs::remove_file(root().join(format!("{id}.input")));
    }
}
pub(super) fn start(post: &J) -> J {
    let action = post["action"].as_str().unwrap_or("");
    if !matches!(action, "add_sub" | "update_subs") {
        return err_msg("Invalid subscription job action");
    }
    let launch = (|| -> Result<J> {
        directory()?;
        let _job_lock =
            sockrocket_core::config::storage::MutationGuard::acquire(&root().join("active.json"))?;
        if let Some(id) = active()
            && let Ok(state) = load(&id)
            && state["state"] == "running"
        {
            return Ok(json!({"ok":true,"job":state,"existing":true}));
        }
        trim_history();
        let id = rand::random::<[u8; 16]>()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let state = json!({"id":id,"state":"running","stage":"queued","completed":0,"total":0,"created":now(),"pid":0});
        write(
            &root().join(format!("{id}.input")),
            &json!({"action":action,"body":post.get("body").unwrap_or(&J::Null)}),
        )?;
        write(&path(&id), &state)?;
        write(&root().join("active.json"), &json!({"id":id}))?;
        let mut command = Command::new(env::current_exe()?);
        command
            .args(["--subscription-worker", &id])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        match command.spawn() {
            Ok(mut child) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(error) => {
                let _ = fs::remove_file(root().join(format!("{id}.input")));
                let failed = json!({"id":id,"state":"finished","result":{"ok":false,"msg":"Cannot launch subscription worker"}});
                write(&path(&id), &failed)?;
                return Err(error.into());
            }
        }
        Ok(json!({"ok":true,"job":state}))
    })();
    launch.unwrap_or_else(|_| {
        err_msg("Cannot start subscription job; existing settings were retained")
    })
}
pub(super) fn status(post: &J) -> J {
    let id = post
        .get("id")
        .and_then(J::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .or_else(active);
    let Some(id) = id else {
        return json!({"ok":true,"job":null});
    };
    let Ok(_job_lock) =
        sockrocket_core::config::storage::MutationGuard::acquire(&root().join("active.json"))
    else {
        return err_msg("Subscription progress is busy; retry shortly");
    };
    match load(&id) {
        Ok(state) => json!({"ok":true,"job":state}),
        Err(_) => err_msg(
            "Subscription job is unavailable; refresh subscriptions to inspect the saved state",
        ),
    }
}
pub(super) fn progress(completed: usize, total: usize, stage: &str) {
    CURRENT.with(|current| {
        if let Some(id) = current.borrow().as_ref()
            && let Ok(mut state) = read(&path(id))
        {
            state["completed"] = json!(completed);
            state["total"] = json!(total);
            state["stage"] = json!(stage);
            let _ = write(&path(id), &state);
        }
    });
}
pub fn worker(id: &str) -> Result<()> {
    directory()?;
    anyhow::ensure!(valid(id), "Invalid subscription job ID");
    let claim =
        sockrocket_core::config::storage::MutationGuard::acquire(&root().join("active.json"))?;
    let mut state = load(id)?;
    anyhow::ensure!(
        state["state"] == "running" && state["pid"] == 0,
        "Job is already owned or complete"
    );
    state["pid"] = json!(std::process::id());
    state["stage"] = json!("fetching");
    write(&path(id), &state)?;
    let input = root().join(format!("{id}.input"));
    let request = read(&input);
    let _ = fs::remove_file(input);
    drop(claim);
    CURRENT.with(|current| *current.borrow_mut() = Some(id.to_string()));
    let result = match request {
        Ok(request) => match request["action"].as_str() {
            Some(action @ ("add_sub" | "update_subs")) => dispatch(action, &request["body"]),
            _ => err_msg("Invalid subscription job request"),
        },
        Err(_) => err_msg("Subscription job request unavailable; saved settings were retained"),
    };
    CURRENT.with(|current| *current.borrow_mut() = None);
    let _job_lock =
        sockrocket_core::config::storage::MutationGuard::acquire(&root().join("active.json"))?;
    let mut state = read(&path(id))?;
    state["state"] = json!("finished");
    state["stage"] = json!("finished");
    state["result"] = result;
    state["finished"] = json!(now());
    write(&path(id), &state)
}

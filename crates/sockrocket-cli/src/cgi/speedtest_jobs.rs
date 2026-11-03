//! One detached worker owns the complete batch; firmware RPC only starts/polls it.
use super::*;
use sha2::{Digest, Sha256};
use std::process::Stdio;

fn root() -> PathBuf {
    let scope = format!(
        "{:x}",
        Sha256::digest(sockrocket_dir().to_string_lossy().as_bytes())
    );
    env::temp_dir().join(format!("sockrocket-speedtest-{}", &scope[..16]))
}
fn read(name: &str) -> Result<J> {
    Ok(serde_json::from_slice(&fs::read(root().join(name))?)?)
}
fn write(name: &str, value: &J) -> Result<()> {
    sockrocket_core::config::storage::atomic_write_private(
        &root().join(name),
        &serde_json::to_string(value)?,
        false,
    )
}
pub(super) fn key(index: usize, value: &Y) -> String {
    // Include credentials/transport in the opaque identity, and distinguish duplicates.
    let canonical = serde_json::to_value(value).unwrap_or(J::Null);
    let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
    format!("{index}-{:x}", Sha256::digest(&bytes))
}
fn running(state: &J) -> bool {
    if state["state"] != "running" {
        return false;
    }
    let pid = state["pid"].as_u64().unwrap_or(0);
    if pid == 0 {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        fs::read(format!("/proc/{pid}/cmdline"))
            .ok()
            .is_some_and(|args| {
                let fields: Vec<_> = args.split(|b| *b == 0).collect();
                fields.contains(&b"--speedtest-worker".as_slice())
                    && fields.contains(&state["id"].as_str().unwrap_or("").as_bytes())
            })
    }
    #[cfg(not(target_os = "linux"))]
    {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .is_ok_and(|s| s.success())
    }
}
pub(super) fn status() -> J {
    let _guard = if root().exists() {
        match sockrocket_core::config::storage::MutationGuard::acquire(&root().join("launch")) {
            Ok(guard) => Some(guard),
            Err(_) => return json!({"error_code":"busy","error":"Speed test state busy"}),
        }
    } else {
        None
    };
    let mut state =
        read("state.json").unwrap_or(json!({"state":"idle","results":[],"completed":0,"total":0}));
    if state["state"] == "running" && !running(&state) {
        state["state"] = json!("interrupted");
    }
    json!({"ok":true,"job":state})
}
pub(super) fn start(post: &J) -> J {
    let result = (|| -> Result<J> {
        anyhow::ensure!(
            matches!(post["mode"].as_str(), Some("tcp" | "http")),
            "Invalid probe mode"
        );
        anyhow::ensure!(
            post["request_id"]
                .as_str()
                .is_some_and(|id| !id.is_empty() && id.len() <= 64),
            "Invalid request ID"
        );
        fs::create_dir_all(root())?;
        anyhow::ensure!(
            !fs::symlink_metadata(root())?.file_type().is_symlink(),
            "Invalid job directory"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root(), fs::Permissions::from_mode(0o700))?;
        }
        let _guard =
            sockrocket_core::config::storage::MutationGuard::acquire(&root().join("launch"))?;
        if let Ok(state) = read("state.json") {
            if running(&state) {
                return Ok(json!({"ok":true,"job":state,"existing":true}));
            }
            // Idempotent start if the launch response was lost and retried.
            if post["request_id"].as_str().is_some() && post["request_id"] == state["request_id"] {
                return Ok(json!({"ok":true,"job":state,"existing":true}));
            }
        }
        let map = load_conf()?;
        let nodes = conf_seq(&map, "nodes");
        let id = format!("{:032x}", rand::random::<u128>());
        write("input.json", &json!({"nodes":nodes,"mode":post["mode"]}))?;
        let mut state = json!({"id":id,"request_id":post["request_id"],"state":"running","pid":0,"total":nodes.len(),"completed":0,"results":[]});
        write("state.json", &state)?;
        let mut command = Command::new(env::current_exe()?);
        command
            .args(["--speedtest-worker", &id])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn()?;
        state["pid"] = json!(child.id());
        write("state.json", &state)?;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(json!({"ok":true,"job":state}))
    })();
    result.unwrap_or_else(|_| json!({"ok":false,"msg":"Cannot start speed test job"}))
}
pub fn worker(id: &str) -> Result<()> {
    // The parent publishes the PID before releasing this lock.
    let guard = sockrocket_core::config::storage::MutationGuard::acquire(&root().join("launch"))?;
    let mut state = read("state.json")?;
    anyhow::ensure!(state["id"] == id, "Stale speed test worker");
    let input = read("input.json")?;
    let _ = fs::remove_file(root().join("input.json"));
    drop(guard);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        let nodes: Vec<Y> = serde_json::from_value(input["nodes"].clone())?;
        let http = input["mode"] == "http";
        let protocol_slots = std::sync::Arc::new(tokio::sync::Semaphore::new(2));
        let mut queue = nodes.into_iter().enumerate();
        let mut tasks = tokio::task::JoinSet::new();
        let mut last_save = std::time::Instant::now();
        loop {
            while tasks.len() < 8 {
                let Some((index, raw)) = queue.next() else {
                    break;
                };
                let slots = protocol_slots.clone();
                tasks.spawn(async move {
                    let key = key(index, &raw);
                    let result = async {
                        let node: sockrocket_core::Node = serde_yaml::from_value(raw)?;
                        let protocol = http
                            || matches!(
                                &node.protocol,
                                sockrocket_core::ProxyProtocol::Tuic { .. }
                                    | sockrocket_core::ProxyProtocol::Hysteria2 { .. }
                                    | sockrocket_core::ProxyProtocol::Wireguard { .. }
                            );
                        if !protocol {
                            return sockrocket_core::tcp_latency_test(&node.server, node.port, 3)
                                .await;
                        }
                        let _permit = slots.acquire().await?;
                        let outbound = sockrocket_core::create_outbound_unwarmed(&node)?;
                        sockrocket_core::quick_http_latency_test(&outbound, 5).await
                    }
                    .await;
                    json!({"key":key,"latency":result.map(i64::from).unwrap_or(-1)})
                });
            }
            let Some(result) = tasks.join_next().await else {
                break;
            };
            state["results"].as_array_mut().unwrap().push(result?);
            state["completed"] = json!(state["results"].as_array().unwrap().len());
            if last_save.elapsed() >= Duration::from_millis(250) {
                write("state.json", &state)?;
                last_save = std::time::Instant::now();
            }
        }
        state["state"] = json!("finished");
        write("state.json", &state)
    })
}

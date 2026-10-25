//! Real CGI/worker processes with a loopback-only subscription server.
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "sockrocket-job-fixture-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        fs::create_dir_all(path.join("scripts")).unwrap();
        fs::write(
            path.join("config.yaml"),
            serde_yaml::to_string(&sockrocket_core::AppConfig::default()).unwrap(),
        )
        .unwrap();
        fs::write(path.join("stopped"), "manual stop").unwrap();
        fs::write(
            path.join("scripts/sockrocket.sh"),
            "#!/bin/sh\necho unexpected-lifecycle > \"$SOCKROCKET_DIR/lifecycle-called\"\nexit 1\n",
        )
        .unwrap();
        Self(path)
    }
    fn call(&self, action: &str, body: Value) -> Value {
        let body = body.to_string();
        let mut child = Command::new(env!("CARGO_BIN_EXE_sockrocket-cli"))
            .arg("cgi")
            .env("SOCKROCKET_DIR", &self.0)
            .env("REQUEST_METHOD", "POST")
            .env("QUERY_STRING", format!("action={action}"))
            .env("CONTENT_LENGTH", body.len().to_string())
            .env_remove("GATEWAY_INTERFACE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(body.as_bytes())
            .unwrap();
        // CGI has one framed JSON line. A surviving background process can
        // retain an inherited pipe handle on Windows; EOF is not the reply.
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert!(
                output.read_line(&mut line).unwrap() > 0,
                "CGI headers required"
            );
            if line.trim().is_empty() {
                break;
            }
        }
        line.clear();
        assert!(
            output.read_line(&mut line).unwrap() > 0,
            "CGI response required"
        );
        let response = serde_json::from_str(&line).unwrap();
        assert!(child.wait().unwrap().success(), "CGI must exit cleanly");
        response
    }
    fn finish(&self, id: &Value) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let response = self.call("subscription_job", json!({"id":id}));
            assert_eq!(response["ok"], true);
            if response["job"]["state"] == "finished" {
                return response["job"].clone();
            }
            assert!(Instant::now() < deadline, "worker must finish");
            thread::sleep(Duration::from_millis(20));
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn private_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[test]
fn slow_refresh_does_not_block_edits_or_restore_a_deleted_subscription() {
    let fixture = Fixture::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/slow", listener.local_addr().unwrap());
    let mut config = sockrocket_core::AppConfig::default();
    config.subscriptions.push(sockrocket_core::Subscription {
        name: "Original".into(),
        url: endpoint,
        format: "auto".into(),
        last_updated: None,
        refresh_interval_hours: 24,
        refresh_revision: String::new(),
    });
    fs::write(
        fixture.0.join("config.yaml"),
        serde_yaml::to_string(&config).unwrap(),
    )
    .unwrap();
    let (arrived_tx, arrived_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut connection, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        connection.read(&mut request).unwrap();
        arrived_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(15)).unwrap();
        let body = "ss://YWVzLTEyOC1nY206Zml4dHVyZQ@new.invalid:8388#new";
        write!(
            connection,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    let launch = fixture.call("subscription_start", json!({"action":"update_subs"}));
    assert_eq!(launch["ok"], true);
    arrived_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let started = Instant::now();
    assert_eq!(
        fixture.call("rename_sub", json!({"index":0,"name":"Renamed"}))["ok"],
        true
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(
        fixture.call("subscriptions", json!({}))["subs"][0]["name"],
        "Renamed"
    );
    assert_eq!(fixture.call("del_sub", json!({"index":0}))["ok"], true);
    release_tx.send(()).unwrap();
    let finished = fixture.finish(&launch["job"]["id"]);
    assert_eq!(finished["result"]["error_code"], "subscriptions_removed");
    let saved = sockrocket_core::AppConfig::from_yaml_str(
        &fs::read_to_string(fixture.0.join("config.yaml")).unwrap(),
    )
    .unwrap();
    assert!(saved.subscriptions.is_empty());
    assert!(saved.nodes.is_empty());
    server.join().unwrap();
}

#[test]
fn partial_delete_during_refresh_counts_only_saved_nodes() {
    let fixture = Fixture::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let kept = format!("http://{addr}/kept");
    let mut config = sockrocket_core::AppConfig::default();
    config.subscriptions.push(sockrocket_core::Subscription {
        name: "Slow".into(),
        url: format!("http://{addr}/slow"),
        format: "auto".into(),
        last_updated: None,
        refresh_interval_hours: 24,
        refresh_revision: String::new(),
    });
    config.subscriptions.push(sockrocket_core::Subscription {
        name: "Kept".into(),
        url: kept.clone(),
        format: "auto".into(),
        last_updated: None,
        refresh_interval_hours: 24,
        refresh_revision: String::new(),
    });
    fs::write(
        fixture.0.join("config.yaml"),
        serde_yaml::to_string(&config).unwrap(),
    )
    .unwrap();
    let (arrived_tx, arrived_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut connection, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        connection.read(&mut request).unwrap();
        arrived_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(15)).unwrap();
        let body = "ss://YWVzLTEyOC1nY206Zml4dHVyZQ@removed.invalid:8388#removed";
        write!(
            connection,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
        let (mut connection, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        connection.read(&mut request).unwrap();
        let body = "ss://YWVzLTEyOC1nY206Zml4dHVyZQ@kept.invalid:8388#kept";
        write!(
            connection,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    let launch = fixture.call("subscription_start", json!({"action":"update_subs"}));
    assert_eq!(launch["ok"], true);
    arrived_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(fixture.call("del_sub", json!({"index":0}))["ok"], true);
    release_tx.send(()).unwrap();
    let finished = fixture.finish(&launch["job"]["id"]);
    assert_eq!(finished["result"]["ok"], true, "{finished}");
    let msg = finished["result"]["msg"].as_str().unwrap();
    assert!(msg.contains("已更新 1 个节点"), "{msg}");
    assert!(msg.contains("1 个订阅在下载期间被删除"), "{msg}");
    let saved = sockrocket_core::AppConfig::from_yaml_str(
        &fs::read_to_string(fixture.0.join("config.yaml")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved.subscriptions.len(), 1);
    assert_eq!(saved.subscriptions[0].url, kept);
    assert_eq!(saved.nodes.len(), 1);
    assert_eq!(saved.nodes[0].server, "kept.invalid");
    server.join().unwrap();
}

#[test]
fn background_job_survives_cgi_exit_deduplicates_and_preserves_stop() {
    let fixture = Fixture::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/fixture", listener.local_addr().unwrap());
    let (arrived_tx, arrived_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let server_released = Arc::new(AtomicBool::new(false));
    let released = server_released.clone();
    let server = thread::spawn(move || {
        let (mut connection, _) = listener.accept().unwrap();
        connection
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = [0; 4096];
        let _ = connection.read(&mut request).unwrap();
        arrived_tx.send(()).unwrap();
        let release = release_rx.recv_timeout(Duration::from_secs(30));
        released.store(true, Ordering::SeqCst);
        release.expect("test must release HTTP body after launch and duplicate responses");
        // Synthetic public documentation endpoint/credentials only.
        let body = "hy2://synthetic@example.invalid:443?fp=chrome&mport=443-445#fixture";
        write!(
            connection,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    let launch = fixture.call(
        "subscription_start",
        json!({"action":"add_sub","body":{"url":endpoint,"format":""}}),
    );
    assert_eq!(launch["ok"], true);
    assert!(
        !server_released.load(Ordering::SeqCst),
        "launch must return while HTTP is blocked"
    );
    arrived_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let id = &launch["job"]["id"];
    let poll = fixture.call("subscription_job", json!({}));
    assert_eq!(poll["job"]["id"], *id, "refresh resumes current job");
    assert_eq!(poll["job"]["state"], "running");
    assert!(
        !poll.to_string().contains(&endpoint),
        "progress must not expose private inputs"
    );
    let duplicate = fixture.call("subscription_start", json!({"action":"update_subs"}));
    assert_eq!(duplicate["existing"], true);
    assert_eq!(duplicate["job"]["id"], *id);
    assert!(
        !server_released.load(Ordering::SeqCst),
        "duplicate must bypass the blocked fetch transaction"
    );
    let status = fixture.call("subscriptions", json!({}));
    assert_eq!(
        status["subs"].as_array().unwrap().len(),
        1,
        "saved URL remains readable during download"
    );
    release_tx.send(()).unwrap();
    let finished = fixture.finish(id);
    assert_eq!(finished["result"]["ok"], true, "{finished}");
    assert_eq!(finished["result"]["node_count"], 1);
    assert_eq!(
        finished["result"]["compatibility_notes"],
        json!([
            "hysteria2_chrome_quic_profile",
            "hysteria2_mport_fixed_port"
        ])
    );
    assert!(
        !finished["result"]["msg"]
            .as_str()
            .unwrap()
            .contains("warnings")
    );
    assert_eq!(
        fixture.call("subscriptions", json!({}))["subs"][0]["name"],
        "订阅 1"
    );
    assert!(fixture.0.join("stopped").exists());
    assert!(
        !fixture.0.join("lifecycle-called").exists(),
        "subscription update cannot undo manual Stop"
    );
    assert!(
        !fixture
            .0
            .join(format!(".subscription-jobs/{}.input", id.as_str().unwrap()))
            .exists()
    );
    private_file(
        &fixture
            .0
            .join(format!(".subscription-jobs/{}.json", id.as_str().unwrap())),
    );
    let saved = fs::read_to_string(fixture.0.join("config.yaml")).unwrap();
    assert!(saved.contains(&endpoint));
    assert!(saved.contains("example.invalid"));
    server.join().unwrap();
    assert_eq!(
        fixture.call("subscription_job", json!({"id":"../../config"}))["ok"],
        false
    );
}
#[test]
fn crashed_worker_reports_failure_without_removing_saved_settings() {
    let fixture = Fixture::new();
    let jobs = fixture.0.join(".subscription-jobs");
    fs::create_dir_all(&jobs).unwrap();
    let id = "0123456789abcdef0123456789abcdef";
    fs::write(
        jobs.join(format!("{id}.json")),
        json!({"id":id,"state":"running","pid":2147483647u32,"created":1}).to_string(),
    )
    .unwrap();
    fs::write(jobs.join(format!("{id}.input")), "{}").unwrap();
    let before = fs::read(fixture.0.join("config.yaml")).unwrap();
    let status = fixture.call("subscription_job", json!({"id":id}));
    assert_eq!(status["job"]["state"], "finished");
    assert_eq!(status["job"]["result"]["ok"], false);
    assert!(!jobs.join(format!("{id}.input")).exists());
    assert_eq!(fs::read(fixture.0.join("config.yaml")).unwrap(), before);
}

#[test]
#[cfg(unix)]
fn batch_reports_progress_through_slow_apply_and_keeps_failed_source_nodes() {
    let fixture = Fixture::new();
    // This script is a fixture only; no real service is touched.
    fs::remove_file(fixture.0.join("stopped")).unwrap();
    fs::write(
        fixture.0.join("scripts/sockrocket.sh"),
        "#!/bin/sh\nsleep 1\nexit 0\n",
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let mut config = sockrocket_core::AppConfig::default();
    for index in 0..4 {
        let url = format!("{endpoint}/{index}");
        config.subscriptions.push(sockrocket_core::Subscription {
            name: format!("fixture-{index}"),
            url: url.clone(),
            format: "auto".into(),
            last_updated: None,
            refresh_interval_hours: 24,
            refresh_revision: String::new(),
        });
        let mut nodes = vec![
            sockrocket_core::config::v2ray::parse_ss_uri(&format!(
                "ss://YWVzLTEyOC1nY206Zml4dHVyZQ@old-{index}.invalid:8388#old-{index}"
            ))
            .unwrap(),
        ];
        sockrocket_core::config::dedup::tag_subscription_nodes(&mut nodes, &url);
        config.nodes.extend(nodes);
    }
    fs::write(
        fixture.0.join("config.yaml"),
        serde_yaml::to_string(&config).unwrap(),
    )
    .unwrap();
    let server = thread::spawn(move || {
        for _ in 0..4 {
            let (mut connection, _) = listener.accept().unwrap();
            connection
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = [0; 4096];
            let n = connection.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..n]);
            thread::sleep(Duration::from_millis(100));
            if request.starts_with("GET /3 ") {
                write!(connection,"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            } else {
                let index = request
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .trim_start_matches('/');
                let body = format!("hy2://synthetic@new-{index}.invalid:443?fp=chrome#new-{index}");
                write!(
                    connection,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            }
        }
    });
    let launch = fixture.call("subscription_start", json!({"action":"update_subs"}));
    assert_eq!(launch["ok"], true);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut saw_apply = false;
    loop {
        let response = fixture.call("subscription_job", json!({"id":launch["job"]["id"]}));
        let job = &response["job"];
        if job["stage"] == "applying" {
            assert_eq!(job["completed"], 4);
            assert_eq!(job["total"], 4);
            saw_apply = true;
        }
        if job["state"] == "finished" {
            assert!(
                saw_apply,
                "apply stage must remain observable during lifecycle work"
            );
            assert_eq!(
                job["result"]["ok"], true,
                "partial refresh reports warnings"
            );
            assert_eq!(job["result"]["node_count"], 4);
            assert_eq!(
                job["result"]["compatibility_notes"],
                json!(["hysteria2_chrome_quic_profile"])
            );
            assert_eq!(job["result"]["warnings"].as_array().unwrap().len(), 1);
            assert_eq!(job["result"]["skipped_subscriptions"], 1);
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(25));
    }
    let saved = sockrocket_core::AppConfig::from_yaml_str(
        &fs::read_to_string(fixture.0.join("config.yaml")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved.nodes.len(), 4);
    assert!(
        saved
            .nodes
            .iter()
            .any(|node| node.server == "old-3.invalid"),
        "failed source retains old node"
    );
    assert!(
        saved
            .nodes
            .iter()
            .any(|node| node.server == "new-0.invalid")
    );
    server.join().unwrap();
}

#[test]
#[cfg(unix)]
fn firmware_bridge_ignores_inherited_content_length_for_nested_job_body() {
    for encoded in [false, true] {
        let fixture = Fixture::new();
        let tools = fixture.0.join("bridge-tools");
        fs::create_dir_all(&tools).unwrap();
        let body = json!({"request_id":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","action":"add_sub","body":{"name":"fixture 配置","url":"http://127.0.0.1:9/fixture","format":"auto"}});
        let nonce = "a".repeat(32);
        fs::write(
            fixture.0.join(format!(
                "sockrocket_rpc_{}_{nonce}",
                if encoded { "b64" } else { "body" }
            )),
            if encoded {
                // Softcenter escapes field quotes and backslashes before dbus.
                B64.encode(body.to_string())
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
            } else {
                body.to_string()
            },
        )
        .unwrap();
        let dbus = tools.join("dbus");
        fs::write(&dbus, "#!/bin/sh\ncase \"$1\" in get) cat \"$SOCKROCKET_DIR/$2\";; remove) rm -f \"$SOCKROCKET_DIR/$2\";; esac\n").unwrap();
        let curl = tools.join("curl");
        fs::write(&curl, "#!/bin/sh\nexit 0\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dbus, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&curl, fs::Permissions::from_mode(0o700)).unwrap();
        let script = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../merlin/scripts/sockrocket_api.sh"),
        )
        .unwrap()
        .replace(". /koolshare/scripts/base.sh", "ID=$1");
        let bridge = fixture.0.join("bridge.sh");
        fs::write(&bridge, script).unwrap();
        let upload = fixture.0.join("upload");
        let path = std::env::join_paths(
            std::iter::once(tools).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let result = Command::new("sh")
            .arg(bridge)
            .args(["123456", "subscription_start", &nonce])
            .env("SOCKROCKET_DIR", &fixture.0)
            .env("SOCKROCKET_BIN", env!("CARGO_BIN_EXE_sockrocket-cli"))
            .env("SOCKROCKET_RPC_UPLOAD_DIR", &upload)
            .env("SOCKROCKET_RPC_RUNTIME_DIR", fixture.0.join("rpc"))
            .env("CONTENT_LENGTH", "2")
            .env("GATEWAY_INTERFACE", "CGI/1.1")
            .env("PATH", path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(result.success());
        let launch: Value = serde_json::from_slice(
            &fs::read(upload.join(format!("sockrocket_rpc_{nonce}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(
            launch["ok"], true,
            "firmware body must not be truncated: {launch}"
        );
        assert_eq!(launch["job"]["id"], "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        let finished = fixture.finish(&launch["job"]["id"]);
        assert_eq!(
            finished["result"]["ok"], false,
            "unreachable fixture must finish with download failure"
        );
        let saved = fixture.call("subscriptions", json!({}));
        assert_eq!(
            saved["subs"].as_array().unwrap().len(),
            1,
            "body and URL must reach the real backend"
        );
        assert_eq!(saved["subs"][0]["name"], body["body"]["name"]);
    }
}

#[test]
fn caller_request_id_is_recoverable_and_completed_retries_are_idempotent() {
    let fixture = Fixture::new();
    let id = "cccccccccccccccccccccccccccccccc";
    let missing = fixture.call("subscription_job", json!({"id":id}));
    assert_eq!(missing["error_code"], "subscription_job_not_found");
    let request = json!({"request_id":id,"action":"update_subs","body":{}});
    let launch = fixture.call("subscription_start", request.clone());
    assert_eq!(launch["ok"], true);
    assert_eq!(launch["job"]["id"], id);
    let finished = fixture.finish(&json!(id));
    let retry = fixture.call("subscription_start", request);
    assert_eq!(retry["existing"], true);
    assert_eq!(retry["job"], finished, "retry cannot start a second worker");
    let invalid = fixture.call(
        "subscription_start",
        json!({"request_id":"../private","action":"update_subs"}),
    );
    assert_eq!(invalid["ok"], false);
}

#[test]
fn startup_failure_reports_safe_stage_and_error_kind() {
    let fixture = Fixture::new();
    fs::write(
        fixture.0.join(".subscription-jobs"),
        "blocked fixture directory",
    )
    .unwrap();
    let launch=fixture.call("subscription_start",json!({"action":"add_sub","body":{"name":"fixture","url":"http://example.invalid/private-fixture"}}));
    assert_eq!(launch["ok"], false);
    assert_eq!(launch["error_code"], "subscription_start_failed");
    assert_eq!(launch["stage"], "prepare_directory");
    assert!(launch["cause"].is_string());
    assert!(launch["hint"].as_str().is_some_and(|hint| !hint.is_empty()));
    assert!(!launch.to_string().contains("example.invalid"));
    assert!(
        !launch
            .to_string()
            .contains(&fixture.0.to_string_lossy().to_string())
    );
}

#[test]
fn failed_job_publication_removes_private_request_and_finishes_owned_job() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.0.join(".subscription-jobs/active.json")).unwrap();
    let id = "dddddddddddddddddddddddddddddddd";
    let launch=fixture.call("subscription_start",json!({"request_id":id,"action":"add_sub","body":{"name":"fixture","url":"http://example.invalid/private-fixture"}}));
    assert_eq!(launch["error_code"], "subscription_start_failed");
    assert_eq!(launch["stage"], "save_job");
    assert!(
        !fixture
            .0
            .join(format!(".subscription-jobs/{id}.input"))
            .exists(),
        "failed launch must not leave private input behind"
    );
    let status = fixture.call("subscription_job", json!({"id":id}));
    assert_eq!(status["job"]["state"], "finished");
    assert_eq!(status["job"]["result"]["ok"], false);
    assert_eq!(
        fixture.call("subscriptions", json!({}))["subs"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn malformed_cgi_requests_return_safe_diagnostics_without_dispatch() {
    fn request(body: &[u8], length: Option<usize>, encoded: bool) -> Value {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sockrocket-cli"));
        command
            .arg("cgi")
            .env("REQUEST_METHOD", "POST")
            .env("QUERY_STRING", "action=version")
            .env("SOCKROCKET_LOCAL_RPC", "1")
            .env(
                "SOCKROCKET_RPC_ENCODING",
                if encoded { "base64" } else { "raw" },
            )
            .env(
                "CONTENT_LENGTH",
                length.map(|n| n.to_string()).unwrap_or_default(),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        child.stdin.take().unwrap().write_all(body).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        assert!(
            out.stderr.is_empty(),
            "request contents must not enter stderr"
        );
        let text = String::from_utf8(out.stdout).unwrap();
        serde_json::from_str(text.split_once("\n\n").unwrap().1).unwrap()
    }
    let payload =
        json!({"name":"示例 \" \\ 配置","url":"https://example.invalid/private-token"}).to_string();
    assert_eq!(
        request(B64.encode(&payload).as_bytes(), None, true)["version"],
        format!("sockrocket-cli {}", env!("CARGO_PKG_VERSION"))
    );
    let escaped = payload.replace('\\', "\\\\").replace('"', "\\\"");
    let invalid = request(escaped.as_bytes(), None, false);
    assert_eq!(invalid["cause"], "InvalidJson");
    assert_eq!(invalid["stage"], "parse_request");
    assert!(!invalid.to_string().contains("private-token"));
    assert_eq!(request(b"not:base64", None, true)["cause"], "InvalidBase64");
    assert_eq!(request(&[255], None, false)["cause"], "InvalidUtf8");
    assert_eq!(request(b"{}", Some(3), false)["cause"], "UnexpectedEof");
    assert_eq!(
        request(b"", Some(1024 * 1024 + 1), false)["error_code"],
        "request_body_too_large"
    );
    // Encoded and decoded sizes are independently bounded, including padding.
    let oversized = B64.encode(vec![b' '; 1024 * 1024 + 1]);
    assert_eq!(
        request(oversized.as_bytes(), None, true)["cause"],
        "TooLarge"
    );
}

#[test]
fn invalid_addresses_are_skipped_without_losing_existing_nodes_or_reporting_success() {
    let fixture = Fixture::new();
    let mut config = sockrocket_core::AppConfig::default();
    config.subscriptions = vec!["not-an-address", "file:///synthetic-profile"]
        .into_iter()
        .map(|url| sockrocket_core::Subscription {
            name: "fixture".into(),
            url: url.into(),
            format: "auto".into(),
            last_updated: None,
            refresh_interval_hours: 24,
            refresh_revision: String::new(),
        })
        .collect();
    config.nodes = vec![
        sockrocket_core::config::v2ray::parse_ss_uri(
            "ss://YWVzLTEyOC1nY206Zml4dHVyZQ@old.invalid:8388#old",
        )
        .unwrap(),
    ];
    let file = fixture.0.join("config.yaml");
    fs::write(&file, serde_yaml::to_string(&config).unwrap()).unwrap();
    let before = fs::read(&file).unwrap();
    let launch = fixture.call("subscription_start", json!({"action":"update_subs"}));
    let result = fixture.finish(&launch["job"]["id"])["result"].clone();
    assert_eq!(result["ok"], false);
    assert_eq!(result["error_code"], "subscription_update_failed");
    assert_eq!(result["skipped_subscriptions"], 2);
    assert_eq!(result["node_count"], 1);
    assert_eq!(fs::read(&file).unwrap(), before);
}

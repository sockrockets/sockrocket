//! Real CGI/worker processes with a loopback-only subscription server.
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
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
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(body.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "CGI must exit cleanly");
        let output = String::from_utf8(output.stdout).unwrap();
        serde_json::from_str(output.split_once("\n\n").unwrap().1).unwrap()
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
fn background_job_survives_cgi_exit_deduplicates_and_preserves_stop() {
    let fixture = Fixture::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/fixture", listener.local_addr().unwrap());
    let (arrived_tx, arrived_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut connection, _) = listener.accept().unwrap();
        connection
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = [0; 4096];
        let _ = connection.read(&mut request).unwrap();
        arrived_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        // Synthetic public documentation endpoint/credentials only.
        let body = "ss://YWVzLTEyOC1nY206Zml4dHVyZQ@example.invalid:8388#fixture";
        write!(
            connection,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    let started = Instant::now();
    let launch = fixture.call(
        "subscription_start",
        json!({"action":"add_sub","body":{"name":"fixture","url":endpoint,"format":"auto"}}),
    );
    assert_eq!(launch["ok"], true);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "launch must not wait for HTTP fetch"
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
    let repeated = Instant::now();
    let duplicate = fixture.call("subscription_start", json!({"action":"update_subs"}));
    assert_eq!(duplicate["existing"], true);
    assert_eq!(duplicate["job"]["id"], *id);
    assert!(
        repeated.elapsed() < Duration::from_secs(3),
        "duplicate must bypass fetch transaction"
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
                let body =
                    format!("ss://YWVzLTEyOC1nY206Zml4dHVyZQ@new-{index}.invalid:8388#new-{index}");
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

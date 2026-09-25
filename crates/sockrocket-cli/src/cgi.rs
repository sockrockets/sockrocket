//! CGI-mode backend for the AsusWRT-Merlin web UI.
//!
//! The router's httpd invokes this binary as `/www/cgi-bin/sockrocket.cgi` (a
//! symlink to `sockrocket-cli`) with standard CGI environment variables; this
//! module dispatches the `action` query parameter and prints a JSON
//! response. It replaces the historical shell-script CGI: all config
//! manipulation goes through `serde_yaml` (preserving unknown keys such as
//! `mode`, `dns_port` and `dns_direct_*`, which the awk/sed script used to
//! mangle), and all JSON is produced with `serde_json` (proper escaping).
//!
//! Process/firewall management (start/stop/restart, dnsmasq, iptables)
//! still delegates to `scripts/sockrocket.sh` and system tools — those are
//! genuinely shell territory on the router — but every decision and every
//! byte of JSON comes from here.
//!
//! Paths default to the router layout under `/jffs/addons/sockrocket` and can be
//! overridden with `SOCKROCKET_DIR` (used by unit tests and local debugging).

use std::env;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde_json::{Value as J, json};
use serde_yaml::{Mapping, Value as Y};

/// True when this process was invoked as a CGI endpoint: either the
/// executable was called through a `sockrocket.cgi*` link (router httpd) or the
/// first argument is the explicit `cgi` subcommand (testing).
pub fn is_cgi() -> bool {
    let argv0 = env::args().next().unwrap_or_default();
    let base = std::path::Path::new(&argv0)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    base.starts_with("sockrocket.cgi") || env::args().nth(1).as_deref() == Some("cgi")
}

/// CGI entry point: reads the request, dispatches, prints the response.
pub fn run() -> Result<()> {
    let action = query_param("action").unwrap_or_default();
    let body = if env::var("REQUEST_METHOD").as_deref() == Ok("POST") {
        read_post_body().unwrap_or_default()
    } else {
        // Allow POST-style calls in `sockrocket-cli cgi` testing mode via stdin? No —
        // actions that need a body also work without one (they error cleanly).
        String::new()
    };
    let post: J = if body.is_empty() {
        J::Null
    } else {
        serde_json::from_str(&body).unwrap_or(J::Null)
    };

    println!("Content-Type: application/json");
    println!("Cache-Control: no-cache");
    println!();
    let out = dispatch(&action, &post);
    println!(
        "{}",
        serde_json::to_string(&out).unwrap_or_else(|_| "{}".into())
    );
    Ok(())
}

fn query_param(name: &str) -> Option<String> {
    for qs in [env::var("QUERY_STRING"), env::var("REQUEST_URI")]
        .into_iter()
        .flatten()
    {
        for pair in qs.split(['&', '?']) {
            if let Some((k, v)) = pair.split_once('=')
                && k == name
            {
                return Some(v.to_string());
            }
        }
    }
    None
}

const MAX_CGI_BODY: u64 = 1024 * 1024; // 1 MiB — same cap as the HTTP API bridge

fn read_post_body() -> Result<String> {
    let mut buf = String::new();
    match env::var("CONTENT_LENGTH")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        Some(len) if len > MAX_CGI_BODY => {
            anyhow::bail!("POST body too large (max {MAX_CGI_BODY} bytes)");
        }
        Some(len) if len > 0 => {
            let mut take = std::io::stdin().take(len);
            take.read_to_string(&mut buf)?;
        }
        _ => {
            // No Content-Length (koolcenter dbus bridge): read to EOF with a hard cap.
            let mut take = std::io::stdin().take(MAX_CGI_BODY + 1);
            take.read_to_string(&mut buf)?;
            if buf.len() as u64 > MAX_CGI_BODY {
                anyhow::bail!("POST body too large (max {MAX_CGI_BODY} bytes)");
            }
        }
    }
    Ok(buf)
}

// === Paths ===

fn sockrocket_dir() -> PathBuf {
    env::var("SOCKROCKET_DIR")
        .unwrap_or_else(|_| "/jffs/addons/sockrocket".to_string())
        .into()
}
fn conf_path() -> PathBuf {
    sockrocket_dir().join("config.yaml")
}
fn pid_path() -> PathBuf {
    sockrocket_dir().join("sockrocket.pid")
}
fn log_path() -> PathBuf {
    sockrocket_dir().join("sockrocket.log")
}
fn sockrocket_sh() -> PathBuf {
    sockrocket_dir().join("scripts").join("sockrocket.sh")
}
fn iptables_sh() -> PathBuf {
    sockrocket_dir().join("scripts").join("iptables.sh")
}

/// The dnsmasq hijack file sockrocket.sh installs/removes. `SOCKROCKET_DNSMASQ_DIR` mirrors
/// the shell-side override so tests can redirect it to a temp dir.
fn dnsmasq_hijack_conf() -> PathBuf {
    PathBuf::from(
        env::var("SOCKROCKET_DNSMASQ_DIR")
            .unwrap_or_else(|_| "/jffs/configs/dnsmasq.d".to_string()),
    )
    .join("sockrocket.conf")
}

// === Small helpers ===

fn is_running() -> bool {
    let Ok(pid) = fs::read_to_string(pid_path()) else {
        return false;
    };
    let pid = pid.trim();
    if pid.is_empty() {
        return false;
    }
    // Verify the PID is alive and belongs to sockrocket-cli (not a recycled PID).
    match fs::read_to_string(format!("/proc/{}/comm", pid)) {
        Ok(comm) => comm.contains("sockrocket-cli"),
        Err(_) => false,
    }
}

fn version_string() -> String {
    format!("sockrocket-cli {}", env!("CARGO_PKG_VERSION"))
}

fn run_cmd(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        None
    }
}

fn run_sh(action: &str) {
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path());
    let mut cmd = Command::new("sh");
    cmd.arg(sockrocket_sh()).arg(action);
    if let Ok(f) = log {
        let f2 = f.try_clone();
        cmd.stdout(f);
        if let Ok(f2) = f2 {
            cmd.stderr(f2);
        }
    }
    let _ = cmd.status();
}

/// Fire-and-forget `sockrocket.sh <action>` in the background (output → sockrocket.log).
/// Reaps the child on a detached thread so the long-lived API process does not
/// accumulate zombies (router PID tables are tiny).
fn spawn_sh(action: &str) {
    let mut cmd = Command::new("sh");
    cmd.arg(sockrocket_sh()).arg(action);
    if let Ok(f) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
        && let Ok(f2) = f.try_clone()
    {
        cmd.stdout(f).stderr(f2);
    }
    if let Ok(mut child) = cmd.spawn() {
        std::thread::Builder::new()
            .name("sockrocket-sh-reaper".into())
            .spawn(move || {
                let _ = child.wait();
            })
            .ok();
    }
}

fn tail_lines(path: &PathBuf, n: usize) -> String {
    let Ok(content) = fs::read_to_string(path) else {
        return String::new();
    };
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

fn err_msg(msg: &str) -> J {
    json!({"ok": false, "msg": msg})
}

// === Config (YAML, preserving unknown keys) ===

fn ykey(k: &str) -> Y {
    Y::String(k.to_string())
}

fn load_conf() -> Result<Mapping> {
    let content = fs::read_to_string(conf_path()).context("Config file not found")?;
    let value: Y = serde_yaml::from_str(&content).context("Failed to parse config")?;
    match value {
        Y::Mapping(m) => Ok(m),
        Y::Null => Ok(Mapping::new()),
        _ => bail!("config root must be a mapping"),
    }
}

fn save_conf(map: &Mapping) -> Result<()> {
    let tmp = conf_path().with_extension("yaml.tmp");
    let text = serde_yaml::to_string(&Y::Mapping(map.clone()))?;
    fs::write(&tmp, text)?;
    let _ = fs::copy(conf_path(), conf_path().with_extension("yaml.bak"));
    fs::rename(&tmp, conf_path())?;
    Ok(())
}

fn conf_str(map: &Mapping, key: &str) -> Option<String> {
    match map.get(ykey(key)) {
        Some(Y::String(s)) => Some(s.clone()),
        Some(Y::Number(n)) => Some(n.to_string()),
        Some(Y::Bool(b)) => Some(b.to_string()),
        _ => None,
    }
}

fn conf_u16(map: &Mapping, key: &str, default: u16) -> u16 {
    map.get(ykey(key))
        .and_then(|v| v.as_u64())
        .map(|n| n as u16)
        .unwrap_or(default)
}

/// Read a boolean config key with a fallback. Accepts YAML booleans and the
/// quoted/string forms the router template may contain.
fn conf_bool(map: &Mapping, key: &str, default: bool) -> bool {
    match map.get(ykey(key)) {
        Some(Y::Bool(b)) => *b,
        Some(Y::String(s)) => matches!(s.to_ascii_lowercase().as_str(), "true" | "yes" | "1"),
        Some(Y::Number(n)) => n.as_i64().unwrap_or(0) != 0,
        _ => default,
    }
}

/// Effective toggle state: an explicit key wins; otherwise derive from the
/// legacy `mode` (tun => both on, anything else => both off). This keeps
/// configs written before the toggles existed working unchanged.
fn effective_toggles(map: &Mapping) -> (bool, bool) {
    let legacy_tun = conf_str(map, "mode").map(|m| m == "tun").unwrap_or(true);
    (
        conf_bool(map, "transparent_proxy", legacy_tun),
        conf_bool(map, "dns_hijack", legacy_tun),
    )
}

fn conf_usize(map: &Mapping, key: &str) -> Option<usize> {
    map.get(ykey(key))
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
}

fn conf_seq<'a>(map: &'a Mapping, key: &str) -> Vec<&'a Y> {
    match map.get(ykey(key)) {
        Some(Y::Sequence(s)) => s.iter().collect(),
        _ => Vec::new(),
    }
}

fn node_count(map: &Mapping) -> usize {
    conf_seq(map, "nodes").len()
}

struct NodeInfo {
    name: String,
    protocol: String,
    server: String,
    port: u16,
}

fn parse_node(v: &Y) -> NodeInfo {
    let get = |k: &str| v.get(ykey(k));
    let str_of = |k: &str| -> String {
        match get(k) {
            Some(Y::String(s)) => s.clone(),
            Some(Y::Number(n)) => n.to_string(),
            _ => String::new(),
        }
    };
    // Protocol label: tagged-enum form ({type: vless, ...}) or legacy flat
    // string (protocol: vless).
    let protocol = match get("protocol") {
        Some(Y::Mapping(p)) => match p.get(ykey("type")) {
            Some(Y::String(t)) => t.clone(),
            _ => "?".to_string(),
        },
        Some(Y::String(t)) => t.clone(),
        _ => "?".to_string(),
    };
    NodeInfo {
        name: str_of("name"),
        protocol,
        server: str_of("server"),
        port: get("port").and_then(|p| p.as_u64()).unwrap_or(0) as u16,
    }
}

fn set_active_node(map: &mut Mapping, index: usize) {
    map.insert(ykey("active_node"), Y::Number(index.into()));
}

// === Embedded HTTP API server ===
//
// The router's httpd (AsusWRT-Merlin) does NOT execute user-supplied CGI
// programs — /www/cgi-bin/sockrocket.cgi 404s no matter what is installed there,
// because /cgi-bin/* requests are handled internally by httpd. The Web UI
// therefore talks to this tiny always-on HTTP server, which reuses the same
// `dispatch()` as the CGI path. It runs as its own long-lived process
// (`sockrocket-cli api`, supervised by scripts/sockrocket.sh), independent of the proxy
// daemon, so the UI can still start/stop the service while the proxy is
// stopped — and it replaces one process spawn per request with a thread.

/// Default port for `sockrocket-cli api` (Web UI ↔ router JSON API).
pub const DEFAULT_API_PORT: u16 = 18188;

struct ApiRequest {
    method: String,
    action: String,
    body: String,
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Read one HTTP request. Returns None on malformed/oversized input or EOF.
/// Caps: 16 KB headers, 1 MB body — plenty for save_config payloads.
async fn read_request(sock: &mut tokio::net::TcpStream) -> Option<ApiRequest> {
    use tokio::io::AsyncReadExt;

    let mut buf: Vec<u8> = Vec::with_capacity(2048);
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > 16 * 1024 {
            return None;
        }
        match sock.read(&mut tmp).await {
            Ok(0) | Err(_) => return None,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    };

    let head = String::from_utf8_lossy(&buf[..header_end]);
    let mut lines = head.split("\r\n");
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_uppercase();
    let target = parts.next().unwrap_or("/").to_string();

    // Query string: /api?action=version&...
    let mut action = String::new();
    if let Some((_, qs)) = target.split_once('?') {
        for pair in qs.split('&') {
            if let Some((k, v)) = pair.split_once('=')
                && k == "action"
            {
                action = v.to_string();
            }
        }
    }

    let mut content_len = 0usize;
    for line in lines {
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            content_len = value.trim().parse().unwrap_or(0);
        }
    }
    if content_len > 1024 * 1024 {
        return None;
    }

    let mut body = buf.split_off(header_end + 4);
    while body.len() < content_len {
        match sock.read(&mut tmp).await {
            Ok(0) | Err(_) => return None,
            Ok(n) => body.extend_from_slice(&tmp[..n]),
        }
    }
    body.truncate(content_len);

    Some(ApiRequest {
        method,
        action,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

/// Serve the Web UI JSON API over plain HTTP on 0.0.0.0:<port>.
/// CORS is wide open because the page is served by the router's httpd on a
/// different port — LAN-scoped, same trust level as the router's own UI.
pub async fn serve_api(port: u16) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;
    use tokio::sync::Semaphore;

    let listener = TcpListener::bind(("0.0.0.0", port))
        .await
        .with_context(|| format!("bind API port {port}"))?;
    println!("sockrocket API listening on 0.0.0.0:{port}");

    // Cap in-flight CGI handlers. Unbounded accept→spawn_blocking let a
    // Test-all + status polling storm open dozens of threads on a 1-core
    // ARM router and stall the data-plane / DNS.
    let api_slots = Arc::new(Semaphore::new(6));

    loop {
        let (mut sock, _peer) = match listener.accept().await {
            Ok(x) => x,
            Err(_) => continue,
        };
        let api_slots = api_slots.clone();
        tokio::spawn(async move {
            let Ok(_slot) = api_slots.acquire_owned().await else {
                return;
            };
            let req = match tokio::time::timeout(Duration::from_secs(10), read_request(&mut sock))
                .await
            {
                Ok(Some(r)) => r,
                _ => return,
            };
            if req.method == "OPTIONS" {
                let _ = sock.write_all(b"HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                return;
            }
            // dispatch() shells out (speedtest, sockrocket.sh, …) and can take
            // seconds — keep it off the async reactor.
            let out = tokio::task::spawn_blocking(move || {
                let post: J = if req.body.is_empty() {
                    J::Null
                } else {
                    serde_json::from_str(&req.body).unwrap_or(J::Null)
                };
                dispatch(&req.action, &post)
            })
            .await
            .unwrap_or_else(|_| json!({"ok": false, "msg": "internal error"}));
            let body = serde_json::to_string(&out).unwrap_or_else(|_| "{}".into());
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nCache-Control: no-cache\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = sock.write_all(head.as_bytes()).await;
            let _ = sock.write_all(body.as_bytes()).await;
        });
    }
}

// === Actions ===

fn dispatch(action: &str, post: &J) -> J {
    match action {
        "status" => act_status(),
        "start" => act_start(),
        "stop" => act_stop(),
        "restart" => act_restart(),
        "nodes" => act_nodes(),
        "set_node" => act_set_node(post),
        "log" => act_log(),
        "config" => act_config(),
        "save_config" => act_save_config(post),
        "version" => json!({"version": version_string()}),
        "subscriptions" => act_subscriptions(),
        "add_sub" => act_add_sub(post),
        "del_sub" => act_del_sub(post),
        "update_subs" => act_update_subs(),
        "speedtest" => act_speedtest(),
        "stats" => act_stats(),
        "set_mode" => act_set_mode(post),
        "set_toggles" => act_set_toggles(post),
        "logs_clear" => act_logs_clear(),
        "ip_check" => act_ip_check(),
        "speedtest_node" => act_speedtest_node(post),
        "node_details" => act_node_details(),
        "add_node" => act_add_node(post),
        "del_node" => act_del_node(post),
        "sysinfo" => act_sysinfo(),
        "get_direct_domains" => act_get_direct_domains(),
        "add_direct_domain" => act_add_direct_domain(post),
        "del_direct_domain" => act_del_direct_domain(post),
        "set_direct_server" => act_set_direct_server(post),
        "get_rules" => act_get_rules(),
        "add_rule" => act_add_rule(post),
        "del_rule" => act_del_rule(post),
        "diagnose" => act_diagnose(),
        _ => json!({"error": "Unknown action"}),
    }
}

fn act_status() -> J {
    let map = load_conf().unwrap_or_default();
    let running = is_running();
    let pid = if running {
        fs::read_to_string(pid_path())
            .unwrap_or_default()
            .trim()
            .to_string()
    } else {
        String::new()
    };
    let (transparent_proxy, dns_hijack) = effective_toggles(&map);
    // Health-check runtime state, written by the daemon process on every
    // probe/switch event (tmpfs file). Null when the daemon has never run
    // with health_check.enabled.
    let health = fs::read_to_string(std::env::temp_dir().join("sockrocket_health.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<J>(&s).ok())
        .unwrap_or(J::Null);
    json!({
        "running": running,
        "pid": pid,
        "socks_port": conf_u16(&map, "socks_port", 1080).to_string(),
        "http_port": conf_u16(&map, "http_port", 1087).to_string(),
        "active_node": conf_usize(&map, "active_node").unwrap_or(0),
        "node_count": node_count(&map),
        "sub_count": conf_seq(&map, "subscriptions").len(),
        "mode": conf_str(&map, "mode").unwrap_or_else(|| "tun".into()),
        "transparent_proxy": transparent_proxy,
        "dns_hijack": dns_hijack,
        "cn_ipset_direct": conf_bool(&map, "cn_ipset_direct", false),
        "version": version_string(),
        "health": health,
    })
}

fn act_start() -> J {
    if is_running() {
        return err_msg("Already running");
    }
    run_sh("start");
    std::thread::sleep(Duration::from_secs(1));
    if is_running() {
        json!({"ok": true, "msg": "Sockrocket started"})
    } else {
        let last = tail_lines(&log_path(), 5).replace('\n', " ");
        json!({"ok": false, "msg": format!("Failed to start: {}", last)})
    }
}

fn act_stop() -> J {
    // run_sh blocks until sockrocket.sh finishes, so the verdict below reflects the
    // real post-stop state rather than a sleep-and-hope guess.
    run_sh("stop");
    // Verify, don't trust: a lock timeout or a cleanup failure used to be
    // reported as success ("Sockrocket stopped") while the transparent-proxy rules
    // and the dnsmasq hijack stayed installed — the UI then showed the
    // service as stopped with the network still hijacked.
    let stopped = !is_running();
    let hijack_gone = !dnsmasq_hijack_conf().exists();
    if stopped && hijack_gone {
        json!({"ok": true, "msg": "Sockrocket stopped"})
    } else {
        let last = tail_lines(&log_path(), 5).replace('\n', " ");
        json!({
            "ok": false,
            "msg": format!("Stop not fully effective (process: {}, DNS hijack: {}). Retry or check logs: {}",
                if stopped { "stopped" } else { "still running" },
                if hijack_gone { "cleared" } else { "still active" },
                last)
        })
    }
}

fn act_restart() -> J {
    run_sh("restart");
    std::thread::sleep(Duration::from_secs(1));
    if is_running() {
        json!({"ok": true, "msg": "Sockrocket restarted"})
    } else {
        err_msg("Restart failed, check log")
    }
}

fn act_nodes() -> J {
    let Ok(map) = load_conf() else {
        return json!({"nodes": [], "active": 0});
    };
    let names: Vec<J> = conf_seq(&map, "nodes")
        .iter()
        .map(|n| J::String(parse_node(n).name))
        .collect();
    json!({"nodes": names, "active": conf_usize(&map, "active_node").unwrap_or(0)})
}

fn act_node_details() -> J {
    let Ok(map) = load_conf() else {
        return json!({"nodes": [], "active": 0});
    };
    let nodes: Vec<J> = conf_seq(&map, "nodes")
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let info = parse_node(n);
            json!({
                "index": i,
                "name": info.name,
                "protocol": info.protocol,
                "server": info.server,
                "port": info.port,
            })
        })
        .collect();
    json!({"nodes": nodes, "active": conf_usize(&map, "active_node").unwrap_or(0)})
}

fn act_set_node(post: &J) -> J {
    let Some(index) = post
        .get("index")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
    else {
        return err_msg("Invalid index");
    };
    let Ok(mut map) = load_conf() else {
        return err_msg("Config file not found");
    };
    let count = node_count(&map);
    if count > 0 && index >= count {
        return json!({"ok": false, "msg": format!("Index {} out of range (0-{})", index, count - 1)});
    }
    set_active_node(&mut map, index);
    match save_conf(&map) {
        Ok(()) => {
            // Do NOT restart: the daemon's ConfigWatcher hot-reloads
            // active_node and swaps the outbound while listeners stay up.
            // A full restart drops SOCKS/HTTP mid-flight and makes "switch
            // then probe" look like the new node is dead.
            json!({"ok": true, "msg": "Active node updated"})
        }
        Err(e) => json!({"ok": false, "msg": format!("Save failed: {e}")}),
    }
}

fn act_log() -> J {
    json!({"log_base64": B64.encode(tail_lines(&log_path(), 150))})
}

fn act_config() -> J {
    let content = fs::read(conf_path()).unwrap_or_default();
    json!({"config_base64": B64.encode(content)})
}

fn act_save_config(post: &J) -> J {
    let Some(b64) = post.get("config").and_then(|v| v.as_str()) else {
        return err_msg("No config data");
    };
    let Ok(bytes) = B64.decode(b64) else {
        return err_msg("Invalid config data (base64 decode failed)");
    };
    if bytes.is_empty() {
        return err_msg("Invalid config data (empty)");
    }
    // Validate against the real config schema — the same structure sockrocket-cli
    // itself parses on startup. Unknown merlin-only keys are tolerated by
    // serde and preserved verbatim on the next CGI-driven edit.
    let text = String::from_utf8_lossy(&bytes);
    if let Err(e) = serde_yaml::from_str::<sockrocket_core::AppConfig>(&text) {
        return json!({"ok": false, "msg": format!("Config validation failed: {e}")});
    }
    let tmp = conf_path().with_extension("yaml.tmp");
    if fs::write(&tmp, &bytes).is_err() {
        return err_msg("Failed to write temp file");
    }
    let _ = fs::copy(conf_path(), conf_path().with_extension("yaml.bak"));
    if fs::rename(&tmp, conf_path()).is_err() {
        return err_msg("Failed to replace config file");
    }
    if is_running() {
        spawn_sh("restart");
    }
    json!({"ok": true, "msg": "Config saved and validated"})
}

fn act_subscriptions() -> J {
    let Ok(map) = load_conf() else {
        return json!({"subs": []});
    };
    let subs: Vec<J> = conf_seq(&map, "subscriptions")
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let get = |k: &str| match s.get(ykey(k)) {
                Some(Y::String(v)) => v.clone(),
                _ => String::new(),
            };
            json!({"index": i, "name": get("name"), "url": get("url"),
                   "format": match get("format") { f if f.is_empty() => "auto".into(), f => f }})
        })
        .collect();
    json!({"subs": subs})
}

fn sub_entry(name: &str, url: &str, format: &str) -> Y {
    let mut m = Mapping::new();
    m.insert(ykey("name"), Y::String(name.to_string()));
    m.insert(ykey("url"), Y::String(url.to_string()));
    m.insert(ykey("format"), Y::String(format.to_string()));
    Y::Mapping(m)
}

fn act_add_sub(post: &J) -> J {
    let name = post.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let url = post.get("url").and_then(|v| v.as_str()).unwrap_or("");
    let format = post
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("auto");
    if name.is_empty() || url.is_empty() {
        return err_msg("Name and URL must not be empty");
    }
    let Ok(mut map) = load_conf() else {
        return err_msg("Config file not found");
    };
    let mut seq = match map.remove(ykey("subscriptions")) {
        Some(Y::Sequence(s)) => s,
        _ => Vec::new(),
    };
    seq.push(sub_entry(name, url, format));
    map.insert(ykey("subscriptions"), Y::Sequence(seq));
    match save_conf(&map) {
        Ok(()) => json!({"ok": true, "msg": "Subscription added"}),
        Err(e) => json!({"ok": false, "msg": format!("Save failed: {e}")}),
    }
}

fn act_del_sub(post: &J) -> J {
    let Some(index) = post
        .get("index")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
    else {
        return err_msg("Invalid parameters");
    };
    let Ok(mut map) = load_conf() else {
        return err_msg("Config file not found");
    };
    let mut seq = match map.remove(ykey("subscriptions")) {
        Some(Y::Sequence(s)) => s,
        _ => Vec::new(),
    };
    if index < seq.len() {
        seq.remove(index);
    }
    map.insert(ykey("subscriptions"), Y::Sequence(seq));
    match save_conf(&map) {
        Ok(()) => json!({"ok": true, "msg": "Subscription deleted"}),
        Err(e) => json!({"ok": false, "msg": format!("Save failed: {e}")}),
    }
}

fn act_update_subs() -> J {
    spawn_sh("update-subs");
    json!({"ok": true, "msg": "Subscription update started; the node list refreshes automatically when done"})
}

fn socks_port() -> u16 {
    load_conf()
        .map(|m| conf_u16(&m, "socks_port", 1080))
        .unwrap_or(1080)
}

/// Cap concurrent node probes on the router. Each probe spins a current-thread
/// runtime + Reality/QUIC handshake; 4+ in parallel was observed to stall the
/// API and inflate timeouts. Shared across all accepted API connections.
fn speedtest_semaphore() -> &'static tokio::sync::Semaphore {
    static SEM: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
    SEM.get_or_init(|| tokio::sync::Semaphore::new(2))
}

/// Latency via the *running* daemon's SOCKS (warm active outbound + pool).
///
/// Used by the dashboard "Speed test" button for the *current* exit only.
/// Per-node list probes use [`act_speedtest_node`] so every node is measured
/// the same way (warm through-proxy), not a mix of SOCKS vs cold outbound.
///
/// --socks5-HOSTNAME: resolve on the daemon side. Plain --socks5 lets the
/// router's dnsmasq answer AAAA first; an IPv4-only exit then returns
/// "host unreachable" (SOCKS REP 4). time_TOTAL is the full RTT through
/// the exit (local TCP to 127.0.0.1 is ~0ms).
fn latency_via_active_socks() -> Result<u32, String> {
    let out = run_cmd(
        "curl",
        &[
            "-s",
            "-o",
            "/dev/null",
            "-w",
            "%{time_total}|%{http_code}",
            "--connect-timeout",
            "10",
            "--max-time",
            "15",
            "--socks5-hostname",
            &format!("127.0.0.1:{}", socks_port()),
            "http://www.gstatic.com/generate_204",
        ],
    )
    .ok_or_else(|| "Connection failed or timed out".to_string())?;
    let mut parts = out.trim().split('|');
    let conn: f64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let code = parts.next().unwrap_or("");
    if code == "204" || code == "200" {
        Ok((conn * 1000.0) as u32)
    } else {
        Err("Connection failed or timed out".into())
    }
}

fn act_speedtest() -> J {
    if !is_running() {
        return json!({"ok": false, "latency": -1, "msg": "Sockrocket is not running; start the service first"});
    }
    match latency_via_active_socks() {
        Ok(ms) => json!({"ok": true, "latency": ms}),
        Err(msg) => json!({"ok": false, "latency": -1, "msg": msg}),
    }
}

fn act_ip_check() -> J {
    if !is_running() {
        return json!({"ok": false, "ip": "", "country": "", "msg": "Sockrocket is not running; start the service first"});
    }
    let out = run_cmd(
        "curl",
        &[
            "-s",
            "--connect-timeout",
            "8",
            "--max-time",
            "12",
            "--socks5-hostname",
            &format!("127.0.0.1:{}", socks_port()),
            "http://ip-api.com/json?fields=query,country",
        ],
    )
    .unwrap_or_default();
    let parsed: J = serde_json::from_str(&out).unwrap_or(J::Null);
    let ip = parsed.get("query").and_then(|v| v.as_str()).unwrap_or("");
    let country = parsed.get("country").and_then(|v| v.as_str()).unwrap_or("");
    if ip.is_empty() {
        json!({"ok": false, "ip": "", "country": "", "msg": "IP check failed; make sure the proxy is running"})
    } else {
        json!({"ok": true, "ip": ip, "country": country})
    }
}

fn act_speedtest_node(post: &J) -> J {
    let Some(index) = post
        .get("index")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
    else {
        return json!({"ok": false, "latency": -1, "msg": "Invalid parameters"});
    };
    let Ok(map) = load_conf() else {
        return json!({"ok": false, "latency": -1, "msg": "Invalid parameters"});
    };
    let nodes = conf_seq(&map, "nodes");
    let Some(node) = nodes.get(index) else {
        return json!({"ok": false, "latency": -1, "msg": format!("Node {index} not found")});
    };
    // Every node uses the same warm through-proxy probe (GUI-aligned):
    // create_outbound_unwarmed + http_latency_test (warmup then measure).
    // Do NOT shortcut the active node via live SOCKS — that made the
    // selected node look an order of magnitude faster than peers and broke
    // ranking. Dashboard "Speed test" still uses SOCKS via act_speedtest.
    let node: sockrocket_core::Node = match serde_yaml::from_value((*node).clone()) {
        Ok(n) => n,
        Err(e) => {
            return json!({"ok": false, "latency": -1, "msg": format!("Failed to parse config for node {index}: {e}")});
        }
    };
    let server = node.server.clone();
    let port = node.port;
    let probe_server = server.clone();
    let result = std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => return Err(anyhow::anyhow!("runtime: {e}")),
        };
        rt.block_on(async {
            // Bound concurrent probes so Test-all cannot saturate the router.
            let _permit = speedtest_semaphore()
                .acquire()
                .await
                .map_err(|_| anyhow::anyhow!("speedtest cancelled"))?;
            let outbound = sockrocket_core::create_outbound_unwarmed(&node)?;
            // Warm path: http_latency_test does warmup (Reality classic
            // discovery / QUIC session) then times the second connect.
            match sockrocket_core::http_latency_test(&outbound, 10).await {
                Ok(ms) => Ok(ms),
                Err(http_err) => {
                    // TCP only on failure — diagnostic, not mixed into the score.
                    let tcp_result =
                        sockrocket_core::tcp_latency_test(&probe_server, port, 5).await;
                    let msg = match tcp_result {
                        Ok(_) => format!(
                            "proxy probe failed: {http_err:#} (tcp port reachable but node unusable)"
                        ),
                        Err(tcp_err) => format!(
                            "proxy probe failed: {http_err:#}; tcp fallback failed: {tcp_err:#}"
                        ),
                    };
                    Err(anyhow::anyhow!(msg))
                }
            }
        })
    })
    .join();
    match result {
        Ok(Ok(ms)) => {
            json!({"ok": true, "latency": ms, "server": server, "port": port, "path": "warm_outbound"})
        }
        Ok(Err(e)) => {
            json!({"ok": false, "latency": -1, "msg": format!("Probe via {server}:{port} failed: {e:#}")})
        }
        Err(_) => json!({"ok": false, "latency": -1, "msg": "Speed test thread panicked"}),
    }
}

fn act_add_node(post: &J) -> J {
    let get = |k: &str| {
        post.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let name = get("name");
    let server = get("server");
    let port = post.get("port").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
    let password = get("password");
    let uuid = get("uuid");
    let cipher = get("cipher");
    if name.is_empty() || server.is_empty() || port == 0 {
        return err_msg("Name, server address and port must not be empty");
    }
    let proto_raw = get("protocol");
    let proto = match proto_raw.as_str() {
        "ss" => "shadowsocks",
        "hy2" => "hysteria2",
        "shadowsocks" | "vmess" | "trojan" | "vless" | "hysteria2" | "tuic" => proto_raw.as_str(),
        _ => "shadowsocks",
    };
    let mut pm = Mapping::new();
    pm.insert(ykey("type"), Y::String(proto.to_string()));
    match proto {
        "shadowsocks" => {
            pm.insert(
                ykey("cipher"),
                Y::String(if cipher.is_empty() {
                    "aes-256-gcm".into()
                } else {
                    cipher
                }),
            );
            pm.insert(ykey("password"), Y::String(password));
        }
        "vmess" => {
            pm.insert(ykey("uuid"), Y::String(uuid));
            pm.insert(ykey("alter_id"), Y::Number(0.into()));
            pm.insert(
                ykey("cipher"),
                Y::String(if cipher.is_empty() {
                    "auto".into()
                } else {
                    cipher
                }),
            );
        }
        "vless" => {
            pm.insert(ykey("uuid"), Y::String(uuid));
        }
        "trojan" | "hysteria2" => {
            pm.insert(ykey("password"), Y::String(password));
        }
        "tuic" => {
            pm.insert(ykey("uuid"), Y::String(uuid));
            pm.insert(ykey("password"), Y::String(password));
        }
        _ => unreachable!(),
    }
    let mut node = Mapping::new();
    node.insert(ykey("name"), Y::String(name.clone()));
    node.insert(ykey("server"), Y::String(server));
    node.insert(ykey("port"), Y::Number(port.into()));
    node.insert(ykey("protocol"), Y::Mapping(pm));

    let Ok(mut map) = load_conf() else {
        return err_msg("Config file not found");
    };
    let mut seq = match map.remove(ykey("nodes")) {
        Some(Y::Sequence(s)) => s,
        _ => Vec::new(),
    };
    seq.push(Y::Mapping(node));
    map.insert(ykey("nodes"), Y::Sequence(seq));
    match save_conf(&map) {
        Ok(()) => json!({"ok": true, "msg": format!("Node added: {name}")}),
        Err(e) => json!({"ok": false, "msg": format!("Save failed: {e}")}),
    }
}

fn act_del_node(post: &J) -> J {
    let Some(index) = post
        .get("index")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
    else {
        return err_msg("Invalid parameters");
    };
    let Ok(mut map) = load_conf() else {
        return err_msg("Invalid parameters");
    };
    let mut seq = match map.remove(ykey("nodes")) {
        Some(Y::Sequence(s)) => s,
        _ => Vec::new(),
    };
    if index < seq.len() {
        seq.remove(index);
    }
    let count = seq.len();
    map.insert(ykey("nodes"), Y::Sequence(seq));
    // Clamp active_node if it now exceeds the node count.
    if let Some(active) = conf_usize(&map, "active_node")
        && count > 0
        && active >= count
    {
        set_active_node(&mut map, count - 1);
    }
    match save_conf(&map) {
        Ok(()) => json!({"ok": true, "msg": "Node deleted"}),
        Err(e) => json!({"ok": false, "msg": format!("Save failed: {e}")}),
    }
}

fn act_set_mode(post: &J) -> J {
    let mode = post.get("mode").and_then(|v| v.as_str()).unwrap_or("");
    if !matches!(mode, "tun" | "socks" | "system") {
        return err_msg("Invalid proxy mode");
    }
    let Ok(mut map) = load_conf() else {
        return err_msg("Config file not found");
    };
    map.insert(ykey("mode"), Y::String(mode.to_string()));
    match save_conf(&map) {
        Ok(()) => {
            if is_running() {
                spawn_sh("restart");
            }
            json!({"ok": true, "msg": "Mode set; takes effect after restart"})
        }
        Err(e) => json!({"ok": false, "msg": format!("Save failed: {e}")}),
    }
}

/// Flip the two independent toggles.
///
/// Each requested change is applied with a minimal action (see the match
/// below) and VERIFIED afterwards; on failure the config key is rolled back
/// so the UI never shows a toggle as on while the underlying rules are off.
/// Actions are synchronous (`run_sh`, not `spawn_sh`) precisely so this
/// rollback is possible — dispatch() already runs off the async reactor in
/// `spawn_blocking`, so blocking here only delays the one request.
///
/// Everything that runs BETWEEN a shell action and the next save re-reads
/// the config from disk first: sockrocket.sh's do_start rewrites `mode` (sed) while
/// we are inside run_sh, and saving the mapping captured before that call
/// would silently revert it (a lost update, seen live on the router). Keys
/// this action owns are re-applied on top of the fresh copy.
fn act_set_toggles(post: &J) -> J {
    let want_proxy = post.get("transparent_proxy").and_then(|v| v.as_bool());
    let want_dns = post.get("dns_hijack").and_then(|v| v.as_bool());
    let want_cn = post.get("cn_ipset_direct").and_then(|v| v.as_bool());
    if want_proxy.is_none() && want_dns.is_none() && want_cn.is_none() {
        return err_msg("No toggles to change");
    }

    let Ok(map) = load_conf() else {
        return err_msg("Config file not found");
    };
    let (cur_proxy, cur_dns) = effective_toggles(&map);
    let cur_cn = conf_bool(&map, "cn_ipset_direct", false);

    // Apply transparent_proxy first: dns_hijack verification depends on the
    // process being up (the DNS listener binds inside sockrocket-cli).
    if let Some(new_proxy) = want_proxy
        && new_proxy != cur_proxy
    {
        // Re-read: the initial load may predate a concurrent edit (another
        // request, or do_start's own migration) and would otherwise be
        // reverted by the save below.
        let mut map = match load_conf() {
            Ok(m) => m,
            Err(e) => return err_msg(&format!("Failed to read config: {e}")),
        };
        map.insert(ykey("transparent_proxy"), Y::Bool(new_proxy));
        if let Err(e) = save_conf(&map) {
            return err_msg(&format!("Save failed: {e}"));
        }
        if new_proxy {
            // restart brings up --tun + iptables + dnsmasq per the keys.
            // do_start rewrites `mode` on disk while we are in here.
            run_sh("restart");
            let ok = run_cmd(
                "iptables",
                &["-t", "mangle", "-L", "SOCKROCKET_MANGLE", "-n"],
            )
            .map(|o| o.contains("MARK"))
            .unwrap_or(false);
            if !ok {
                // Re-read before the rollback for the same reason: the disk
                // state changed during run_sh.
                let mut map = match load_conf() {
                    Ok(m) => m,
                    Err(e) => return err_msg(&format!("Failed to read config: {e}")),
                };
                map.insert(ykey("transparent_proxy"), Y::Bool(false));
                let _ = save_conf(&map);
                // Config now says off — make the router match: tear down any
                // half-applied rules and restart a plain (non-tun) daemon,
                // otherwise a --tun process lingers with no steering rules.
                run_sh("proxy-off");
                return json!({"ok": false, "msg": "Failed to enable transparent proxy: TUN or iptables rules not ready (check the tun module)"});
            }
        } else {
            run_sh("proxy-off");
            let still_on = run_cmd(
                "iptables",
                &["-t", "mangle", "-L", "SOCKROCKET_MANGLE", "-n"],
            )
            .map(|o| o.contains("MARK"))
            .unwrap_or(false);
            if still_on {
                let mut map = match load_conf() {
                    Ok(m) => m,
                    Err(e) => return err_msg(&format!("Failed to read config: {e}")),
                };
                map.insert(ykey("transparent_proxy"), Y::Bool(true));
                let _ = save_conf(&map);
                return json!({"ok": false, "msg": "Failed to disable transparent proxy: iptables rules still present"});
            }
        }
    }

    if let Some(new_dns) = want_dns
        && new_dns != cur_dns
    {
        // Same re-read as the proxy block: a save after run_sh must start
        // from what is on disk NOW, not from the snapshot taken earlier.
        let mut map = match load_conf() {
            Ok(m) => m,
            Err(e) => return err_msg(&format!("Failed to read config: {e}")),
        };
        map.insert(ykey("dns_hijack"), Y::Bool(new_dns));
        if let Err(e) = save_conf(&map) {
            return err_msg(&format!("Save failed: {e}"));
        }
        run_sh(if new_dns { "dns-on" } else { "dns-off" });
        let installed = dnsmasq_hijack_conf().exists();
        if installed != new_dns {
            let mut map = match load_conf() {
                Ok(m) => m,
                Err(e) => return err_msg(&format!("Failed to read config: {e}")),
            };
            map.insert(ykey("dns_hijack"), Y::Bool(cur_dns));
            let _ = save_conf(&map);
            return json!({
                "ok": false,
                "msg": if new_dns {
                    "Failed to enable DNS hijack: sockrocket DNS port not listening (start the service first)"
                } else {
                    "Failed to disable DNS hijack: config still set"
                }
            });
        }
    }

    // CN ipset direct: pure iptables-layer change, no daemon restart needed.
    // Applies live only when the mangle chain exists (transparent proxy on);
    // otherwise the key is saved and do_start applies it on the next start.
    if let Some(new_cn) = want_cn
        && new_cn != cur_cn
    {
        let mut map = match load_conf() {
            Ok(m) => m,
            Err(e) => return err_msg(&format!("Failed to read config: {e}")),
        };
        map.insert(ykey("cn_ipset_direct"), Y::Bool(new_cn));
        if let Err(e) = save_conf(&map) {
            return err_msg(&format!("Save failed: {e}"));
        }
        let chain_up = run_cmd(
            "iptables",
            &["-t", "mangle", "-L", "SOCKROCKET_MANGLE", "-n"],
        )
        .is_some();
        if chain_up {
            let action = if new_cn { "ipset-on" } else { "ipset-off" };
            let script = iptables_sh().to_string_lossy().to_string();
            let _ = run_cmd("sh", &[&script, action]);
            let applied = run_cmd(
                "iptables",
                &[
                    "-t",
                    "mangle",
                    "-C",
                    "SOCKROCKET_MANGLE",
                    "-m",
                    "set",
                    "--match-set",
                    "sockrocket_cn",
                    "dst",
                    "-j",
                    "RETURN",
                ],
            )
            .is_some();
            if applied != new_cn {
                let mut map = match load_conf() {
                    Ok(m) => m,
                    Err(e) => return err_msg(&format!("Failed to read config: {e}")),
                };
                map.insert(ykey("cn_ipset_direct"), Y::Bool(cur_cn));
                let _ = save_conf(&map);
                return json!({"ok": false, "msg": "Failed to toggle CN direct acceleration: ipset rules not applied (make sure ipset is available and the service is started)"});
            }
        }
    }

    json!({"ok": true, "msg": "Toggles updated"})
}

fn act_logs_clear() -> J {
    let _ = fs::write(log_path(), "");
    json!({"ok": true, "msg": "Log cleared"})
}

fn act_stats() -> J {
    let running = is_running();
    let mut uptime = "stopped".to_string();
    let mut pid_val: u64 = 0;
    if running {
        pid_val = fs::read_to_string(pid_path())
            .unwrap_or_default()
            .trim()
            .parse()
            .unwrap_or(0);
        if let (Ok(stat), Ok(proc_stat)) = (
            fs::read_to_string(format!("/proc/{}/stat", pid_val)),
            fs::read_to_string("/proc/stat"),
        ) {
            let starttime: u64 = stat
                .split_whitespace()
                .nth(21)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let btime: u64 = proc_stat
                .lines()
                .find(|l| l.starts_with("btime"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if btime > 0 && now > 0 && starttime > 0 {
                let elapsed = now.saturating_sub(btime + starttime / 100);
                uptime = format!(
                    "{}h {}m {}s",
                    elapsed / 3600,
                    (elapsed % 3600) / 60,
                    elapsed % 60
                );
            } else {
                uptime = "running".to_string();
            }
        }
    }
    let dns_active = dnsmasq_hijack_conf().exists();
    let fw_active = run_cmd(
        "iptables",
        &["-t", "mangle", "-L", "SOCKROCKET_MANGLE", "-n"],
    )
    .map(|out| out.contains("MARK"))
    .unwrap_or(false);
    json!({"ok": true, "uptime": uptime, "pid": pid_val, "dns_active": dns_active, "fw_active": fw_active})
}

fn act_sysinfo() -> J {
    let meminfo = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let kb_of = |key: &str| -> u64 {
        meminfo
            .lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    };
    let mem_total = kb_of("MemTotal:");
    let mem_avail = kb_of("MemAvailable:");
    let mem_used = mem_total.saturating_sub(mem_avail);
    let mem_pct = mem_used
        .checked_mul(100)
        .and_then(|v| v.checked_div(mem_total))
        .unwrap_or(0);
    let load = fs::read_to_string("/proc/loadavg")
        .unwrap_or_default()
        .split_whitespace()
        .next()
        .unwrap_or("0.00")
        .to_string();
    let uptime_s: u64 = fs::read_to_string("/proc/uptime")
        .unwrap_or_default()
        .split_whitespace()
        .next()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0) as u64;
    let wan_ip = run_cmd("nvram", &["get", "wan0_ipaddr"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    json!({
        "ok": true,
        "mem_total_kb": mem_total,
        "mem_used_kb": mem_used,
        "mem_pct": mem_pct,
        "load": load,
        "uptime": format!("{}h {}m", uptime_s / 3600, (uptime_s % 3600) / 60),
        "wan_ip": wan_ip,
    })
}

fn direct_domains(map: &Mapping) -> Vec<String> {
    conf_seq(map, "dns_direct_domains")
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect()
}

fn act_get_direct_domains() -> J {
    let map = load_conf().unwrap_or_default();
    let server = conf_str(&map, "dns_direct_server").unwrap_or_else(|| "114.114.114.114".into());
    let domains: Vec<J> = direct_domains(&map).into_iter().map(J::String).collect();
    json!({"ok": true, "server": server, "domains": domains})
}

fn valid_domain(d: &str) -> bool {
    let parts = d.split('.');
    let mut count = 0;
    let mut last_ok = false;
    for p in parts {
        count += 1;
        last_ok = !p.is_empty()
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if p.is_empty() {
            return false;
        }
    }
    count >= 2 && last_ok
}

fn act_add_direct_domain(post: &J) -> J {
    let domain = post.get("domain").and_then(|v| v.as_str()).unwrap_or("");
    if !valid_domain(domain) {
        return err_msg("Invalid domain format");
    }
    let Ok(mut map) = load_conf() else {
        return err_msg("Config file not found");
    };
    if direct_domains(&map).iter().any(|d| d == domain) {
        return json!({"ok": false, "msg": format!("Domain {domain} already exists")});
    }
    let mut seq = match map.remove(ykey("dns_direct_domains")) {
        Some(Y::Sequence(s)) => s,
        _ => Vec::new(),
    };
    seq.push(Y::String(domain.to_string()));
    map.insert(ykey("dns_direct_domains"), Y::Sequence(seq));
    match save_conf(&map) {
        Ok(()) => json!({"ok": true, "msg": format!("Added: {domain}")}),
        Err(e) => json!({"ok": false, "msg": format!("Save failed: {e}")}),
    }
}

fn act_del_direct_domain(post: &J) -> J {
    let Some(index) = post
        .get("index")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
    else {
        return err_msg("Invalid parameters");
    };
    let Ok(mut map) = load_conf() else {
        return err_msg("Invalid parameters");
    };
    let mut seq = match map.remove(ykey("dns_direct_domains")) {
        Some(Y::Sequence(s)) => s,
        _ => Vec::new(),
    };
    if index < seq.len() {
        seq.remove(index);
    }
    map.insert(ykey("dns_direct_domains"), Y::Sequence(seq));
    match save_conf(&map) {
        Ok(()) => json!({"ok": true, "msg": "Deleted"}),
        Err(e) => json!({"ok": false, "msg": format!("Save failed: {e}")}),
    }
}

fn act_set_direct_server(post: &J) -> J {
    let server = post.get("server").and_then(|v| v.as_str()).unwrap_or("");
    if server.parse::<std::net::Ipv4Addr>().is_err() {
        return err_msg("Enter a valid IPv4 address");
    }
    let Ok(mut map) = load_conf() else {
        return err_msg("Config file not found");
    };
    map.insert(ykey("dns_direct_server"), Y::String(server.to_string()));
    match save_conf(&map) {
        Ok(()) => {
            if is_running() {
                spawn_sh("restart");
            }
            json!({"ok": true, "msg": format!("DNS server updated to {server}; takes effect after restart")})
        }
        Err(e) => json!({"ok": false, "msg": format!("Save failed: {e}")}),
    }
}

/* ── Routing rules (Merlin Web UI) ─────────────────────────────────── */

/// Rule types accepted from the UI — mirrors RuleSet::from_config.
const RULE_TYPES: &[&str] = &[
    "domain",
    "domain-suffix",
    "domain-keyword",
    "ip-cidr",
    "geoip",
    "final",
];
/// Rule targets accepted from the UI.
const RULE_TARGETS: &[&str] = &["direct", "proxy", "reject"];

/// Validate the pattern for a rule type; returns the normalized pattern
/// (or the fixed placeholder for `final`, which takes no pattern).
fn valid_rule_pattern(rule_type: &str, pattern: &str) -> std::result::Result<String, String> {
    let p = pattern.trim();
    match rule_type {
        "domain" => {
            if valid_domain(p) {
                Ok(p.to_lowercase())
            } else {
                Err("Invalid domain format (e.g. www.example.com)".into())
            }
        }
        "domain-suffix" => {
            let s = p.trim_start_matches('.');
            if valid_domain(s) {
                Ok(s.to_lowercase())
            } else {
                Err("Invalid domain suffix format (e.g. example.com)".into())
            }
        }
        "domain-keyword" => {
            if !p.is_empty() && !p.chars().any(char::is_whitespace) {
                Ok(p.to_lowercase())
            } else {
                Err("Keyword must not be empty or contain whitespace".into())
            }
        }
        "ip-cidr" => {
            let (addr, prefix) = p.split_once('/').unwrap_or((p, ""));
            let Ok(ip) = addr.parse::<std::net::IpAddr>() else {
                return Err("Invalid IP address (e.g. 10.0.0.0/8)".into());
            };
            let max = if ip.is_ipv4() { 32 } else { 128 };
            match prefix.parse::<u8>() {
                Ok(n) if n <= max => Ok(format!("{addr}/{n}")),
                _ => Err(format!("Invalid prefix length (0-{max})")),
            }
        }
        "geoip" => {
            if p.len() == 2 && p.chars().all(|c| c.is_ascii_alphabetic()) {
                Ok(p.to_uppercase())
            } else {
                Err("Invalid GeoIP country code (e.g. CN, US)".into())
            }
        }
        "final" => Ok("*".to_string()),
        _ => Err(format!("Unknown rule type: {rule_type}")),
    }
}

/// Read the `rules:` sequence as JSON rows for the UI.
fn act_get_rules() -> J {
    let map = load_conf().unwrap_or_default();
    let rules: Vec<J> = map
        .get(ykey("rules"))
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .enumerate()
                .map(|(i, r)| {
                    json!({
                        "index": i,
                        "rule_type": r.get(ykey("rule_type")).and_then(|v| v.as_str()).unwrap_or(""),
                        "pattern": r.get(ykey("pattern")).and_then(|v| v.as_str()).unwrap_or(""),
                        "target": r.get(ykey("target")).and_then(|v| v.as_str()).unwrap_or("proxy"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    json!({"ok": true, "rules": rules})
}

/// Add a rule. User rules go FIRST in the sequence: evaluation is
/// first-match-wins, and the built-in private-range/geoip-CN direct rules
/// would otherwise shadow UI-added rules for domestic domains.
fn act_add_rule(post: &J) -> J {
    let rule_type = post.get("rule_type").and_then(|v| v.as_str()).unwrap_or("");
    if !RULE_TYPES.contains(&rule_type) {
        return err_msg("Invalid rule type");
    }
    let target = post.get("target").and_then(|v| v.as_str()).unwrap_or("");
    if !RULE_TARGETS.contains(&target) {
        return err_msg("Invalid target action (direct / proxy / reject)");
    }
    let pattern = match valid_rule_pattern(
        rule_type,
        post.get("pattern").and_then(|v| v.as_str()).unwrap_or(""),
    ) {
        Ok(p) => p,
        Err(msg) => return err_msg(&msg),
    };

    let Ok(mut map) = load_conf() else {
        return err_msg("Config file not found");
    };
    let mut seq = match map.remove(ykey("rules")) {
        Some(Y::Sequence(s)) => s,
        _ => Vec::new(),
    };
    // Duplicate check: same type+pattern already present.
    let dup = seq.iter().any(|r| {
        r.get(ykey("rule_type")).and_then(|v| v.as_str()) == Some(rule_type)
            && r.get(ykey("pattern")).and_then(|v| v.as_str()) == Some(pattern.as_str())
    });
    if dup {
        return json!({"ok": false, "msg": format!("Rule already exists: {rule_type} {pattern}")});
    }

    let mut entry = Mapping::new();
    entry.insert(ykey("rule_type"), Y::String(rule_type.to_string()));
    entry.insert(ykey("pattern"), Y::String(pattern.clone()));
    entry.insert(ykey("target"), Y::String(target.to_string()));
    seq.insert(0, Y::Mapping(entry));
    map.insert(ykey("rules"), Y::Sequence(seq));

    match save_conf(&map) {
        Ok(()) => {
            // The TUN-side rule override reads the router built at daemon
            // start, so rules need a full restart to take effect there.
            if is_running() {
                spawn_sh("restart");
            }
            json!({"ok": true, "msg": format!("Added {rule_type} {pattern} -> {target} (takes effect after service restart)")})
        }
        Err(e) => json!({"ok": false, "msg": format!("Save failed: {e}")}),
    }
}

fn act_del_rule(post: &J) -> J {
    let Some(index) = post
        .get("index")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
    else {
        return err_msg("Invalid parameters");
    };
    let Ok(mut map) = load_conf() else {
        return err_msg("Config file not found");
    };
    let mut seq = match map.remove(ykey("rules")) {
        Some(Y::Sequence(s)) => s,
        _ => Vec::new(),
    };
    if index >= seq.len() {
        return err_msg("Rule index out of range");
    }
    seq.remove(index);
    map.insert(ykey("rules"), Y::Sequence(seq));
    match save_conf(&map) {
        Ok(()) => {
            if is_running() {
                spawn_sh("restart");
            }
            json!({"ok": true, "msg": "Deleted (takes effect after service restart)"})
        }
        Err(e) => json!({"ok": false, "msg": format!("Save failed: {e}")}),
    }
}

fn act_diagnose() -> J {
    let running = is_running();
    let config_valid = fs::read_to_string(conf_path())
        .map(|c| serde_yaml::from_str::<sockrocket_core::AppConfig>(&c).is_ok())
        .unwrap_or(false);
    let ipt_count = run_cmd(
        "iptables",
        &["-t", "mangle", "-L", "SOCKROCKET_MANGLE", "-n"],
    )
    .map(|out| out.matches("MARK").count())
    .unwrap_or(0);
    let dns_rules = if dnsmasq_hijack_conf().exists() { 1 } else { 0 };
    // .output(), NOT .status(): the diagnose endpoint runs inside the api
    // bridge whose stderr is sockrocket.log — an inherited stderr would spill probe
    // errors ("Couldn't load target `SOCKROCKET_MANGLE'" on old iptables) into the
    // log on every poll while the proxy is off.
    // pidof, not pgrep: stock Merlin firmware ships /bin/pidof but NO pgrep,
    // so the pgrep probe always failed and diagnose reported "dnsmasq not running"
    // on a router whose dnsmasq was happily running.
    let dnsmasq_running = Command::new("pidof")
        .arg("dnsmasq")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let fw_active = Command::new("iptables")
        .args([
            "-t",
            "mangle",
            "-C",
            "PREROUTING",
            "-j",
            "SOCKROCKET_MANGLE",
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let errors: Vec<String> = tail_lines(&log_path(), 500)
        .lines()
        .filter(|l| {
            let l = l.to_lowercase();
            l.contains("error") || l.contains("fail")
        })
        .map(|s| s.to_string())
        .collect();
    let last_errors = errors
        .iter()
        .rev()
        .take(10)
        .rev()
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    json!({
        "ok": true,
        "running": running,
        "config_valid": config_valid,
        "iptables_rules": ipt_count,
        "dns_rules": dns_rules,
        "dnsmasq_running": dnsmasq_running,
        "fw_active": fw_active,
        "last_errors_base64": B64.encode(last_errors),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    static TEST_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    struct TestEnv {
        dir: PathBuf,
        // SOCKROCKET_DIR is process-global; serialize config-mutating tests.
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl TestEnv {
        fn new(config: &str) -> Self {
            // Recover from poisoning instead of cascading it: when one test
            // panics while holding TEST_LOCK (SOCKROCKET_DIR is process-global, so
            // the lock must be held for the whole test), every later test
            // would otherwise fail with PoisonError — masking which test
            // actually broke. The Mutex<()> has no state to corrupt.
            let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let seq = TEST_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let dir = env::temp_dir().join(format!(
                "sockrocket-cgi-test-{}-{}",
                std::process::id(),
                seq
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("config.yaml"), config).unwrap();
            unsafe { env::set_var("SOCKROCKET_DIR", &dir) };
            Self { dir, _guard: guard }
        }
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    const SAMPLE: &str = r#"listen_addr: "0.0.0.0"
socks_port: 1080
http_port: 1087
dns_port: 5300
dns_direct_server: "114.114.114.114"
mode: "tun"
active_node: 0
subscriptions:
  - name: "Sub 1"
    url: "https://example.com/sub"
    format: "auto"
nodes:
  - name: "Node A"
    server: "a.example.com"
    port: 443
    protocol:
      type: vless
      uuid: "00000000-0000-0000-0000-000000000000"
    transport: null
  - name: "Node B"
    server: "b.example.com"
    port: 40001
    protocol:
      type: hysteria2
      password: "pw"
    transport: null
rules: []
dns_direct_domains:
  - "baidu.com"
  - "qq.com"
"#;

    #[test]
    fn status_preserves_shape() {
        let _env = TestEnv::new(SAMPLE);
        let s = act_status();
        assert_eq!(s["running"], J::Bool(false));
        assert_eq!(s["active_node"], J::from(0));
        assert_eq!(s["node_count"], J::from(2));
        assert_eq!(s["sub_count"], J::from(1));
        assert_eq!(s["mode"], J::from("tun"));
        assert_eq!(s["socks_port"], J::from("1080"));
    }

    #[test]
    fn stop_reports_success_when_nothing_is_left_running() {
        let _env = TestEnv::new(SAMPLE);
        // Nothing is running (no pid file) and no hijack file exists. Point
        // SOCKROCKET_DNSMASQ_DIR at an empty temp dir so the check is deterministic
        // instead of depending on the host's real /jffs.
        let dns_dir = env::temp_dir().join(format!("sockrocket-dns-empty-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dns_dir);
        fs::create_dir_all(&dns_dir).unwrap();
        unsafe { env::set_var("SOCKROCKET_DNSMASQ_DIR", &dns_dir) };
        // sockrocket.sh is absent in the test env, so run_sh("stop") is a no-op —
        // the "already stopped, nothing left behind" shape.
        let r = act_stop();
        unsafe { env::remove_var("SOCKROCKET_DNSMASQ_DIR") };
        let _ = fs::remove_dir_all(&dns_dir);
        assert_eq!(r["ok"], J::Bool(true), "{r}");
    }

    #[test]
    fn stop_reports_failure_when_hijack_survives() {
        let _env = TestEnv::new(SAMPLE);
        // Simulate the failed-cleanup state: the dnsmasq hijack is still
        // installed after the stop ran. The API must NOT report success —
        // this is the state that blackholed LAN DNS while the UI claimed
        // the service was stopped.
        let dns_dir = env::temp_dir().join(format!("sockrocket-dns-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dns_dir);
        fs::create_dir_all(&dns_dir).unwrap();
        fs::write(dns_dir.join("sockrocket.conf"), "server=127.0.0.1#5300\n").unwrap();
        unsafe { env::set_var("SOCKROCKET_DNSMASQ_DIR", &dns_dir) };
        let r = act_stop();
        unsafe { env::remove_var("SOCKROCKET_DNSMASQ_DIR") };
        let _ = fs::remove_dir_all(&dns_dir);
        assert_eq!(
            r["ok"],
            J::Bool(false),
            "surviving hijack must fail the stop: {r}"
        );
        assert!(r["msg"].as_str().unwrap_or("").contains("DNS hijack"));
    }

    #[test]
    fn node_details_parses_tagged_protocol() {
        let _env = TestEnv::new(SAMPLE);
        let d = act_node_details();
        let nodes = d["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0]["name"], J::from("Node A"));
        assert_eq!(nodes[0]["protocol"], J::from("vless"));
        assert_eq!(nodes[0]["server"], J::from("a.example.com"));
        assert_eq!(nodes[0]["port"], J::from(443));
        assert_eq!(nodes[1]["protocol"], J::from("hysteria2"));
    }

    #[test]
    fn set_node_edits_and_preserves_merlin_keys() {
        let _env = TestEnv::new(SAMPLE);
        let r = act_set_node(&json!({"index": 1}));
        assert_eq!(r["ok"], J::Bool(true));
        let text = fs::read_to_string(conf_path()).unwrap();
        let map = load_conf().unwrap();
        assert_eq!(conf_usize(&map, "active_node"), Some(1));
        // Merlin-only keys survive a CGI edit round-trip.
        assert_eq!(conf_str(&map, "mode").as_deref(), Some("tun"));
        assert_eq!(conf_u16(&map, "dns_port", 0), 5300);
        assert!(text.contains("dns_direct_domains"));
        // Out-of-range rejected.
        let r = act_set_node(&json!({"index": 5}));
        assert_eq!(r["ok"], J::Bool(false));
    }

    #[test]
    fn subscription_add_del() {
        let _env = TestEnv::new(SAMPLE);
        let r = act_add_sub(&json!({"name": "Sub 2", "url": "https://x.com/s", "format": "clash"}));
        assert_eq!(r["ok"], J::Bool(true));
        let subs = act_subscriptions()["subs"].as_array().unwrap().clone();
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[1]["format"], J::from("clash"));
        let r = act_del_sub(&json!({"index": 0}));
        assert_eq!(r["ok"], J::Bool(true));
        let subs = act_subscriptions()["subs"].as_array().unwrap().clone();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0]["name"], J::from("Sub 2"));
    }

    #[test]
    fn add_node_builds_valid_tagged_protocol() {
        let _env = TestEnv::new(SAMPLE);
        for (proto, extra) in [
            (
                "shadowsocks",
                json!({"password": "pw", "cipher": "aes-256-gcm"}),
            ),
            ("vmess", json!({"uuid": "u", "cipher": "auto"})),
            ("vless", json!({"uuid": "u"})),
            ("trojan", json!({"password": "pw"})),
            ("hysteria2", json!({"password": "pw"})),
            ("tuic", json!({"uuid": "u", "password": "pw"})),
        ] {
            let mut req = json!({"name": format!("N-{proto}"), "server": "s.com", "port": 443, "protocol": proto});
            req.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            let r = act_add_node(&req);
            assert_eq!(r["ok"], J::Bool(true), "add_node {proto}: {r}");
        }
        // Every produced node must parse against the real schema sockrocket-cli uses.
        let text = fs::read_to_string(conf_path()).unwrap();
        serde_yaml::from_str::<sockrocket_core::AppConfig>(&text)
            .expect("config with added nodes must validate");
        let d = act_node_details();
        assert_eq!(d["nodes"].as_array().unwrap().len(), 8);
    }

    #[test]
    fn del_node_clamps_active() {
        let _env = TestEnv::new(SAMPLE);
        act_set_node(&json!({"index": 1}));
        let r = act_del_node(&json!({"index": 1}));
        assert_eq!(r["ok"], J::Bool(true));
        let map = load_conf().unwrap();
        assert_eq!(conf_usize(&map, "active_node"), Some(0));
        assert_eq!(node_count(&map), 1);
    }

    #[test]
    fn direct_domains_crud() {
        let _env = TestEnv::new(SAMPLE);
        let r = act_add_direct_domain(&json!({"domain": "baidu.com"}));
        assert_eq!(r["ok"], J::Bool(false), "duplicate rejected");
        let r = act_add_direct_domain(&json!({"domain": "not a domain"}));
        assert_eq!(r["ok"], J::Bool(false), "invalid rejected");
        let r = act_add_direct_domain(&json!({"domain": "example.org"}));
        assert_eq!(r["ok"], J::Bool(true));
        let d = act_get_direct_domains();
        assert_eq!(d["domains"].as_array().unwrap().len(), 3);
        let r = act_del_direct_domain(&json!({"index": 0}));
        assert_eq!(r["ok"], J::Bool(true));
        let d = act_get_direct_domains();
        let domains = d["domains"].as_array().unwrap();
        assert_eq!(domains.len(), 2);
        assert_eq!(domains[0], J::from("qq.com"));
        let r = act_set_direct_server(&json!({"server": "223.5.5.5"}));
        assert_eq!(r["ok"], J::Bool(true));
        assert_eq!(act_get_direct_domains()["server"], J::from("223.5.5.5"));
        let r = act_set_direct_server(&json!({"server": "999.1.1.1"}));
        assert_eq!(r["ok"], J::Bool(false));
    }

    #[test]
    fn rules_crud() {
        let _env = TestEnv::new(SAMPLE);
        // Empty rules list initially.
        assert_eq!(act_get_rules()["rules"].as_array().unwrap().len(), 0);

        // Validation rejections.
        for bad in [
            json!({"rule_type": "nope", "pattern": "x.com", "target": "direct"}),
            json!({"rule_type": "domain", "pattern": "not a domain", "target": "direct"}),
            json!({"rule_type": "ip-cidr", "pattern": "10.0.0.0/33", "target": "direct"}),
            json!({"rule_type": "geoip", "pattern": "CNN", "target": "direct"}),
            json!({"rule_type": "domain", "pattern": "x.com", "target": "nuke"}),
        ] {
            let r = act_add_rule(&bad);
            assert_eq!(r["ok"], J::Bool(false), "must reject {bad}");
        }

        // Valid adds; new rules land at the FRONT (first-match-wins).
        let r = act_add_rule(
            &json!({"rule_type": "domain-suffix", "pattern": "Example.COM", "target": "proxy"}),
        );
        assert_eq!(r["ok"], J::Bool(true), "{r}");
        let r = act_add_rule(&json!({"rule_type": "geoip", "pattern": "us", "target": "proxy"}));
        assert_eq!(r["ok"], J::Bool(true), "{r}");

        let rules = act_get_rules()["rules"].as_array().unwrap().clone();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0]["rule_type"], J::from("geoip"), "newest first");
        assert_eq!(rules[0]["pattern"], J::from("US"), "geoip uppercased");
        assert_eq!(
            rules[1]["pattern"],
            J::from("example.com"),
            "suffix lowercased"
        );

        // Duplicate rejected.
        let r = act_add_rule(
            &json!({"rule_type": "domain-suffix", "pattern": "example.com", "target": "direct"}),
        );
        assert_eq!(r["ok"], J::Bool(false), "duplicate rejected");

        // The produced YAML must parse against the real AppConfig schema.
        let text = fs::read_to_string(conf_path()).unwrap();
        let cfg: sockrocket_core::AppConfig =
            serde_yaml::from_str(&text).expect("config with added rules must validate");
        assert_eq!(cfg.rules.len(), 2);

        // Delete by index.
        let r = act_del_rule(&json!({"index": 1}));
        assert_eq!(r["ok"], J::Bool(true));
        let rules = act_get_rules()["rules"].as_array().unwrap().clone();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0]["pattern"], J::from("US"));
        let r = act_del_rule(&json!({"index": 9}));
        assert_eq!(r["ok"], J::Bool(false), "out-of-range rejected");
    }

    #[test]
    fn save_config_validates_and_preserves_unknown_keys() {
        let _env = TestEnv::new(SAMPLE);
        let bad = B64.encode("listen_addr: 127.0.0.1\nnodes: [{name: x}]\n");
        let r = act_save_config(&json!({"config": bad}));
        assert_eq!(r["ok"], J::Bool(false), "schema violation rejected: {r}");
        assert!(fs::read_to_string(conf_path()).unwrap().contains("Node A"));

        let good = B64.encode(SAMPLE.replace("Node A", "Node A2"));
        let r = act_save_config(&json!({"config": good}));
        assert_eq!(r["ok"], J::Bool(true), "{r}");
        let map = load_conf().unwrap();
        assert_eq!(conf_str(&map, "mode").as_deref(), Some("tun"));
        assert_eq!(conf_u16(&map, "dns_port", 0), 5300);
    }

    #[test]
    fn legacy_flat_protocol_still_parses() {
        let legacy = SAMPLE.replace(
            "    protocol:\n      type: vless\n      uuid: \"00000000-0000-0000-0000-000000000000\"\n    transport: null",
            "    protocol: vless",
        );
        let _env = TestEnv::new(&legacy);
        let d = act_node_details();
        assert_eq!(d["nodes"][0]["protocol"], J::from("vless"));
    }

    #[test]
    fn valid_domain_check() {
        assert!(valid_domain("baidu.com"));
        assert!(valid_domain("a.b.example.org"));
        assert!(!valid_domain("nodot"));
        assert!(!valid_domain("bad..com"));
        assert!(!valid_domain(""));
    }

    #[test]
    fn status_exposes_toggle_keys() {
        let _env = TestEnv::new(SAMPLE);
        let s = act_status();
        // SAMPLE has mode: tun and no explicit toggle keys -> both derived true.
        assert_eq!(s["transparent_proxy"], J::Bool(true), "{s}");
        assert_eq!(s["dns_hijack"], J::Bool(true), "{s}");
    }

    #[test]
    fn status_derives_toggles_from_legacy_mode() {
        let _env = TestEnv::new(&SAMPLE.replace("mode: \"tun\"", "mode: \"socks\""));
        let s = act_status();
        assert_eq!(s["transparent_proxy"], J::Bool(false), "{s}");
        assert_eq!(s["dns_hijack"], J::Bool(false), "{s}");
    }

    #[test]
    fn status_prefers_explicit_toggle_keys() {
        // Explicit keys win over what mode would imply.
        let cfg = SAMPLE.replace(
            "mode: \"tun\"",
            "mode: \"tun\"\ntransparent_proxy: false\ndns_hijack: true",
        );
        let _env = TestEnv::new(&cfg);
        let s = act_status();
        assert_eq!(s["transparent_proxy"], J::Bool(false), "{s}");
        assert_eq!(s["dns_hijack"], J::Bool(true), "{s}");
    }

    #[test]
    fn set_toggles_writes_keys_and_leaves_others_alone() {
        let _env = TestEnv::new(SAMPLE);
        // sockrocket.sh is absent in the test env, so the shell action is a no-op;
        // the keys must still be written and the untouched ones preserved.
        let _ = act_set_toggles(&json!({"dns_hijack": false}));
        let map = load_conf().unwrap();
        assert_eq!(conf_str(&map, "dns_hijack").as_deref(), Some("false"));
        assert_eq!(conf_str(&map, "mode").as_deref(), Some("tun"));
        assert!(map.contains_key(ykey("nodes")));
    }

    #[test]
    fn set_toggles_rejects_empty_request() {
        let _env = TestEnv::new(SAMPLE);
        let r = act_set_toggles(&json!({}));
        assert_eq!(r["ok"], J::Bool(false), "{r}");
    }

    // THE LOST-UPDATE REGRESSION: inside one set_toggles request, the proxy
    // block saves, then run_sh("restart") makes sockrocket.sh's do_start rewrite
    // `mode` on disk (sed) while we wait, and the next save used to write
    // back the mapping captured BEFORE that restart — silently reverting the
    // sed. Seen live on the router: flipping both toggles ended with
    // config.yaml saying mode: socks while the daemon ran with --tun.
    //
    // Deterministic reproduction without the router: a fake sockrocket.sh performs
    // the same sed during restart, and because the iptables verification
    // binary does not exist on a dev box, the action takes its ROLLBACK path
    // — which performs exactly the post-run_sh save that used to clobber.
    // Requires a real POSIX `sh` (run_sh invokes `Command::new("sh")`); skip
    // on Windows where that is typically absent from PATH.
    #[cfg(unix)]
    #[test]
    fn set_toggles_preserves_concurrent_disk_edits() {
        // mode: socks -> both toggles derived false, so the flip is real.
        let cfg = SAMPLE.replace("mode: \"tun\"", "mode: \"socks\"");
        let _env = TestEnv::new(&cfg);
        let sh_path = sockrocket_sh();
        fs::create_dir_all(sh_path.parent().unwrap()).unwrap();
        fs::write(
            &sh_path,
            "#!/bin/sh\n[ \"$1\" = \"restart\" ] || exit 0\nf=\"$SOCKROCKET_DIR/config.yaml\"\ntmp=\"$f.tmp\"\nsed 's/^mode:.*/mode: \"tun\"/' \"$f\" > \"$tmp\" && mv \"$tmp\" \"$f\"\nexit 0\n",
        )
        .unwrap();

        let r = act_set_toggles(&json!({"transparent_proxy": true}));
        // iptables is absent in the test env, so the proxy-on verification
        // fails and the action rolls the key back.
        assert_eq!(r["ok"], J::Bool(false), "{r}");
        let map = load_conf().unwrap();
        assert_eq!(
            conf_str(&map, "mode").as_deref(),
            Some("tun"),
            "the rollback save must not revert the concurrent mode edit made during restart"
        );
        assert_eq!(
            conf_str(&map, "transparent_proxy").as_deref(),
            Some("false")
        );
    }
}

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

mod http;
mod network_settings;
mod proxy_groups;
mod route_trace;
mod rule_presets;
mod storage;
mod subscription_jobs;
pub use subscription_jobs::worker as subscription_worker;

use std::env;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process::Command;
#[cfg(test)]
use std::sync::Arc;
use std::sync::OnceLock;
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
    let method = env::var("REQUEST_METHOD").unwrap_or_else(|_| "GET".into());
    let network = env::var("GATEWAY_INTERFACE").is_ok()
        && env::var("SOCKROCKET_LOCAL_RPC").as_deref() != Ok("1");
    let request_header = env::var("HTTP_X_SOCKROCKET_REQUEST").ok();
    let origin = env::var("HTTP_ORIGIN").ok();
    let host = env::var("HTTP_HOST").unwrap_or_default();
    let origin_ok = origin.is_none_or(|origin| {
        origin == format!("http://{host}") || origin == format!("https://{host}")
    });
    if (network && (request_header.as_deref() != Some("1") || !origin_ok))
        || (!read_only_action(&action) && method != "POST")
    {
        println!(
            "Status: 403 Forbidden\nContent-Type: application/json\n\n{{\"ok\":false,\"error\":\"request_denied\"}}"
        );
        return Ok(());
    }
    let post = if method == "POST" {
        read_post_body().and_then(|body| {
            if body.is_empty() {
                Ok(J::Null)
            } else {
                serde_json::from_str(&body).map_err(|error| {
                    let mut out = request_error(
                        "request_body_invalid",
                        "InvalidJson",
                        "Request JSON is invalid; refresh the page after installing the complete plugin package",
                    );
                    out["stage"] = json!("parse_request");
                    out["line"] = json!(error.line());
                    out["column"] = json!(error.column());
                    out
                })
            }
        })
    } else {
        Ok(J::Null)
    };

    // Parsing failures need the same JSON envelope as action failures. Do not
    // propagate them to main(), where stderr is discarded by firmware RPC.
    let out = match post {
        Ok(post) => dispatch(&action, &post),
        Err(error) => error,
    };
    println!("Content-Type: application/json");
    println!("Cache-Control: no-cache");
    println!();
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

fn request_error(code: &str, cause: &str, message: &str) -> J {
    json!({"ok":false,"error_code":code,"stage":"read_request","cause":cause,
        "error":message,"hint":"通过软件中心安装完整插件包后刷新页面；勿单独替换脚本或二进制。"})
}

fn read_post_body() -> std::result::Result<String, J> {
    let encoded = env::var("SOCKROCKET_LOCAL_RPC").as_deref() == Ok("1")
        && env::var("SOCKROCKET_RPC_ENCODING").as_deref() == Ok("base64");
    let limit = if encoded {
        MAX_CGI_BODY.div_ceil(3) * 4
    } else {
        MAX_CGI_BODY
    };
    let length = env::var("CONTENT_LENGTH")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|len| *len > 0);
    let too_large = || {
        let mut error = request_error(
            "request_body_too_large",
            "TooLarge",
            "Request body exceeds the 1 MiB decoded limit",
        );
        error["hint"] = json!("缩小单次提交内容后重试。");
        error
    };
    if length.is_some_and(|len| len > limit) {
        return Err(too_large());
    }
    // No Content-Length on dbus RPC: read to EOF with an encoded-size cap.
    let mut buf = Vec::new();
    std::io::stdin()
        .take(length.unwrap_or(limit + 1))
        .read_to_end(&mut buf)
        .map_err(|error| {
            request_error(
                "request_body_read_failed",
                &format!("{:?}", error.kind()),
                "Cannot read request body",
            )
        })?;
    if buf.len() as u64 > limit {
        return Err(too_large());
    }
    if length.is_some_and(|len| len != buf.len() as u64) {
        return Err(request_error(
            "request_body_invalid",
            "UnexpectedEof",
            "Request body was truncated",
        ));
    }
    if encoded {
        buf = B64.decode(&buf).map_err(|_| {
            request_error(
                "request_body_invalid",
                "InvalidBase64",
                "Request body encoding is invalid",
            )
        })?;
        if buf.len() as u64 > MAX_CGI_BODY {
            return Err(too_large());
        }
    }
    String::from_utf8(buf).map_err(|_| {
        request_error(
            "request_body_invalid",
            "InvalidUtf8",
            "Request body is not valid UTF-8",
        )
    })
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

fn run_sh(action: &str) -> Result<()> {
    // Editing/importing settings must not undo an explicit manual stop.
    // The Start action uses "start" and remains available.
    if action == "restart" && sockrocket_dir().join("stopped").exists() {
        return Ok(());
    }
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
    let status = cmd.status().context("Cannot invoke lifecycle controller")?;
    anyhow::ensure!(
        status.success(),
        "Lifecycle action {action} failed with {status}"
    );
    Ok(())
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
    let groups: Vec<sockrocket_core::ProxyGroupConfig> = serde_yaml::from_value(
        map.get(ykey("groups"))
            .cloned()
            .unwrap_or(Y::Sequence(vec![])),
    )
    .map_err(|_| anyhow::anyhow!("Invalid proxy groups"))?;
    sockrocket_core::config::group::validate_targets(&groups, &load_routing_rules(map))
        .map_err(anyhow::Error::msg)?;
    let text = serde_yaml::to_string(&Y::Mapping(map.clone()))?;
    storage::atomic_write(&text)
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
        .and_then(|n| u16::try_from(n).ok())
        .filter(|port| *port > 0)
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

fn read_only_action(action: &str) -> bool {
    matches!(
        action,
        "status"
            | "speedtest"
            | "speedtest_node"
            | "ip_check"
            | "check_update"
            | "diagnose"
            | "nodes"
            | "log"
            | "config"
            | "version"
            | "subscriptions"
            | "subscription_job"
            | "stats"
            | "node_details"
            | "sysinfo"
            | "get_direct_domains"
            | "get_rules"
            | "trace_route"
            | "proxy_groups"
            | "network_settings"
            | "dns_test"
            | "export_rules"
    )
}

#[cfg(test)]
use http::{ApiRequest, read_request, serve_api_authenticated};
pub use http::{DEFAULT_API_PORT, serve_api};

// === Actions ===

fn dispatch(action: &str, post: &J) -> J {
    let _transaction = if matches!(
        action,
        "stop" | "subscription_start" | "add_sub" | "update_subs"
    ) || read_only_action(action)
    {
        None
    } else {
        match storage::MutationGuard::acquire() {
            Ok(guard) => Some(guard),
            Err(e) => return err_msg(&format!("Configuration transaction failed: {e}")),
        }
    };
    dispatch_inner(action, post)
}

fn dispatch_inner(action: &str, post: &J) -> J {
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
        "subscription_start" => subscription_jobs::start(post),
        "subscription_job" => subscription_jobs::status(post),
        "add_sub" => act_add_sub(post),
        "del_sub" => act_del_sub(post),
        "rename_sub" => act_rename_sub(post),
        "update_subs" => act_update_subs(),
        "speedtest" => act_speedtest(),
        "stats" => act_stats(),
        "set_mode" => act_set_mode(post),
        "set_proxy_mode" => act_set_proxy_mode(post),
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
        "proxy_groups" => proxy_groups::get(),
        "save_proxy_group" => proxy_groups::save(post),
        "delete_proxy_group" => proxy_groups::delete(post),
        "save_device_policy" => proxy_groups::device(post),
        "network_settings" => network_settings::get(),
        "save_network_settings" => network_settings::save(post),
        "dns_test" => network_settings::test(post),
        "get_rules" => act_get_rules(),
        "add_rule_preset" => rule_presets::add(post),
        "trace_route" => route_trace::act_trace_route(post),
        "add_rule" => act_add_rule(post),
        "batch_add_rules" => act_batch_add_rules(post),
        "apply_rule_scene" => act_apply_rule_scene(post),
        "export_rules" => act_export_rules(),
        "del_rule" => act_del_rule(post),
        "move_rule" => act_move_rule(post),
        "update_rule" => act_update_rule(post),
        "set_rule_group_enabled" => act_set_rule_group_enabled(post),
        "set_rule_group_target" => act_set_rule_group_target(post),
        "delete_rule_group" => act_delete_rule_group(post),
        "replace_rule_group" => act_replace_rule_group(post),
        "check_update" => act_check_update(),
        "apply_update" => act_apply_update(),
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
        "proxy_mode": conf_str(&map, "proxy_mode")
            .unwrap_or_else(|| "rule".into())
            .to_ascii_lowercase(),
        "rules_revision":route_trace::rules_revision(&map),
        "network_revision":network_settings::revision(&map),
        "ipv6_mode":conf_str(&map,"ipv6_mode").unwrap_or_else(||"compatibility".into()),
        "ipv6_active":network_settings::ipv6_active(running, transparent_proxy, conf_str(&map,"ipv6_mode").as_deref()==Some("dual_stack")),
        "transparent_proxy": transparent_proxy,
        "dns_hijack": dns_hijack,
        "cn_ipset_direct": conf_bool(&map, "cn_ipset_direct", false),
        "version": version_string(),
        "health": health,
    })
}

fn prepare_manual_start() -> Result<()> {
    // Explicit Start restores the complete LAN path. Internal restarts and
    // watchdog recovery keep the user's independent toggle selections.
    let mut map = load_conf()?;
    map.insert(ykey("transparent_proxy"), Y::Bool(true));
    map.insert(ykey("dns_hijack"), Y::Bool(true));
    save_conf(&map)
}

fn act_start() -> J {
    if is_running() {
        return err_msg("Already running");
    }
    if prepare_manual_start().is_err() {
        return err_msg("无法保存启动设置，服务未启动，请检查配置与存储权限");
    }
    if let Err(error) = run_sh("start") {
        return err_msg(&error.to_string());
    }
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
    if let Err(error) = run_sh("stop") {
        return err_msg(&error.to_string());
    }
    // Belt: if DNS hijack / MARK rules somehow survived stop, force restore.
    if dnsmasq_hijack_conf().exists()
        && let Err(error) = run_sh("network-restore")
    {
        return err_msg(&error.to_string());
    }
    let stopped = !is_running();
    let hijack_gone = !dnsmasq_hijack_conf().exists();
    if stopped && hijack_gone {
        json!({"ok": true, "msg": "Sockrocket stopped — direct (non-proxy) network restored"})
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
    // An explicit Restart action may start a stopped service. Background
    // settings updates still use restart and preserve the stopped marker.
    let action = if is_running() {
        "restart"
    } else {
        if prepare_manual_start().is_err() {
            return err_msg("无法保存启动设置，服务未启动，请检查配置与存储权限");
        }
        "start"
    };
    if let Err(error) = run_sh(action) {
        return err_msg(&error.to_string());
    }
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
    // Use the startup parser, including rule/group target validation. A
    // structurally valid YAML file can still prevent the daemon from starting.
    let Ok(text) = String::from_utf8(bytes) else {
        return err_msg("Invalid config data (UTF-8 decode failed)");
    };
    if let Err(e) = validate_advanced_config(&text) {
        return json!({"ok": false, "msg": format!("Config validation failed: {e}")});
    }
    if let Err(error) =
        sockrocket_core::config::storage::atomic_write_private(&conf_path(), &text, true)
    {
        return err_msg(&format!("Failed to persist config: {error}"));
    }
    if is_running()
        && let Err(error) = run_sh("restart")
    {
        return err_msg(&error.to_string());
    }
    json!({"ok": true, "msg": "Config saved and validated"})
}

fn validate_advanced_config(text: &str) -> Result<()> {
    let config = sockrocket_core::AppConfig::from_yaml_str(text)?;
    if !config.nodes.is_empty() {
        anyhow::ensure!(
            config
                .active_node
                .is_none_or(|index| index < config.nodes.len()),
            "active_node is out of range"
        );
    }
    let raw: Y = serde_yaml::from_str(text)?;
    let dns = sockrocket_core::config::dns::DnsSettings::from_yaml(&raw)?;
    let transparent = raw
        .get("transparent_proxy")
        .and_then(Y::as_bool)
        .unwrap_or(true);
    anyhow::ensure!(
        !transparent
            || config.ipv6_mode != sockrocket_core::config::dns::Ipv6Mode::Compatibility
            || dns.strategy != sockrocket_core::config::dns::AddressStrategy::Ipv6Only,
        "IPv6-only DNS requires dual_stack in Merlin transparent mode"
    );
    Ok(())
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

/// Fetch + parse a subscription URL on a current-thread runtime (CGI / API
/// handlers are sync). Hard-capped at [`SUB_FETCH_TIMEOUT_SECS`] so the Web UI
/// never waits forever on a hung airport URL.
fn fetch_sub_nodes(
    name: &str,
    url: &str,
    format: &str,
) -> Result<sockrocket_core::config::subscription::SubscriptionFetchReport, String> {
    let sub = sockrocket_core::Subscription {
        name: name.to_string(),
        url: url.to_string(),
        format: format.to_string(),
        last_updated: None,
        refresh_interval_hours: 24,
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("Failed to start fetch runtime: {e}"))?;
    let timeout = Duration::from_secs(sockrocket_core::SUB_FETCH_TIMEOUT_SECS);
    rt.block_on(async {
        match tokio::time::timeout(
            timeout,
            sockrocket_core::config::subscription::fetch_subscription_report(&sub),
        )
        .await
        {
            Ok(Ok(nodes)) => Ok(nodes),
            Ok(Err(e)) => Err(format!("{e:#}")),
            Err(_) => Err(format!(
                "Timed out after {}s — check the URL or try again",
                sockrocket_core::SUB_FETCH_TIMEOUT_SECS
            )),
        }
    })
}

/// Write nodes, subscription URLs, and the active index back into config.yaml
/// without dropping router-only keys. Returns the saved node count.
fn persist_subscription_state(cfg: &sockrocket_core::AppConfig) -> Result<usize, String> {
    sockrocket_core::config::storage::write_nodes_and_subscriptions(&conf_path(), cfg)
        .map_err(|e| format!("Failed to persist config: {e}"))?;
    Ok(cfg.nodes.len())
}

fn load_app_config() -> Result<sockrocket_core::AppConfig, String> {
    let content =
        fs::read_to_string(conf_path()).map_err(|e| format!("Failed to read config: {e}"))?;
    serde_yaml::from_str(&content).map_err(|e| format!("Config parse failed: {e}"))
}

fn act_add_sub(post: &J) -> J {
    let name = post
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let url = post
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let format = post
        .get("format")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("auto");
    if sockrocket_core::config::subscription::validate_subscription_url(url).is_err() {
        return json!({"ok":false,"error_code":"subscription_url_invalid","stage":"validate_subscription",
            "cause":"InvalidUrl","msg":"订阅地址无效，未保存。","hint":"填写完整的 http 或 https 订阅地址；名称和格式可以留空。"});
    }
    if !matches!(
        format,
        "auto" | "clash" | "v2ray" | "singbox" | "sing-box" | "base64"
    ) {
        return err_msg("Unsupported subscription format");
    }
    let name = {
        // Only the read/modify/commit step needs the configuration lock.
        // Downloading under this lock made every other edit time out.
        let Ok(_transaction) = storage::MutationGuard::acquire() else {
            return err_msg("Configuration transaction busy; retry shortly");
        };
        let Ok(mut map) = load_conf() else {
            return err_msg("Config file not found");
        };
        let mut seq = match map.remove(ykey("subscriptions")) {
            Some(Y::Sequence(s)) => s,
            _ => Vec::new(),
        };
        if seq
            .iter()
            .any(|item| item.get(ykey("url")).and_then(Y::as_str) == Some(url))
        {
            return json!({"ok":false,"error_code":"subscription_exists","stage":"validate_subscription",
                "cause":"DuplicateSubscription","msg":"该订阅已存在，请更新已有订阅。",
                "hint":"在“我的订阅”中点击“全部更新”，无需重复添加；不会覆盖已有节点。"});
        }
        let name = if name.is_empty() {
            let used: std::collections::HashSet<_> = seq
                .iter()
                .filter_map(|entry| entry.get(ykey("name")).and_then(Y::as_str))
                .collect();
            let mut number = seq.len() + 1;
            while used.contains(format!("订阅 {number}").as_str()) {
                number += 1;
            }
            format!("订阅 {number}")
        } else {
            name.to_owned()
        };
        seq.push(sub_entry(&name, url, format));
        map.insert(ykey("subscriptions"), Y::Sequence(seq));
        if let Err(e) = save_conf(&map) {
            return json!({"ok": false, "msg": format!("Save failed: {e}")});
        }
        name
    };

    // Fetch + parse here (not only on restart) so the Web UI can show a real
    // error instead of "saved but still 0 nodes" after a silent background fail.
    subscription_jobs::progress(0, 1, "fetching");
    match fetch_sub_nodes(&name, url, format) {
        Ok(report) if report.nodes.is_empty() => json!({
            "ok": false,
            "msg": "Subscription saved, but parsed 0 nodes — check URL / format (try auto, clash, or v2ray)",
            "node_count": 0
        }),
        Ok(report) => {
            let warning = if report.skipped_entries > 0 {
                format!("；{} 条无效或不兼容的节点未导入", report.skipped_entries)
            } else {
                String::new()
            };
            let compatibility_notes = report.compatibility_notes;
            subscription_jobs::progress(1, 1, "applying");
            let nodes = report.nodes;
            let fetched = nodes.len();
            let saved = sockrocket_core::config::storage::commit_subscription_refresh(
                &conf_path(),
                vec![(url.to_string(), nodes)],
            )
            .map_err(|error| error.to_string());
            match saved {
                Ok(cfg) if cfg.subscriptions.iter().any(|source| source.url == url) => {
                    let total = cfg.nodes.len();
                    if let Err(error) = run_sh("restart") {
                        return err_msg(&error.to_string());
                    }
                    json!({
                        "ok": true,
                        "msg": format!("已导入 {fetched} 个节点，共 {total} 个，设置已应用{warning}"),
                        "node_count": total,
                        "compatibility_notes": compatibility_notes
                    })
                }
                Ok(_) => json!({"ok":false,"error_code":"subscription_removed",
                    "msg":"订阅在下载期间被删除；下载结果已丢弃，现有节点保持不变"}),
                Err(e) => json!({
                    "ok": false,
                    "msg": format!("Parsed {fetched} node(s) but failed to save: {e}")
                }),
            }
        }
        Err(e) => json!({
            "ok": false,
            "msg": format!("Subscription saved, but fetch/parse failed: {e}"),
            "node_count": 0
        }),
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
    let Ok(mut cfg) = load_app_config() else {
        return err_msg("Config file not found");
    };
    let Some(url) = cfg.subscriptions.get(index).map(|sub| sub.url.clone()) else {
        return err_msg("Subscription index out of range");
    };
    sockrocket_core::remove_subscription_nodes(&mut cfg, &url);
    match persist_subscription_state(&cfg) {
        Ok(_) => json!({"ok": true, "msg": "Subscription deleted"}),
        Err(e) => json!({"ok": false, "msg": format!("Save failed: {e}")}),
    }
}

/// Rename a subscription display name only — no fetch, no restart.
fn act_rename_sub(post: &J) -> J {
    let Some(index) = post
        .get("index")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
    else {
        return err_msg("Invalid parameters");
    };
    let name = post
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if name.is_empty() {
        return err_msg("Name must not be empty");
    }
    let Ok(mut map) = load_conf() else {
        return err_msg("Config file not found");
    };
    let mut seq = match map.remove(ykey("subscriptions")) {
        Some(Y::Sequence(s)) => s,
        _ => Vec::new(),
    };
    if index >= seq.len() {
        return err_msg("Subscription index out of range");
    }
    match &mut seq[index] {
        Y::Mapping(m) => {
            m.insert(ykey("name"), Y::String(name.to_string()));
        }
        _ => return err_msg("Invalid subscription entry"),
    }
    map.insert(ykey("subscriptions"), Y::Sequence(seq));
    match save_conf(&map) {
        Ok(()) => json!({"ok": true, "msg": "Subscription renamed", "name": name}),
        Err(e) => json!({"ok": false, "msg": format!("Save failed: {e}")}),
    }
}

fn act_update_subs() -> J {
    let map = {
        let Ok(_transaction) = storage::MutationGuard::acquire() else {
            return err_msg("Configuration transaction busy; retry shortly");
        };
        let Ok(map) = load_conf() else {
            return err_msg("Config file not found");
        };
        map
    };
    let subs = conf_seq(&map, "subscriptions");
    if subs.is_empty() {
        return err_msg("No subscriptions configured");
    }

    let mut refreshed = Vec::new();
    let mut errors = Vec::new();
    let mut compatibility_notes = Vec::new();
    let mut skipped_subscriptions = 0usize;
    for (position, s) in subs.iter().enumerate() {
        subscription_jobs::progress(position, subs.len(), "fetching");
        let get = |k: &str| match s.get(ykey(k)) {
            Some(Y::String(v)) => v.clone(),
            _ => String::new(),
        };
        let name = get("name");
        let url = get("url");
        let format = match get("format") {
            f if f.is_empty() => "auto".into(),
            f => f,
        };
        if sockrocket_core::config::subscription::validate_subscription_url(&url).is_err() {
            skipped_subscriptions += 1;
            errors.push(format!(
                "第 {} 个订阅地址无效，已跳过并保留已有节点",
                position + 1
            ));
            continue;
        }
        match fetch_sub_nodes(&name, &url, &format) {
            Ok(report) if report.nodes.is_empty() => {
                skipped_subscriptions += 1;
                errors.push(format!("'{name}': parsed 0 nodes"));
            }
            Ok(report) => {
                compatibility_notes.extend(report.compatibility_notes);
                if report.skipped_entries > 0 {
                    errors.push(format!(
                        "{} URI entries were not imported (unsupported options or invalid entries)",
                        report.skipped_entries
                    ));
                }
                refreshed.push((url, report.nodes));
            }
            Err(e) => {
                skipped_subscriptions += 1;
                errors.push(format!(
                    "第 {} 个订阅下载或解析失败，已跳过：{e}",
                    position + 1
                ));
            }
        }
    }

    if refreshed.is_empty() {
        let detail = if errors.is_empty() {
            "no nodes returned".into()
        } else {
            errors.join("; ")
        };
        return json!({
            "ok": false,
            "error_code":"subscription_update_failed",
            "stage":"fetching",
            "msg": format!("Update failed — {detail}"),
            "skipped_subscriptions": skipped_subscriptions,
            "node_count": conf_seq(&map, "nodes").len()
        });
    }

    subscription_jobs::progress(subs.len(), subs.len(), "applying");
    let fetched_by_url: std::collections::HashMap<_, _> = refreshed
        .iter()
        .map(|(url, nodes)| (url.clone(), nodes.len()))
        .collect();
    let cfg = match sockrocket_core::config::storage::commit_subscription_refresh(
        &conf_path(),
        refreshed,
    ) {
        Ok(cfg) => cfg,
        Err(error) => {
            return json!({"ok": false, "msg": format!("Fetched nodes but save failed: {error}")});
        }
    };
    let applied: Vec<_> = fetched_by_url
        .keys()
        .filter(|url| cfg.subscriptions.iter().any(|source| source.url == **url))
        .cloned()
        .collect();
    if applied.is_empty() {
        return json!({"ok":false,"error_code":"subscriptions_removed",
            "msg":"订阅在下载期间被删除；下载结果已丢弃，现有节点保持不变"});
    }
    let fetched: usize = applied.iter().map(|url| fetched_by_url[url]).sum();
    let dropped = fetched_by_url.len() - applied.len();
    if dropped > 0 {
        skipped_subscriptions += dropped;
        errors.push(format!("{dropped} 个订阅在下载期间被删除，其节点已丢弃"));
    }
    let total = cfg.nodes.len();
    if let Err(error) = run_sh("restart") {
        return err_msg(&error.to_string());
    }
    let warn = if errors.is_empty() {
        String::new()
    } else {
        format!("（部分内容未更新：{}）", errors.join("; "))
    };
    compatibility_notes.sort_unstable();
    compatibility_notes.dedup();
    json!({
        "ok": true,
        "msg": format!("已更新 {fetched} 个节点，共 {total} 个，设置已应用{warn}"),
        "node_count": total,
        "compatibility_notes": compatibility_notes,
        "skipped_subscriptions": skipped_subscriptions,
        "warnings": errors
    })
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
    let port = post
        .get("port")
        .and_then(|v| v.as_u64())
        .and_then(|port| u16::try_from(port).ok())
        .unwrap_or(0);
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
        "anytls" | "wireguard" | "shadowsocks" | "vmess" | "trojan" | "vless" | "hysteria2"
        | "tuic" => proto_raw.as_str(),
        _ => return err_msg("Unsupported protocol"),
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
        "anytls" | "trojan" | "hysteria2" => {
            pm.insert(ykey("password"), Y::String(password));
        }
        "tuic" => {
            pm.insert(ykey("uuid"), Y::String(uuid));
            pm.insert(ykey("password"), Y::String(password));
        }
        "wireguard" => {
            let Some(value) = post.get("wireguard") else {
                return err_msg("WireGuard configuration is required");
            };
            let config: sockrocket_core::config::model::WireguardConfig =
                match serde_json::from_value(value.clone()) {
                    Ok(c) => c,
                    Err(_) => return err_msg("Invalid WireGuard configuration"),
                };
            let value = serde_yaml::to_value(config).unwrap();
            if let Y::Mapping(fields) = value {
                pm.extend(fields);
            }
        }
        _ => unreachable!(),
    }
    let mut node = Mapping::new();
    node.insert(ykey("name"), Y::String(name.clone()));
    node.insert(ykey("server"), Y::String(server));
    node.insert(ykey("port"), Y::Number(port.into()));
    node.insert(ykey("protocol"), Y::Mapping(pm));
    if proto == "anytls" {
        let sni = get("sni");
        use sockrocket_core::config::model::{TlsConfig, TransportConfig, TransportType};
        let transport = serde_yaml::to_value(TransportConfig {
            transport_type: TransportType::Tls,
            tls: Some(TlsConfig {
                sni: (!sni.is_empty()).then_some(sni),
                skip_cert_verify: post
                    .get("skip_cert_verify")
                    .and_then(J::as_bool)
                    .unwrap_or(false),
                alpn: None,
                fingerprint: None,
            }),
            ws: None,
            reality: None,
        })
        .unwrap();
        node.insert(ykey("transport"), transport);
    }
    if matches!(proto, "anytls" | "wireguard") {
        let valid = serde_yaml::from_value::<sockrocket_core::config::model::Node>(Y::Mapping(
            node.clone(),
        ))
        .ok()
        .is_some_and(|node| {
            sockrocket_core::proxy::factory::create_outbound_unwarmed(&node).is_ok()
        });
        if !valid {
            return err_msg("Invalid protocol credentials, endpoint or transport");
        }
    }

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
            if is_running()
                && let Err(error) = run_sh("restart")
            {
                return err_msg(&error.to_string());
            }
            json!({"ok": true, "msg": "Mode set; takes effect after restart"})
        }
        Err(e) => json!({"ok": false, "msg": format!("Save failed: {e}")}),
    }
}

/// Set routing mode: `rule` | `global` | `direct` (same as desktop GUI).
///
/// Global turns off `cn_ipset_direct` so China IPs are not diverted around TUN
/// at the iptables layer (otherwise "global" would still bypass domestic).
fn act_set_proxy_mode(post: &J) -> J {
    let mode = post
        .get("proxy_mode")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if !matches!(mode.as_str(), "rule" | "global" | "direct") {
        return err_msg("Invalid proxy_mode (rule / global / direct)");
    }
    let Ok(mut map) = load_conf() else {
        return err_msg("Config file not found");
    };
    let prev = conf_str(&map, "proxy_mode").unwrap_or_else(|| "rule".into());
    if prev.eq_ignore_ascii_case(&mode) {
        return json!({"ok": true, "msg": format!("Already in {mode} mode"), "proxy_mode": mode});
    }
    map.insert(ykey("proxy_mode"), Y::String(mode.clone()));
    if mode == "global" {
        map.insert(ykey("cn_ipset_direct"), Y::Bool(false));
    }
    if let Err(e) = save_conf(&map) {
        return err_msg(&format!("Save failed: {e}"));
    }
    if mode == "global" && is_running() {
        // Drop CN ipset bypass if it was on.
        let _ = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!(
                "\"{}/scripts/iptables.sh\" ipset-off 2>/dev/null || true",
                sockrocket_dir().display()
            ))
            .status();
    }
    if is_running() {
        if let Err(error) = run_sh("restart") {
            return err_msg(&error.to_string());
        }
        if !is_running() {
            return json!({
                "ok": false,
                "msg": "Saved mode but service failed to restart — check logs",
                "proxy_mode": mode
            });
        }
    }
    let label = match mode.as_str() {
        "global" => "Global (all via proxy)",
        "direct" => "Direct (no proxy)",
        _ => "Rule (load rules + China-direct)",
    };
    json!({"ok": true, "msg": format!("Proxy mode: {label}"), "proxy_mode": mode})
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
    let (cur_proxy, mut cur_dns) = effective_toggles(&map);
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
            // Full transparent path: TUN + DNS hijack. Fake-IP only exists
            // with --tun; without TUN, hijack still serves short-TTL real IPs
            // (safe). TUN without hijack leaves clients on polluted ISP DNS.
            map.insert(ykey("dns_hijack"), Y::Bool(true));
            if let Err(e) = save_conf(&map) {
                return err_msg(&format!("Save failed: {e}"));
            }
            // restart brings up --tun + iptables + dnsmasq per the keys.
            // do_start rewrites `mode` on disk while we are in here.
            if let Err(error) = run_sh("restart") {
                return err_msg(&error.to_string());
            }
            let ok = run_cmd(
                "iptables",
                &["-t", "mangle", "-L", "SOCKROCKET_MANGLE", "-n"],
            )
            .map(|o| o.contains("MARK"))
            .unwrap_or(false);
            let dns_ok = dnsmasq_hijack_conf().exists();
            if !ok || !dns_ok {
                // Re-read before the rollback for the same reason: the disk
                // state changed during run_sh.
                let mut map = match load_conf() {
                    Ok(m) => m,
                    Err(e) => return err_msg(&format!("Failed to read config: {e}")),
                };
                map.insert(ykey("transparent_proxy"), Y::Bool(false));
                // Keep DNS hijack on so LAN stays on short-TTL sockrocket DNS
                // (no Fake-IP without --tun) instead of ISP long-TTL.
                map.insert(ykey("dns_hijack"), Y::Bool(true));
                let _ = save_conf(&map);
                // Config now says TUN off — tear MARK rules and restart plain
                // daemon; DNS hijack is re-applied by do_start when wanted.
                if let Err(error) = run_sh("proxy-off") {
                    return err_msg(&error.to_string());
                }
                return json!({"ok": false, "msg": "Failed to enable transparent proxy: TUN/iptables/DNS hijack not ready (check the tun module and logs)"});
            }
        } else {
            // Turn TUN/iptables off but KEEP dns_hijack. Without --tun the
            // daemon serves real IPs (no Fake-IP) with short TTL — LAN stays
            // online, and re-enabling TUN recovers in seconds instead of
            // waiting out ISP DNS TTLs (~60s+).
            if let Err(error) = run_sh("proxy-off") {
                return err_msg(&error.to_string());
            }
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
                return json!({"ok": false, "msg": "Failed to disable transparent proxy: iptables MARK still present"});
            }
            // Refresh cur_dns from disk (proxy-off / do_start may leave it on).
            if let Ok(m) = load_conf() {
                let (_, d) = effective_toggles(&m);
                cur_dns = d;
            }
            // Postcondition: if DNS is supposed to stay on, the hijack file
            // must exist. Heal with dns-on; do not flip the key off (that
            // would push clients onto ISP long TTLs).
            let dns_should_stay = want_dns.unwrap_or(cur_dns);
            if dns_should_stay && sockrocket_sh().is_file() && !dnsmasq_hijack_conf().exists() {
                if let Err(error) = run_sh("dns-on") {
                    return err_msg(&error.to_string());
                }
                if !dnsmasq_hijack_conf().exists() {
                    return json!({
                        "ok": false,
                        "msg": "Transparent proxy is off, but DNS hijack is missing — LAN may be on ISP DNS. Check that the DNS port is listening, then toggle DNS hijack on."
                    });
                }
            }
        }
        if new_proxy {
            cur_dns = true;
        }
    }

    if let Some(new_dns) = want_dns
        && new_dns != cur_dns
    {
        // DNS hijack without TUN is OK: Fake-IP is only enabled with --tun.
        // SOCKS-mode hijack serves short-TTL real IPs (china-split).
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
        if let Err(error) = run_sh(if new_dns { "dns-on" } else { "dns-off" }) {
            return err_msg(&error.to_string());
        }
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

/// Subscription table = current members' node history, not the side ledger.
/// The side ledger misses traffic recorded before a node was linked to a subscription.
fn subscription_month_rows(
    nodes: &[sockrocket_core::NodeMonthlyTraffic],
    traffic_path: &std::path::Path,
) -> Vec<sockrocket_core::NodeMonthlyTraffic> {
    let Ok(cfg) = load_app_config() else {
        return sockrocket_core::TrafficLedger::read_recent_subscriptions(traffic_path, 3)
            .unwrap_or_default();
    };
    let subscription_names: Vec<(String, String)> = cfg
        .subscriptions
        .iter()
        .map(|sub| {
            (
                sockrocket_core::subscription_public_id(&sub.url),
                sub.name.clone(),
            )
        })
        .collect();
    let known: std::collections::HashSet<&str> = subscription_names
        .iter()
        .map(|(id, _)| id.as_str())
        .collect();
    let membership: Vec<(String, String)> = cfg
        .nodes
        .iter()
        .filter_map(|node| {
            let url = node
                .extra
                .get(sockrocket_core::SUB_URL_EXTRA_KEY)
                .map(String::as_str)
                .filter(|url| !url.is_empty())?;
            let sub_id = sockrocket_core::subscription_public_id(url);
            if !known.contains(sub_id.as_str()) {
                return None;
            }
            Some((
                sockrocket_core::node_public_id(&sockrocket_core::node_fingerprint(node)),
                sub_id,
            ))
        })
        .collect();
    sockrocket_core::sum_subscription_members(nodes, &membership, &subscription_names)
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
    let traffic_path = sockrocket_dir().join("traffic.yaml");
    let traffic_nodes: Vec<J> = sockrocket_core::TrafficLedger::read_file(&traffic_path)
        .unwrap_or_default()
        .into_iter()
        .take(40)
        .map(|snap| {
            json!({
                "id": snap.id,
                "name": sockrocket_core::public_traffic_label(&snap.name, if snap.id == sockrocket_core::DIRECT_TRAFFIC_ID { "直连" } else { "节点" }),
                "session_up": snap.session.up,
                "session_down": snap.session.down,
                "session_total": snap.session.total(),
            })
        })
        .collect();
    let month_keys = sockrocket_core::recent_month_keys(3);
    let recent = |rows: Vec<sockrocket_core::NodeMonthlyTraffic>, include_id: bool| -> Vec<J> {
        rows.into_iter()
            .take(40)
            .map(|row| {
                let fallback = if row.id == sockrocket_core::DIRECT_TRAFFIC_ID {
                    "直连"
                } else if include_id {
                    "节点"
                } else {
                    "订阅"
                };
                json!({
                    "id": if include_id { row.id } else { String::new() },
                    "name": sockrocket_core::public_traffic_label(&row.name, fallback),
                    "months": row.months.iter().map(|(key, counters)| json!({
                        "key": key,
                        "up": counters.up,
                        "down": counters.down,
                        "total": counters.total(),
                    })).collect::<Vec<_>>(),
                    "total_up": row.total.up,
                    "total_down": row.total.down,
                    "total": row.total.total(),
                })
            })
            .collect()
    };
    let recent_months =
        sockrocket_core::TrafficLedger::read_recent_months(&traffic_path, 3).unwrap_or_default();
    let recent_subscriptions = subscription_month_rows(&recent_months, &traffic_path);
    let session_started =
        sockrocket_core::TrafficLedger::read_session_started(&traffic_path).unwrap_or(0);
    json!({
        "ok": true,
        "uptime": uptime,
        "pid": pid_val,
        "dns_active": dns_active,
        "fw_active": fw_active,
        "traffic": {
            "session_started": session_started,
            "nodes": traffic_nodes,
            "recent_month_keys": month_keys,
            "recent_months": recent(recent_months, true),
            "recent_subscriptions": recent(recent_subscriptions, false),
        }
    })
}

fn vm_rss_kb(pid: &str) -> u64 {
    fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(0)
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
    // Per-process RSS so the UI can show Sockrocket's own footprint vs
    // system-wide MemAvailable pressure (often ~40–50% on Merlin even idle).
    let daemon_rss_kb = fs::read_to_string(pid_path())
        .ok()
        .map(|p| vm_rss_kb(p.trim()))
        .filter(|_| is_running())
        .unwrap_or(0);
    let api_rss_kb = vm_rss_kb(&std::process::id().to_string());
    json!({
        "ok": true,
        "mem_total_kb": mem_total,
        "mem_used_kb": mem_used,
        "mem_pct": mem_pct,
        "daemon_rss_kb": daemon_rss_kb,
        "api_rss_kb": api_rss_kb,
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
            if is_running()
                && let Err(error) = run_sh("restart")
            {
                return err_msg(&error.to_string());
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
    "dst-port",
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
        "dst-port" => {
            let (min, max) = if let Some((a, b)) = p.split_once('-') {
                (
                    a.trim()
                        .parse::<u16>()
                        .map_err(|_| "Invalid port range (e.g. 443 or 1000-2000)")?,
                    b.trim()
                        .parse::<u16>()
                        .map_err(|_| "Invalid port range (e.g. 443 or 1000-2000)")?,
                )
            } else {
                let port = p
                    .parse::<u16>()
                    .map_err(|_| "Invalid port (e.g. 443 or 1000-2000)")?;
                (port, port)
            };
            let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
            Ok(if lo == hi {
                lo.to_string()
            } else {
                format!("{lo}-{hi}")
            })
        }
        "final" => Ok("*".to_string()),
        _ => Err(format!("Unknown rule type: {rule_type}")),
    }
}

fn yaml_to_routing_rule(r: &Y) -> Option<sockrocket_core::RoutingRule> {
    Some(sockrocket_core::RoutingRule {
        source: r
            .get(ykey("source"))
            .and_then(Y::as_str)
            .unwrap_or("")
            .to_string(),
        name: r
            .get(ykey("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        group: r
            .get(ykey("group"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        rule_type: r.get(ykey("rule_type"))?.as_str()?.to_string(),
        pattern: r.get(ykey("pattern"))?.as_str()?.to_string(),
        target: r
            .get(ykey("target"))
            .and_then(|v| v.as_str())
            .unwrap_or("proxy")
            .to_string(),
        enabled: r
            .get(ykey("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        priority: r
            .get(ykey("priority"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32,
    })
}

fn routing_rule_to_yaml(r: sockrocket_core::RoutingRule) -> Y {
    let mut entry = Mapping::new();
    if !r.source.is_empty() {
        entry.insert(ykey("source"), Y::String(r.source));
    }
    if !r.name.is_empty() {
        entry.insert(ykey("name"), Y::String(r.name));
    }
    if !r.group.is_empty() {
        entry.insert(ykey("group"), Y::String(r.group));
    }
    entry.insert(ykey("rule_type"), Y::String(r.rule_type));
    entry.insert(ykey("pattern"), Y::String(r.pattern));
    entry.insert(ykey("target"), Y::String(r.target));
    entry.insert(ykey("priority"), Y::Number(r.priority.into()));
    entry.insert(ykey("enabled"), Y::Bool(r.enabled));
    Y::Mapping(entry)
}

fn load_routing_rules(map: &Mapping) -> Vec<sockrocket_core::RoutingRule> {
    map.get(ykey("rules"))
        .and_then(|v| v.as_sequence())
        .map(|seq| seq.iter().filter_map(yaml_to_routing_rule).collect())
        .unwrap_or_default()
}

fn save_routing_rules(map: &mut Mapping, rules: Vec<sockrocket_core::RoutingRule>) {
    let seq: Vec<Y> = rules.into_iter().map(routing_rule_to_yaml).collect();
    map.insert(ykey("rules"), Y::Sequence(seq));
}

fn same_rule_group(rule_group: &str, want: &str) -> bool {
    let g = rule_group.trim();
    let w = want.trim();
    if w.is_empty() { g.is_empty() } else { g == w }
}

fn rule_kind(rule_type: &str) -> &str {
    if matches!(rule_type, "match" | "final") {
        "final"
    } else {
        rule_type
    }
}

fn requested_rule_source(post: &J, default: &str) -> std::result::Result<String, String> {
    let value = match post.get("source") {
        None => default,
        Some(value) => value.as_str().ok_or("来源必须是 IP 地址或 CIDR 字符串")?,
    };
    sockrocket_core::router::normalize_rule_source(value)
}

fn requested_rule_priority(post: &J, default: i32) -> std::result::Result<i32, &'static str> {
    match post.get("priority") {
        None => Ok(default),
        Some(value) => value
            .as_i64()
            .and_then(|n| i32::try_from(n).ok())
            .ok_or("优先级必须是 -2147483648 到 2147483647 之间的整数"),
    }
}

fn rule_edit_snapshot(rule: &sockrocket_core::RoutingRule) -> J {
    json!({"source":rule.source,"name":rule.name,"group":rule.group,"rule_type":rule.rule_type,
        "pattern":rule.pattern,"target":rule.target,"priority":rule.priority,"enabled":rule.enabled})
}

fn stale_rule_request(post: &J, rule: &Y) -> bool {
    post.get("expected").is_some_and(|expected| {
        yaml_to_routing_rule(rule).is_none_or(|rule| expected != &rule_edit_snapshot(&rule))
    })
}

/// Read the `rules:` sequence as JSON rows for the UI.
fn act_get_rules() -> J {
    let Ok(map) = load_conf() else {
        return err_msg("规则配置读取失败，请检查诊断后重试");
    };
    let parsed = load_routing_rules(&map);
    if map
        .get(ykey("rules"))
        .is_some_and(|v| !matches!(v, Y::Null | Y::Sequence(_)))
        || parsed.len() != conf_seq(&map, "rules").len()
    {
        return err_msg("部分规则配置损坏，未返回不完整的规则列表，请检查诊断");
    }
    let rules: Vec<J> = parsed
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            json!({
                "index": i,
                "source": r.source,
                "name": r.name,
                "group": r.group,
                "rule_type": r.rule_type,
                "pattern": r.pattern,
                "target": r.target,
                "priority": r.priority,
                "enabled": r.enabled,
            })
        })
        .collect();
    json!({"ok": true, "rules": rules, "revision": route_trace::rules_revision(&map), "presets":rule_presets::catalog(), "proxy_groups":proxy_groups::catalog(&map), "nodes":proxy_groups::node_catalog(&map), "proxy_mode":conf_str(&map,"proxy_mode").unwrap_or_else(||"rule".into()).to_ascii_lowercase()})
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
    if !proxy_groups::valid_target(target) {
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
    let source = match requested_rule_source(post, "") {
        Ok(value) => value,
        Err(error) => return err_msg(&error),
    };
    // Duplicate check: same type+pattern already present.
    let dup = seq.iter().any(|r| {
        r.get(ykey("rule_type")).and_then(|v| v.as_str()) == Some(rule_type)
            && r.get(ykey("pattern")).and_then(|v| v.as_str()) == Some(pattern.as_str())
            && r.get(ykey("source")).and_then(Y::as_str).unwrap_or("") == source
    });
    if dup {
        return json!({"ok": false, "msg": format!("Rule already exists: {rule_type} {pattern}")});
    }

    let mut entry = Mapping::new();
    if !source.is_empty() {
        entry.insert(ykey("source"), Y::String(source));
    }
    let name = post
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if !name.is_empty() {
        entry.insert(ykey("name"), Y::String(name.to_string()));
    }
    let group = post
        .get("group")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if !group.is_empty() {
        entry.insert(ykey("group"), Y::String(group.to_string()));
    }
    entry.insert(ykey("rule_type"), Y::String(rule_type.to_string()));
    entry.insert(ykey("pattern"), Y::String(pattern.clone()));
    entry.insert(ykey("target"), Y::String(target.to_string()));
    let priority = match requested_rule_priority(post, 0) {
        Ok(n) => n,
        Err(e) => return err_msg(e),
    };
    entry.insert(ykey("priority"), Y::Number(priority.into()));
    entry.insert(ykey("enabled"), Y::Bool(true));
    // Higher priority first: insert at front of equal-priority group so UI
    // "new rule" stays ahead of older same-priority rules (list order tie-break).
    let insert_at = seq
        .iter()
        .position(|r| {
            r.get(ykey("priority"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0) as i32
                <= priority
        })
        .unwrap_or(seq.len());
    seq.insert(insert_at, Y::Mapping(entry));
    map.insert(ykey("rules"), Y::Sequence(seq));

    match save_conf(&map) {
        Ok(()) => {
            // The TUN-side rule override reads the router built at daemon
            // start, so rules need a full restart to take effect there.
            if is_running()
                && let Err(error) = run_sh("restart")
            {
                return err_msg(&error.to_string());
            }
            json!({"ok": true, "msg": format!("Added {rule_type} {pattern} -> {target} (takes effect after service restart)")})
        }
        Err(e) => json!({"ok": false, "msg": format!("Save failed: {e}")}),
    }
}

/// Edit in place; an expected snapshot prevents a stale browser editing another rule.
fn act_update_rule(post: &J) -> J {
    let Some(index) = post
        .get("index")
        .and_then(J::as_u64)
        .and_then(|n| usize::try_from(n).ok())
    else {
        return err_msg("Invalid rule index");
    };
    let Ok(mut map) = load_conf() else {
        return err_msg("Config file not found");
    };
    let mut rules = load_routing_rules(&map);
    let Some(old) = rules.get(index) else {
        return err_msg("Rule index out of range");
    };
    let snapshot = rule_edit_snapshot(old);
    if post.get("expected") != Some(&snapshot) {
        return err_msg("规则已变化，请刷新后重新编辑");
    }
    let typ = post.get("rule_type").and_then(J::as_str).unwrap_or("");
    let target = post.get("target").and_then(J::as_str).unwrap_or("");
    if !RULE_TYPES.contains(&typ) || !proxy_groups::valid_target(target) {
        return err_msg("Invalid rule type or action");
    }
    let pattern =
        match valid_rule_pattern(typ, post.get("pattern").and_then(J::as_str).unwrap_or("")) {
            Ok(p) => p,
            Err(e) => return err_msg(&e),
        };
    let source = match requested_rule_source(post, &old.source) {
        Ok(value) => value,
        Err(error) => return err_msg(&error),
    };
    if rules.iter().enumerate().any(|(i, r)| {
        i != index
            && rule_kind(&r.rule_type) == rule_kind(typ)
            && r.pattern == pattern
            && r.source == source
    }) {
        return err_msg("相同来源、类型和匹配内容的规则已存在");
    }
    let priority = match requested_rule_priority(post, old.priority) {
        Ok(n) => n,
        Err(e) => return err_msg(e),
    };
    let mut updated = old.clone();
    updated.source = source;
    updated.priority = priority;
    updated.rule_type = typ.to_string();
    updated.pattern = pattern;
    updated.target = target.to_string();
    updated.group = post
        .get("group")
        .and_then(J::as_str)
        .unwrap_or(&old.group)
        .trim()
        .to_string();
    updated.enabled = post
        .get("enabled")
        .and_then(J::as_bool)
        .unwrap_or(old.enabled);
    rules[index] = updated;
    save_routing_rules(&mut map, rules);
    match save_conf(&map) {
        Ok(()) => {
            if is_running()
                && let Err(error) = run_sh("restart")
            {
                return err_msg(&error.to_string());
            }
            json!({"ok": true, "msg": "规则已保存"})
        }
        Err(e) => err_msg(&format!("Save failed: {e}")),
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
    if stale_rule_request(post, &seq[index]) {
        return err_msg("规则已变化，请刷新后重试");
    }
    seq.remove(index);
    map.insert(ykey("rules"), Y::Sequence(seq));
    match save_conf(&map) {
        Ok(()) => {
            if is_running()
                && let Err(error) = run_sh("restart")
            {
                return err_msg(&error.to_string());
            }
            json!({"ok": true, "msg": "Deleted (takes effect after service restart)"})
        }
        Err(e) => json!({"ok": false, "msg": format!("Save failed: {e}")}),
    }
}

/// Swap a rule with its neighbour (`dir`: -1 = up / higher in list, +1 = down).
/// List order is the tie-breaker for equal `priority` values.
fn act_move_rule(post: &J) -> J {
    let Some(index) = post
        .get("index")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
    else {
        return err_msg("Invalid parameters");
    };
    let dir = post.get("dir").and_then(|v| v.as_i64()).unwrap_or(0);
    if dir != -1 && dir != 1 {
        return err_msg("dir must be -1 (up) or 1 (down)");
    }
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
    if stale_rule_request(post, &seq[index]) {
        return err_msg("规则已变化，请刷新后重试");
    }
    // Move against the same effective order shown by the WebUI, rather than
    // swapping unrelated YAML neighbours when priorities differ.
    let mut indexed: Vec<_> = seq.into_iter().enumerate().collect();
    let final_rule = |r: &Y| {
        matches!(
            r.get(ykey("rule_type")).and_then(Y::as_str),
            Some("final" | "match")
        )
    };
    let phase = |r: &Y| {
        u8::from(
            r.get(ykey("source"))
                .and_then(Y::as_str)
                .unwrap_or("")
                .is_empty(),
        ) * 2
            + u8::from(final_rule(r))
    };
    indexed.sort_by(|a, b| {
        phase(&a.1)
            .cmp(&phase(&b.1))
            .then_with(|| {
                let priority = |r: &Y| r.get(ykey("priority")).and_then(Y::as_i64).unwrap_or(0);
                priority(&b.1).cmp(&priority(&a.1))
            })
            .then_with(|| a.0.cmp(&b.0))
    });
    let index = indexed
        .iter()
        .position(|(original, _)| *original == index)
        .expect("validated index");
    seq = indexed.into_iter().map(|(_, rule)| rule).collect();
    let other = index as isize + dir as isize;
    if other < 0 || other as usize >= seq.len() {
        return err_msg("Already at edge");
    }
    if phase(&seq[index]) != phase(&seq[other as usize]) {
        return err_msg("设备规则、全局规则和最终兜底不能跨阶段移动");
    }
    seq.swap(index, other as usize);
    // Keep YAML priority aligned with list order (same as desktop GUI).
    let n = seq.len() as i64;
    for (i, item) in seq.iter_mut().enumerate() {
        if let Y::Mapping(m) = item {
            m.insert(ykey("priority"), Y::Number((n - i as i64).into()));
        }
    }
    map.insert(ykey("rules"), Y::Sequence(seq));
    match save_conf(&map) {
        Ok(()) => {
            if is_running()
                && let Err(error) = run_sh("restart")
            {
                return err_msg(&error.to_string());
            }
            json!({"ok": true, "msg": "Order updated (takes effect after service restart)"})
        }
        Err(e) => json!({"ok": false, "msg": format!("Save failed: {e}")}),
    }
}

/// Batch paste / import: multiline text → many rules (same parser as desktop GUI).
fn act_batch_add_rules(post: &J) -> J {
    let text = post.get("text").and_then(|v| v.as_str()).unwrap_or("");
    if text.trim().is_empty() {
        return err_msg("Paste one or more rules first");
    }
    let default_target = post
        .get("target")
        .and_then(|v| v.as_str())
        .unwrap_or("proxy");
    if !proxy_groups::valid_target(default_target) {
        return err_msg("Invalid target action (direct / proxy / reject)");
    }
    let replace = post
        .get("replace")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let group = post
        .get("group")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let rule_type = post
        .get("rule_type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    // Unified Clash-style parse (`TYPE,pattern,TARGET`). Plain domains/CIDRs
    // still auto-detect; typed paste without TYPE is accepted when `rule_type`
    // is set (WebUI type dropdown) for one-line-per-pattern convenience.
    let looks_clash = text.lines().map(str::trim).any(|l| {
        let head = l.split([',', ' ', ':']).next().unwrap_or("");
        matches!(
            head.to_ascii_uppercase().as_str(),
            "SOURCE"
                | "DOMAIN"
                | "DOMAIN-SUFFIX"
                | "DOMAIN-KEYWORD"
                | "IP-CIDR"
                | "GEOIP"
                | "DST-PORT"
                | "PORT"
                | "MATCH"
                | "FINAL"
        )
    });
    let parsed = if looks_clash || rule_type.is_empty() {
        sockrocket_core::parse_batch_rules_in_group(text, default_target, group)
    } else if sockrocket_core::rule_type_supports_bulk(rule_type) {
        sockrocket_core::parse_typed_patterns(text, rule_type, default_target, group)
    } else {
        sockrocket_core::parse_batch_rules_in_group(text, default_target, group)
    };
    if let Some(error) = parsed.errors.first() {
        return err_msg(&format!(
            "第 {} 行：{}；未保存任何更改",
            error.line, error.message
        ));
    }
    if parsed.rules.is_empty() {
        let msg = parsed
            .errors
            .first()
            .map(|e| format!("No valid rules (line {}: {})", e.line, e.message))
            .unwrap_or_else(|| "No valid rules".into());
        return err_msg(&msg);
    }
    let Ok(mut map) = load_conf() else {
        return err_msg("Config file not found");
    };
    let mut existing = load_routing_rules(&map);
    let conflict = if replace {
        sockrocket_core::BatchConflict::Replace
    } else {
        sockrocket_core::BatchConflict::Skip
    };
    let priority = match requested_rule_priority(post, 0) {
        Ok(n) => n,
        Err(e) => return err_msg(e),
    };
    let source = match requested_rule_source(post, "") {
        Ok(value) => value,
        Err(error) => return err_msg(&error),
    };
    let mut parsed = parsed;
    for rule in &mut parsed.rules {
        if post.get("source").is_some() {
            if !rule.source.is_empty() && rule.source != source {
                return err_msg("每行 SOURCE 与所选来源不一致；未保存任何更改");
            }
            rule.source.clone_from(&source);
        }
        rule.priority = if post.get("priority").is_some() {
            priority
        } else {
            existing
                .iter()
                .find(|r| {
                    rule_kind(&r.rule_type) == rule_kind(&rule.rule_type)
                        && r.pattern == rule.pattern
                        && r.source == rule.source
                })
                .map_or(0, |r| r.priority)
        };
    }
    let n_in = parsed.rules.len();
    let stats = sockrocket_core::merge_batch_rules(&mut existing, parsed.rules, conflict);
    save_routing_rules(&mut map, existing);
    match save_conf(&map) {
        Ok(()) => {
            if is_running()
                && let Err(error) = run_sh("restart")
            {
                return err_msg(&error.to_string());
            }
            let msg = format!(
                "Batch: +{} added, {} replaced, {} duplicates (from {} parsed)",
                stats.added, stats.replaced, stats.skipped, n_in
            );
            json!({"ok": true, "msg": msg})
        }
        Err(e) => json!({"ok": false, "msg": format!("Save failed: {e}")}),
    }
}

fn act_apply_rule_scene(post: &J) -> J {
    let id = post.get("scene").and_then(|v| v.as_str()).unwrap_or("");
    let Some(scene) = sockrocket_core::rule_scene_by_id(id) else {
        return err_msg("Unknown scene");
    };
    let Ok(mut map) = load_conf() else {
        return err_msg("Config file not found");
    };
    let mut existing = load_routing_rules(&map);
    let group_label = match id {
        "cross_border" => "Cross-border",
        "domestic_direct" => "Domestic direct",
        other => other,
    };
    let mut incoming = scene.rules_in_group(group_label);
    if let Some(target) = post.get("target").and_then(J::as_str) {
        if !proxy_groups::valid_target(target) {
            return err_msg("Invalid strategy target");
        }
        for rule in &mut incoming {
            if rule.target == "proxy" {
                rule.target = target.into();
            }
        }
    }
    let source = match requested_rule_source(post, "") {
        Ok(v) => v,
        Err(e) => return err_msg(&e),
    };
    for rule in &mut incoming {
        rule.source.clone_from(&source);
    }
    let n = incoming.len();
    let stats = sockrocket_core::merge_batch_rules(
        &mut existing,
        incoming,
        sockrocket_core::BatchConflict::Skip,
    );
    save_routing_rules(&mut map, existing);
    match save_conf(&map) {
        Ok(()) => {
            if is_running()
                && let Err(error) = run_sh("restart")
            {
                return err_msg(&error.to_string());
            }
            json!({
                "ok": true,
                "msg": format!(
                    "Scene「{group_label}」: +{} of {} rules ({} already present)",
                    stats.added, n, stats.skipped
                )
            })
        }
        Err(e) => json!({"ok": false, "msg": format!("Save failed: {e}")}),
    }
}

fn act_export_rules() -> J {
    let Ok(map) = load_conf() else {
        return err_msg("Config file not found");
    };
    let rules = load_routing_rules(&map);
    let text = sockrocket_core::format_rules_export(&rules);
    json!({"ok": true, "text": text, "count": rules.len()})
}

fn act_set_rule_group_target(post: &J) -> J {
    let group = post
        .get("group")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let Some(target) = post.get("target").and_then(|v| v.as_str()) else {
        return err_msg("请选择分组出口");
    };
    if !proxy_groups::valid_target(target) || target == "direct" || target == "reject" {
        return err_msg("分组出口只能是全局节点、一个节点，或已有的节点分组");
    }
    let Ok(mut map) = load_conf() else {
        return err_msg("Config file not found");
    };
    let mut rules = load_routing_rules(&map);
    if rules.len() != conf_seq(&map, "rules").len() {
        return err_msg("规则配置损坏，请先修复后再修改分组出口");
    }
    if let Some(expected) = post.get("expected_rules") {
        let current: Vec<_> = rules
            .iter()
            .filter(|r| same_rule_group(&r.group, &group))
            .map(rule_edit_snapshot)
            .collect();
        if expected != &json!(current) {
            return err_msg("分组规则已变化，请刷新后重新选择出口");
        }
    }
    let changed = match sockrocket_core::assign_group_outlet(&mut rules, &group, target) {
        Ok(changed) => changed,
        Err(_) => return err_msg("无法设置这个分组的出口"),
    };
    if changed == 0 {
        return json!({"ok": true, "msg": "分组出口未变化"});
    }
    save_routing_rules(&mut map, rules);
    match save_conf(&map) {
        Ok(()) => {
            if is_running()
                && let Err(error) = run_sh("restart")
            {
                return json!({"ok":false,"saved":true,"applied":false,"error_code":"rule_apply_failed","msg":"分组出口已保存，但代理重新加载失败；请查看诊断后重试应用。","detail":error.to_string()});
            }
            json!({"ok": true, "saved": true, "applied": is_running(), "msg": format!("已更新 {changed} 条规则的出口")})
        }
        Err(e) => json!({"ok": false, "msg": format!("Save failed: {e}")}),
    }
}

fn act_set_rule_group_enabled(post: &J) -> J {
    let group = post
        .get("group")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let Some(enabled) = post.get("enabled").and_then(|v| v.as_bool()) else {
        return err_msg("请明确选择开启或停用分组");
    };
    let Ok(mut map) = load_conf() else {
        return err_msg("Config file not found");
    };
    let mut rules = load_routing_rules(&map);
    if rules.len() != conf_seq(&map, "rules").len() {
        return err_msg("规则配置损坏，请先修复后再修改分组状态");
    }
    if let Some(expected) = post.get("expected_rules") {
        let current: Vec<_> = rules
            .iter()
            .filter(|r| same_rule_group(&r.group, &group))
            .map(rule_edit_snapshot)
            .collect();
        if expected != &json!(current) {
            return err_msg("分组规则已变化，请刷新后重新选择开启或停用");
        }
    }
    let mut n = 0usize;
    let mut changed = false;
    for r in &mut rules {
        if same_rule_group(&r.group, &group) {
            changed |= r.enabled != enabled;
            r.enabled = enabled;
            n += 1;
        }
    }
    if n == 0 {
        return err_msg("No rules in that group");
    }
    if !changed {
        return json!({"ok":true,"msg":"分组已处于所选状态"});
    }
    save_routing_rules(&mut map, rules);
    match save_conf(&map) {
        Ok(()) => {
            if is_running()
                && let Err(error) = run_sh("restart")
            {
                return json!({"ok":false,"saved":true,"applied":false,"error_code":"rule_apply_failed","msg":"规则已保存，但代理重新加载失败；已保存状态与当前运行状态可能不同，请查看诊断后重试应用。","detail":error.to_string()});
            }
            json!({
                "ok": true,
                "saved":true,
                "applied":is_running(),
                "msg": format!(
                    "{} {} rule(s) in group",
                    if enabled { "Enabled" } else { "Disabled" },
                    n
                )
            })
        }
        Err(e) => json!({"ok": false, "msg": format!("Save failed: {e}")}),
    }
}

fn act_replace_rule_group(post: &J) -> J {
    let old_group = post
        .get("group")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let new_group = post
        .get("new_group")
        .and_then(|v| v.as_str())
        .unwrap_or(old_group.as_str())
        .trim()
        .to_string();
    if new_group.is_empty() && !old_group.trim().is_empty() {
        return err_msg("Group name is required");
    }
    let rule_type = post
        .get("rule_type")
        .and_then(|v| v.as_str())
        .unwrap_or("domain-suffix")
        .trim();
    let target = post
        .get("target")
        .and_then(|v| v.as_str())
        .unwrap_or("proxy");
    if !proxy_groups::valid_target(target) {
        return err_msg("Invalid target action (direct / proxy / reject)");
    }
    let text = post.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let enabled = post
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let priority = match requested_rule_priority(post, 0) {
        Ok(n) => n,
        Err(e) => return err_msg(e),
    };

    let mut incoming = Vec::new();
    if text.trim().is_empty() && matches!(rule_type, "match" | "final") {
        match sockrocket_core::make_typed_rule("match", "*", target, &new_group) {
            Ok(mut rule) => {
                rule.enabled = enabled;
                rule.priority = priority;
                incoming.push(rule);
            }
            Err(e) => return err_msg(&e),
        }
    } else if text.trim().is_empty() {
        return err_msg("Paste the group's patterns first");
    } else {
        let looks_clash = text.lines().map(str::trim).any(|l| {
            let head = l.split([',', ' ', ':']).next().unwrap_or("");
            matches!(
                head.to_ascii_uppercase().as_str(),
                "SOURCE"
                    | "DOMAIN"
                    | "DOMAIN-SUFFIX"
                    | "DOMAIN-KEYWORD"
                    | "IP-CIDR"
                    | "GEOIP"
                    | "DST-PORT"
                    | "PORT"
                    | "MATCH"
                    | "FINAL"
            )
        });
        let parsed = if looks_clash {
            sockrocket_core::parse_batch_rules_in_group(text, target, &new_group)
        } else if sockrocket_core::rule_type_supports_bulk(rule_type) {
            sockrocket_core::parse_typed_patterns(text, rule_type, target, &new_group)
        } else {
            sockrocket_core::parse_batch_rules_in_group(text, target, &new_group)
        };
        if let Some(error) = parsed.errors.first() {
            return err_msg(&format!(
                "第 {} 行：{}；未保存任何更改",
                error.line, error.message
            ));
        }
        if parsed.rules.is_empty() {
            let msg = parsed
                .errors
                .first()
                .map(|e| format!("No valid rules (line {}: {})", e.line, e.message))
                .unwrap_or_else(|| "No valid patterns in the group".into());
            return err_msg(&msg);
        }
        for mut rule in parsed.rules {
            rule.group = new_group.clone();
            rule.enabled = enabled;
            rule.priority = priority;
            incoming.push(rule);
        }
    }
    if incoming.is_empty() {
        return err_msg("No valid patterns in the group");
    }

    let Ok(mut map) = load_conf() else {
        return err_msg("Config file not found");
    };
    let mut rules = load_routing_rules(&map);
    let members: Vec<_> = rules
        .iter()
        .filter(|r| same_rule_group(&r.group, &old_group))
        .collect();
    if members.is_empty() {
        return err_msg("分组已不存在，请刷新后重试");
    }
    if let Some(expected) = post.get("expected_rules") {
        let current: Vec<_> = members.iter().map(|r| rule_edit_snapshot(r)).collect();
        if expected != &json!(current) {
            return err_msg("分组规则已变化，请刷新后重新编辑");
        }
    }
    let common_source = if members.iter().all(|r| r.source == members[0].source) {
        members[0].source.as_str()
    } else {
        ""
    };
    let source = match requested_rule_source(post, common_source) {
        Ok(value) => value,
        Err(error) => return err_msg(&error),
    };
    let default_enabled = members.iter().any(|r| r.enabled);
    let default_priority = members[0].priority;
    for rule in &mut incoming {
        if post.get("source").is_some() || rule.source.is_empty() {
            rule.source.clone_from(&source);
        }
        let previous = members.iter().find(|r| {
            rule_kind(&r.rule_type) == rule_kind(&rule.rule_type)
                && r.pattern == rule.pattern
                && r.source == rule.source
        });
        if post.get("enabled").is_none() {
            rule.enabled = previous.map_or(default_enabled, |r| r.enabled);
        }
        if post.get("priority").is_none() {
            rule.priority = previous.map_or(default_priority, |r| r.priority);
        }
        if let Some(previous) = previous {
            rule.name.clone_from(&previous.name);
        }
    }
    let n = match sockrocket_core::replace_rule_group(&mut rules, &old_group, incoming) {
        Ok(n) => n,
        Err(e) => return err_msg(&e),
    };
    save_routing_rules(&mut map, rules);
    match save_conf(&map) {
        Ok(()) => {
            if is_running()
                && let Err(error) = run_sh("restart")
            {
                return err_msg(&error.to_string());
            }
            json!({"ok": true, "msg": format!("Saved group ({n} rule(s))")})
        }
        Err(e) => json!({"ok": false, "msg": format!("Save failed: {e}")}),
    }
}

fn act_delete_rule_group(post: &J) -> J {
    let group = post
        .get("group")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let Ok(mut map) = load_conf() else {
        return err_msg("Config file not found");
    };
    let mut rules = load_routing_rules(&map);
    let before_n = rules.len();
    rules.retain(|r| !same_rule_group(&r.group, &group));
    let removed = before_n - rules.len();
    if removed == 0 {
        return err_msg("No rules in that group");
    }
    save_routing_rules(&mut map, rules);
    match save_conf(&map) {
        Ok(()) => {
            if is_running()
                && let Err(error) = run_sh("restart")
            {
                return err_msg(&error.to_string());
            }
            json!({"ok": true, "msg": format!("Removed {removed} rule(s) from group")})
        }
        Err(e) => json!({"ok": false, "msg": format!("Save failed: {e}")}),
    }
}

fn act_check_update() -> J {
    let current = env!("CARGO_PKG_VERSION");
    let prefer = sockrocket_core::preferred_merlin_cli_asset().map(|s| s.to_string());
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return err_msg(&format!("Runtime error: {e}")),
    };
    let result =
        rt.block_on(async { sockrocket_core::check_for_update(current, prefer.as_deref()).await });
    match result {
        Ok(check) => {
            let available = check.availability == sockrocket_core::UpdateAvailability::Available;
            json!({
                "ok": true,
                "current": check.current,
                "latest": check.latest,
                "available": available,
                "html_url": check.html_url,
                "asset_url": check.asset_url,
                "asset_name": check.asset_name,
                "msg": if available {
                    format!("Update available: v{} → v{}", check.current, check.latest)
                } else {
                    format!("Up to date (v{})", check.current)
                }
            })
        }
        Err(e) => json!({"ok": false, "msg": format!("Update check failed: {e}")}),
    }
}

/// Download the matching musl CLI binary from GitHub Releases and replace the
/// installed binary (config preserved). Restarts the service when it was running.
fn act_apply_update() -> J {
    let current = env!("CARGO_PKG_VERSION");
    let Some(asset) = sockrocket_core::preferred_merlin_cli_asset() else {
        return err_msg("Unsupported architecture for auto-update");
    };
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return err_msg(&format!("Runtime error: {e}")),
    };
    let check = match rt
        .block_on(async { sockrocket_core::check_for_update(current, Some(asset)).await })
    {
        Ok(c) => c,
        Err(e) => return err_msg(&format!("Update check failed: {e}")),
    };
    if check.availability != sockrocket_core::UpdateAvailability::Available {
        return json!({"ok": true, "msg": format!("Already up to date (v{})", check.current)});
    }
    let Some(url) = check.asset_url.clone() else {
        return err_msg(&format!(
            "Latest v{} has no asset `{asset}` — open {}",
            check.latest, check.html_url
        ));
    };

    let dest = std::env::temp_dir().join(format!("sockrocket-cli-update-{}", check.latest));
    if let Err(e) = rt.block_on(async { sockrocket_core::download_file(&url, &dest).await }) {
        return err_msg(&format!("Download failed: {e}"));
    }

    let bin = sockrocket_dir().join("sockrocket-cli");
    // Stage on the destination filesystem before stopping anything. A full
    // JFFS must leave the old executable intact (also avoids ETXTBSY while
    // this API process is executing that file).
    let staged = bin.with_file_name(format!("sockrocket-cli.update-{}", rand::random::<u64>()));
    let prepare = (|| -> std::io::Result<()> {
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)?;
        let mut input = fs::File::open(&dest)?;
        std::io::copy(&mut input, &mut output)?;
        output.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))?;
        }
        Ok(())
    })();
    if let Err(error) = prepare {
        let _ = fs::remove_file(&staged);
        return err_msg(&format!(
            "Cannot stage update; installed program unchanged: {error}"
        ));
    }
    let was_running = is_running();
    if was_running && let Err(error) = run_sh("stop") {
        let _ = fs::remove_file(&staged);
        return err_msg(&error.to_string());
    }
    // Only rename touches the installed executable; failed staging and failed
    // rename both preserve it. The download in the temporary directory is
    // removed only after installation succeeds.
    if let Err(error) = fs::rename(&staged, &bin) {
        let _ = fs::remove_file(&staged);
        if was_running {
            let _ = run_sh("start");
        }
        return err_msg(&format!(
            "Failed to install update; previous binary retained: {error}"
        ));
    }
    let _ = fs::remove_file(&dest);
    // Record package version for the Web UI.
    let _ = fs::write(
        sockrocket_dir().join("version"),
        format!("{}\n", check.latest),
    );
    if was_running && let Err(error) = run_sh("start") {
        return err_msg(&error.to_string());
    }
    // Restart API after the HTTP response is flushed — killing ourselves
    // mid-write drops the WebUI reply.
    let api_sh = sockrocket_sh();
    let _ = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "sleep 2; sh '{}' api-restart >/dev/null 2>&1",
            api_sh.display()
        ))
        .spawn();
    json!({
        "ok": true,
        "msg": format!("Updated to v{} — service restarted (API reloads in ~2s)", check.latest),
        "latest": check.latest,
        "current": check.latest
    })
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

    async fn parse_test_request(bytes: Vec<u8>) -> Option<ApiRequest> {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let client = tokio::spawn(async move {
            let mut socket = tokio::net::TcpStream::connect(addr).await.unwrap();
            // Invalid headers may cause the server to close early.
            let _ = socket.write_all(&bytes).await;
            let _ = socket.shutdown().await;
        });
        let (mut socket, _) = listener.accept().await.unwrap();
        let result = read_request(&mut socket).await;
        client.await.unwrap();
        result
    }

    #[tokio::test]
    async fn api_rejects_ambiguous_or_oversized_requests() {
        for request in [
            "POST /api HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n{}",
            "POST /api HTTP/1.1\r\nContent-Length: invalid\r\n\r\n{}",
            "POST /api HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
            "GET /api HTTP/1.1 extra\r\n\r\n",
        ] {
            assert!(
                parse_test_request(request.as_bytes().to_vec())
                    .await
                    .is_none(),
                "{request}"
            );
        }
        let oversized = format!(
            "GET /api HTTP/1.1\r\nX-Padding: {}\r\n\r\n",
            "x".repeat(16384)
        );
        assert!(parse_test_request(oversized.into_bytes()).await.is_none());
        let valid = parse_test_request(
            b"POST /api?action=status HTTP/1.1\r\nContent-Length: 2\r\n\r\n{}".to_vec(),
        )
        .await
        .unwrap();
        assert_eq!(valid.action, "status");
        assert_eq!(valid.body, "{}");
    }

    #[tokio::test]
    async fn api_drops_excess_connections_and_accepts_after_slot_is_freed() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);
        let server = tokio::spawn(serve_api_authenticated(
            addr.port(),
            Arc::new("test-token".into()),
        ));
        let first = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(socket) = tokio::net::TcpStream::connect(addr).await {
                    break socket;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let mut clients = vec![first];
        for _ in 1..6 {
            clients.push(tokio::net::TcpStream::connect(addr).await.unwrap());
        }
        // All six connections with incomplete headers hold their slots.
        for client in &mut clients {
            assert!(
                tokio::time::timeout(Duration::from_millis(20), client.read(&mut [0u8; 1]),)
                    .await
                    .is_err()
            );
        }
        let mut excess = tokio::net::TcpStream::connect(addr).await.unwrap();
        let dropped = tokio::time::timeout(Duration::from_secs(1), excess.read(&mut [0u8; 1]))
            .await
            .expect("overload must not queue another socket/task");
        // Windows can report RST when an unhandled connection is dropped.
        // EOF and ConnectionReset both mean admission rejected it immediately.
        match dropped {
            Ok(0) => {}
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
            other => panic!("overloaded API must close without data: {other:?}"),
        }

        // A cheap OPTIONS request releases one slot without dispatch/config I/O.
        // Verify the complete HTTP response, not EOF: Windows can deliver a
        // reset after the framed response when the server closes the socket.
        // read_exact still fails if any response byte is missing or invalid.
        let expected = b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        clients[0]
            .write_all(b"OPTIONS /api HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        let mut reply = vec![0; expected.len()];
        tokio::time::timeout(Duration::from_secs(1), clients[0].read_exact(&mut reply))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reply, expected);
        let mut next = tokio::net::TcpStream::connect(addr).await.unwrap();
        next.write_all(b"OPTIONS /api HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        let mut reply = vec![0; expected.len()];
        tokio::time::timeout(Duration::from_secs(1), next.read_exact(&mut reply))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reply, expected);
        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn api_requires_token_rejects_browser_origins_and_get_mutations() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpStream,
        };
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let task = tokio::spawn(serve_api_authenticated(
            address.port(),
            Arc::new("secret".into()),
        ));
        for _ in 0..100 {
            if TcpStream::connect(address).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        for (request, status) in [
            ("GET /api?action=config HTTP/1.1\r\n\r\n", "401"),
            (
                "GET /api?action=version HTTP/1.1\r\nAuthorization: Bearer wrong\r\n\r\n",
                "401",
            ),
            (
                "GET /api?action=stop HTTP/1.1\r\nAuthorization: Bearer secret\r\n\r\n",
                "405",
            ),
            (
                "GET /api?action=version HTTP/1.1\r\nOrigin: https://evil.example\r\nAuthorization: Bearer secret\r\n\r\n",
                "403",
            ),
            (
                "GET /api?action=version HTTP/1.1\r\nHost: evil.example\r\nAuthorization: Bearer secret\r\n\r\n",
                "403",
            ),
            (
                "GET /api?action=version HTTP/1.1\r\nAuthorization: Bearer secret\r\n\r\n",
                "200",
            ),
        ] {
            let mut stream = TcpStream::connect(address).await.unwrap();
            stream.write_all(request.as_bytes()).await.unwrap();
            let mut reply = String::new();
            stream.read_to_string(&mut reply).await.unwrap();
            assert!(reply.starts_with(&format!("HTTP/1.1 {status}")), "{reply}");
            assert!(!reply.contains("Access-Control-Allow-Origin"));
        }
        task.abort();
        let _ = task.await;
    }

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
            fs::create_dir_all(dir.join("scripts")).unwrap();
            fs::write(dir.join("scripts/sockrocket.sh"), "#!/bin/sh\nexit 0\n").unwrap();
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

    #[test]
    fn structured_network_policy_preserves_stop_and_private_configuration() {
        let env = TestEnv::new(SAMPLE);
        fs::write(env.dir.join("stopped"), "").unwrap();
        fs::write(env.dir.join("scripts/sockrocket.sh"), "#!/bin/sh\nexit 7\n").unwrap();
        let before = load_conf().unwrap();
        let settings = network_settings::get();
        assert_eq!(settings["ok"], true, "{settings}");
        let body =
            json!({"dns":settings["dns"],"ipv6_mode":"dual_stack","revision":settings["revision"]});
        let saved = dispatch("save_network_settings", &body);
        assert_eq!(saved["ok"], true, "{saved}");
        assert_eq!(saved["applied"], false);
        assert!(env.dir.join("stopped").exists());
        let after = load_conf().unwrap();
        for field in ["nodes", "subscriptions", "dns_direct_server", "mode"] {
            assert_eq!(before.get(ykey(field)), after.get(ykey(field)));
        }
        assert!(after.get(ykey("dns")).is_some());
        let bytes = fs::read(env.dir.join("config.yaml")).unwrap();
        assert_eq!(
            dispatch("save_network_settings", &body)["error_code"],
            "network_config_stale"
        );
        assert_eq!(fs::read(env.dir.join("config.yaml")).unwrap(), bytes);
        let mut bad = body;
        bad["dns"]["direct_servers"] =
            json!(["https://secret:password@resolver.example.test/dns-query?private=token"]);
        let reply = dispatch("save_network_settings", &bad);
        assert_eq!(reply["error_code"], "dns_config_invalid");
        let reply = reply.to_string();
        assert!(!reply.contains("password") && !reply.contains("private=token"));
        assert_eq!(fs::read(env.dir.join("config.yaml")).unwrap(), bytes);
    }

    #[test]
    fn proxy_group_api_preserves_privacy_references_and_device_inheritance() {
        let env = TestEnv::new(SAMPLE);
        fs::write(env.dir.join("stopped"), "manual").unwrap();
        let state = dispatch("proxy_groups", &json!({}));
        assert_eq!(state["ok"], true);
        let rendered = state.to_string();
        assert!(!rendered.contains("a.example.com") && !rendered.contains("00000000-0000"));
        let fp = state["nodes"][0]["fingerprint"].clone();
        assert_eq!(fp.as_str().unwrap().len(), 64);
        let body = json!({"revision":state["revision"],"group":{"name":"Video","type":"select","members":[fp],"current":fp}});
        assert_eq!(dispatch("save_proxy_group", &body)["ok"], true);
        assert_eq!(
            dispatch("save_proxy_group", &body)["ok"],
            false,
            "stale editor must not overwrite"
        );
        let rules = dispatch(
            "add_rule",
            &json!({"rule_type":"domain-suffix","pattern":"example.invalid","target":"group:Video"}),
        );
        assert_eq!(rules["ok"], true, "{rules}");
        let trace = dispatch(
            "trace_route",
            &json!({"destination":"example.invalid","resolve_dns":false}),
        );
        assert_eq!(trace["trace"]["action"], "group:Video");
        assert_eq!(trace["node_name"], "Node A");
        let fresh = dispatch("proxy_groups", &json!({}));
        assert_eq!(
            dispatch(
                "delete_proxy_group",
                &json!({"revision":fresh["revision"],"name":"Video"})
            )["ok"],
            false
        );
        let renamed = dispatch(
            "save_proxy_group",
            &json!({"revision":fresh["revision"],"original_name":"Video","group":{"name":"Media","type":"select","members":[fp],"current":fp}}),
        );
        assert_eq!(renamed["ok"], true, "{renamed}");
        assert_eq!(
            load_routing_rules(&load_conf().unwrap())[0].target,
            "group:Media"
        );
        let revision = route_trace::rules_revision(&load_conf().unwrap());
        let saved = dispatch(
            "save_device_policy",
            &json!({"revision":revision,"source":"192.0.2.9","target":"group:Media"}),
        );
        assert_eq!(saved["ok"], true, "{saved}");
        let trace = dispatch(
            "trace_route",
            &json!({"destination":"unlisted.example","source":"192.0.2.9","resolve_dns":false}),
        );
        assert_eq!(trace["trace"]["action"], "group:Media");
        let shared_trace = dispatch(
            "trace_route",
            &json!({"destination":"baidu.com","source":"192.0.2.9","resolve_dns":false}),
        );
        assert_eq!(shared_trace["trace"]["action"], "direct");
        let default_trace = dispatch(
            "trace_route",
            &json!({"destination":"baidu.com","source":"192.0.2.10","resolve_dns":false}),
        );
        assert_eq!(default_trace["trace"]["action"], "direct");
        let revision = route_trace::rules_revision(&load_conf().unwrap());
        assert_eq!(
            dispatch(
                "save_device_policy",
                &json!({"revision":revision,"source":"192.0.2.9","target":"inherit"})
            )["ok"],
            true
        );
        assert_eq!(
            load_routing_rules(&load_conf().unwrap()).len(),
            1,
            "inherit removes only the matching source final"
        );
        assert!(env.dir.join("stopped").exists());
        let fresh = dispatch("proxy_groups", &json!({}));
        assert!(!fresh.to_string().contains("a.example.com"));
        let before = fs::read(env.dir.join("config.yaml")).unwrap();
        assert_eq!(
            dispatch(
                "batch_add_rules",
                &json!({"text":"DOMAIN,example.net,group:Missing","target":"proxy"})
            )["ok"],
            false
        );
        assert_eq!(fs::read(env.dir.join("config.yaml")).unwrap(), before);
    }

    #[test]
    fn stop_does_not_wait_for_config_transaction() {
        let env = TestEnv::new(SAMPLE);
        let _lock = storage::MutationGuard::acquire().unwrap();
        let start = std::time::Instant::now();
        let response = dispatch("stop", &json!({}));
        assert_eq!(response["ok"], true, "{response}");
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(env.dir.join("config.yaml").exists());
    }

    #[test]
    fn duplicate_subscription_and_invalid_input_leave_config_unchanged() {
        let env = TestEnv::new(SAMPLE);
        let before = fs::read(env.dir.join("config.yaml")).unwrap();
        let duplicate = act_add_sub(&json!({"name":"duplicate", "url":"https://example.com/sub"}));
        assert_eq!(duplicate["error_code"], "subscription_exists");
        assert_eq!(duplicate["stage"], "validate_subscription");
        for body in [
            json!({"name":"duplicate", "url":"https://example.com/sub"}),
            json!({"name":"bad", "url":"file:///private/config", "format":"auto"}),
            json!({"name":"bad", "url":"https://example.com/sub2", "format":"invalid"}),
            json!({"name":"   ", "url":"https://"}),
        ] {
            assert_eq!(act_add_sub(&body)["ok"], false);
            assert_eq!(fs::read(env.dir.join("config.yaml")).unwrap(), before);
        }
        assert_eq!(
            act_add_node(&json!({"name":"example", "server":"example.com", "port":65537}))["ok"],
            false
        );
        assert_eq!(fs::read(env.dir.join("config.yaml")).unwrap(), before);
    }

    #[test]
    fn restart_after_edit_preserves_manual_stop() {
        let env = TestEnv::new(SAMPLE);
        fs::write(env.dir.join("stopped"), "").unwrap();
        fs::write(env.dir.join("scripts/sockrocket.sh"), "#!/bin/sh\nexit 7\n").unwrap();
        run_sh("restart").unwrap();
        assert!(run_sh("start").is_err());
        assert!(env.dir.join("stopped").exists());
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
    fn a_rule_group_can_pin_a_different_node() {
        let _env = TestEnv::new(SAMPLE);
        assert_eq!(
            dispatch(
                "add_rule",
                &json!({"rule_type":"domain-suffix","pattern":"video.example","target":"proxy","group":"Video"})
            )["ok"],
            true
        );
        assert_eq!(
            dispatch(
                "add_rule",
                &json!({"rule_type":"domain-suffix","pattern":"ads.example","target":"reject","group":"Video"})
            )["ok"],
            true
        );
        let listed = dispatch("get_rules", &json!({}));
        let id = listed["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["name"] == "Node B")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let target = format!("node:{id}");
        let result = dispatch(
            "set_rule_group_target",
            &json!({"group":"Video","target":target}),
        );
        assert_eq!(result["ok"], true, "{result}");
        let rules = load_routing_rules(&load_conf().unwrap());
        let video: Vec<_> = rules.iter().filter(|rule| rule.group == "Video").collect();
        assert!(
            video
                .iter()
                .any(|rule| rule.pattern == "video.example" && rule.target == target)
        );
        assert!(
            video
                .iter()
                .any(|rule| rule.pattern == "ads.example" && rule.target == "reject")
        );
        assert_eq!(
            dispatch(
                "set_rule_group_target",
                &json!({"group":"Video","target":"proxy","expected_rules":[]})
            )["ok"],
            false
        );
    }

    #[test]
    fn route_trace_is_read_only_normalized_and_revision_guarded() {
        let env = TestEnv::new(SAMPLE);
        let before = fs::read(env.dir.join("config.yaml")).unwrap();
        let revision = dispatch("get_rules", &json!({}))["revision"].clone();
        let result = dispatch(
            "trace_route",
            &json!({"destination":"https://WWW.BAIDU.COM/private?secret=token","destination_ip":"203.0.113.1","resolve_dns":false,"expected_revision":revision}),
        );
        assert_eq!(result["ok"], true);
        assert_eq!(result["destination"], "www.baidu.com");
        assert_eq!(result["port"], 443);
        assert_eq!(result["trace"]["action"], "direct");
        assert_eq!(result["trace"]["definitive"], true);
        assert!(!result.to_string().contains("secret"));
        assert!(!result.to_string().contains("a.example.com"));
        assert_eq!(
            dispatch(
                "trace_route",
                &json!({"destination":"example.com","expected_revision":"old"})
            )["error_code"],
            "route_trace_stale"
        );
        for request in [
            json!({"destination":"https://user:password@example.com"}),
            json!({"destination":"example.com","source":"192.0.2.0/24"}),
            json!({"destination":"example.com","port":0}),
            json!({"destination":"example.com","resolve_dns":"false"}),
        ] {
            assert_eq!(
                dispatch("trace_route", &request)["error_code"],
                "route_trace_invalid"
            );
        }
        assert_eq!(
            dispatch("trace_route", &json!({"destination":"198.18.0.1"}))["error_code"],
            "route_trace_fake_ip"
        );
        assert_eq!(fs::read(env.dir.join("config.yaml")).unwrap(), before);
        assert!(read_only_action("trace_route"));
        fs::write(
            env.dir.join("config.yaml"),
            SAMPLE.replace("rules: []", "rules: [invalid]"),
        )
        .unwrap();
        assert_eq!(
            dispatch(
                "trace_route",
                &json!({"destination":"example.com","resolve_dns":false})
            )["error_code"],
            "route_trace_config",
            "malformed rules cannot silently shift the winning rule's saved index"
        );
    }

    #[test]
    fn route_trace_defaults_to_first_node_like_daemon_and_status() {
        let _env = TestEnv::new(&SAMPLE.replace("active_node: 0\n", ""));
        let request =
            json!({"destination":"example.com","destination_ip":"203.0.113.1","resolve_dns":false});
        let result = dispatch("trace_route", &request);
        assert_eq!(result["ok"], true);
        assert_eq!(result["active_node"], 0);
        assert_eq!(result["active_node"], act_status()["active_node"]);
        assert_eq!(result["node_name"], "Node A");
        let mut map = load_conf().unwrap();
        set_active_node(&mut map, 1);
        save_conf(&map).unwrap();
        let result = dispatch("trace_route", &request);
        assert_eq!(result["active_node"], 1);
        assert_eq!(result["node_name"], "Node B");
        map.insert(ykey("nodes"), Y::Sequence(vec![]));
        save_conf(&map).unwrap();
        let result = dispatch("trace_route", &request);
        assert!(result["active_node"].is_null());
        assert!(result["node_name"].is_null());
    }

    #[test]
    fn route_trace_handles_mode_and_exact_saved_rule_index() {
        let _env=TestEnv::new(&SAMPLE.replace("rules: []","rules:\n  - {rule_type: domain, pattern: example.com, target: reject, priority: 999}\n  - {rule_type: final, pattern: '*', target: direct, source: 192.0.2.8, priority: 0}"));
        let result = dispatch(
            "trace_route",
            &json!({"destination":"http://unlisted.example/path","source":"192.0.2.8","resolve_dns":false}),
        );
        assert_eq!(result["ok"], true);
        assert_eq!(result["port"], 80);
        assert_eq!(result["trace"]["matched_rule"], 1);
        assert_eq!(result["trace"]["matched_snapshot"]["source"], "192.0.2.8");
        assert_eq!(result["trace"]["action"], "direct");
        let mut map = load_conf().unwrap();
        map.insert(ykey("proxy_mode"), Y::String("global".into()));
        save_conf(&map).unwrap();
        let result = dispatch(
            "trace_route",
            &json!({"destination":"example.com","resolve_dns":false}),
        );
        assert_eq!(result["trace"]["action"], "proxy");
        assert_eq!(result["trace"]["steps"], json!([]));
        assert_eq!(result["trace"]["dns"], "not_needed");
    }

    #[test]
    fn explicit_start_restores_capture_but_background_restart_preserves_choices() {
        let env = TestEnv::new(&SAMPLE.replace(
            "mode: \"tun\"",
            "mode: \"socks\"\ntransparent_proxy: false\ndns_hijack: false",
        ));
        // Fail quickly after recording the controller invocation. The test
        // exercises the API transaction without opening a real network path.
        fs::write(
            sockrocket_sh(),
            "#!/bin/sh\nprintf '%s' \"$1\" > \"$SOCKROCKET_DIR/action\"\nexit 7\n",
        )
        .unwrap();
        let before = load_conf().unwrap();
        for action in ["start", "restart"] {
            save_conf(&before).unwrap();
            assert_eq!(dispatch(action, &json!({}))["ok"], false);
            let map = load_conf().unwrap();
            assert!(conf_bool(&map, "transparent_proxy", false));
            assert!(conf_bool(&map, "dns_hijack", false));
            for key in ["nodes", "subscriptions", "rules", "dns_direct_domains"] {
                assert_eq!(map.get(ykey(key)), before.get(ykey(key)));
            }
            assert_eq!(fs::read_to_string(env.dir.join("action")).unwrap(), "start");
        }
        save_conf(&before).unwrap();
        assert!(run_sh("restart").is_err());
        assert_eq!(load_conf().unwrap(), before);
        // Stop remains independent of the configuration lock and preferences.
        let lock = storage::MutationGuard::acquire().unwrap();
        let _ = dispatch("stop", &json!({}));
        assert_eq!(load_conf().unwrap(), before);
        drop(lock);
    }

    #[test]
    fn lifecycle_failure_cannot_report_success() {
        let _env = TestEnv::new(SAMPLE);
        fs::write(sockrocket_sh(), "#!/bin/sh\nexit 7\n").unwrap();
        for action in ["start", "stop", "restart"] {
            let reply = dispatch(action, &J::Null);
            assert_eq!(reply["ok"], false, "{reply}");
            assert!(reply["msg"].as_str().unwrap().contains("failed"), "{reply}");
        }
    }

    #[test]
    fn concurrent_config_mutations_preserve_every_change() {
        let _env = TestEnv::new(SAMPLE);
        std::thread::scope(|scope| {
            let mut tasks = Vec::new();
            for index in 0..12 {
                tasks.push(scope.spawn(move || {
                    dispatch(
                        "add_direct_domain",
                        &json!({"domain": format!("n{index}.example.com")}),
                    )
                }));
            }
            for task in tasks {
                let reply = task.join().unwrap();
                assert_eq!(reply["ok"], true, "{reply}");
            }
        });
        let map = load_conf().unwrap();
        let domains = direct_domains(&map);
        for index in 0..12 {
            assert!(domains.contains(&format!("n{index}.example.com")));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(conf_path()).unwrap().permissions().mode() & 0o077,
                0
            );
        }
    }

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
        // Unreachable URL: entry is still saved; sync fetch reports failure.
        let r = act_add_sub(
            &json!({"name": "Sub 2", "url": "https://127.0.0.1:1/s", "format": "base64"}),
        );
        assert!(r.get("msg").and_then(|v| v.as_str()).is_some());
        let subs = act_subscriptions()["subs"].as_array().unwrap().clone();
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[1]["format"], J::from("base64"));
        assert_eq!(subs[1]["name"], J::from("Sub 2"));
        let r = act_del_sub(&json!({"index": 0}));
        assert_eq!(r["ok"], J::Bool(true));
        let subs = act_subscriptions()["subs"].as_array().unwrap().clone();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0]["name"], J::from("Sub 2"));
    }

    #[test]
    fn deleting_subscription_drops_its_nodes_and_keeps_the_url_list() {
        let env = TestEnv::new(SAMPLE);
        let path = env.dir.join("config.yaml");
        let mut text = fs::read_to_string(&path).unwrap();
        text = text.replace(
            "    server: \"a.example.com\"\n",
            "    server: \"a.example.com\"\n    extra:\n      sub_url: \"https://example.com/sub\"\n",
        );
        fs::write(&path, text).unwrap();
        let r = act_del_sub(&json!({"index": 0}));
        assert_eq!(r["ok"], J::Bool(true), "{r}");
        let cfg = load_app_config().unwrap();
        assert!(cfg.subscriptions.is_empty());
        assert_eq!(cfg.nodes.len(), 1);
        assert_eq!(cfg.nodes[0].name, "Node B");
        let saved = fs::read_to_string(&path).unwrap();
        assert!(saved.contains("dns_port:"));
        assert!(saved.contains("5300"));
    }

    #[test]
    fn subscription_rename() {
        let _env = TestEnv::new(SAMPLE);
        let r = act_rename_sub(&json!({"index": 0, "name": "  Home  "}));
        assert_eq!(r["ok"], J::Bool(true), "{r}");
        assert_eq!(r["name"], J::from("Home"));
        let subs = act_subscriptions()["subs"].as_array().unwrap().clone();
        assert_eq!(subs[0]["name"], J::from("Home"));
        // URL unchanged
        assert!(!subs[0]["url"].as_str().unwrap_or("").is_empty());
        let r = act_rename_sub(&json!({"index": 0, "name": "   "}));
        assert_eq!(r["ok"], J::Bool(false));
        let r = act_rename_sub(&json!({"index": 99, "name": "X"}));
        assert_eq!(r["ok"], J::Bool(false));
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
            (
                "anytls",
                json!({"password":"synthetic-private-password", "sni":"tls.example.invalid"}),
            ),
            (
                "wireguard",
                json!({"wireguard":{
                    "private_key":"CwsLCwsLCwsLCwsLCwsLCwsLCwsLCwsLCwsLCwsLCws=",
                    "public_key":"FhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhY=",
                    "local_addresses":["192.0.2.2/32"], "dns":["1.1.1.1"], "mtu":1280
                }}),
            ),
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
        assert_eq!(d["nodes"].as_array().unwrap().len(), 10);
        assert!(!d.to_string().contains("synthetic-private-password"));
        assert!(!d.to_string().contains("private_key"));
        let config: sockrocket_core::AppConfig = serde_yaml::from_str(&text).unwrap();
        for node in config.nodes.iter().skip(8) {
            assert!(sockrocket_core::create_outbound_unwarmed(node).is_ok());
        }
        let before = fs::read_to_string(conf_path()).unwrap();
        assert_eq!(
            act_add_node(
                &json!({"name":"invalid","server":"vpn.example.invalid","port":443,"protocol":"anytls","password":""})
            )["ok"],
            false
        );
        assert_eq!(fs::read_to_string(conf_path()).unwrap(), before);
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
    fn device_rule_crud_batch_and_group_edit_preserve_sources() {
        let _env = TestEnv::new(SAMPLE);
        for (source, target) in [
            ("192.0.2.10", "proxy"),
            ("192.0.2.20", "direct"),
            ("", "reject"),
        ] {
            let result = act_add_rule(
                &json!({"source":source,"group":"Devices","rule_type":"domain-suffix","pattern":"video.invalid","target":target}),
            );
            assert_eq!(result["ok"], true, "{result}");
        }
        let saved = fs::read_to_string(conf_path()).unwrap();
        for bad in [json!("192.0.2.1/33"), json!("phone.local"), json!(123)] {
            assert_eq!(
                act_add_rule(
                    &json!({"source":bad,"rule_type":"final","pattern":"*","target":"direct"})
                )["ok"],
                false
            );
            assert_eq!(fs::read_to_string(conf_path()).unwrap(), saved);
        }
        let exported = act_export_rules()["text"].as_str().unwrap().to_string();
        assert!(exported.contains("SOURCE,192.0.2.10/32,"));
        let result = act_replace_rule_group(
            &json!({"group":"Devices","text":exported,"rule_type":"domain-suffix","target":"proxy"}),
        );
        assert_eq!(result["ok"], true, "{result}");
        assert_eq!(act_get_rules()["rules"].as_array().unwrap().len(), 3);
        let mut expected = act_get_rules()["rules"][0].clone();
        expected.as_object_mut().unwrap().remove("index");
        let result = act_update_rule(
            &json!({"index":0,"expected":expected,"rule_type":"domain-suffix","pattern":"video.invalid","target":"direct","source":"2001:db8:1::1234/64"}),
        );
        assert_eq!(result["ok"], true, "{result}");
        assert_eq!(act_get_rules()["rules"][0]["source"], "2001:db8:1::/64");
        let before = fs::read_to_string(conf_path()).unwrap();
        let result = act_batch_add_rules(
            &json!({"source":"192.0.2.10","text":"SOURCE,192.0.2.20/32,DOMAIN-SUFFIX,other.invalid,PROXY"}),
        );
        assert_eq!(result["ok"], false);
        assert_eq!(fs::read_to_string(conf_path()).unwrap(), before);
        assert_eq!(
            act_batch_add_rules(
                &json!({"text":"SOURCE,192.0.2.30,DOMAIN-SUFFIX,video.invalid,REJECT"})
            )["ok"],
            true
        );
        let cfg =
            sockrocket_core::AppConfig::from_yaml_str(&fs::read_to_string(conf_path()).unwrap())
                .unwrap();
        assert_eq!(cfg.rules.len(), 4);
        assert!(cfg.rules.iter().any(|r| r.source == "192.0.2.30/32"));
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
    fn rule_edit_preserves_position_and_rejects_stale_snapshot() {
        let _env = TestEnv::new(SAMPLE);
        assert_eq!(
            act_add_rule(
                &json!({"rule_type":"domain-suffix","pattern":"example.invalid","target":"proxy","group":"Demo","priority":7})
            )["ok"],
            true
        );
        let mut expected = act_get_rules()["rules"][0].clone();
        expected.as_object_mut().unwrap().remove("index");
        let request = json!({"index":0,"expected":expected,"rule_type":"domain-suffix","pattern":"media.example.invalid","target":"direct","group":"Demo","enabled":false});
        assert_eq!(dispatch_inner("update_rule", &request)["ok"], true);
        let row = &act_get_rules()["rules"][0];
        assert_eq!(row["pattern"], "media.example.invalid");
        assert_eq!(row["priority"], 7);
        assert_eq!(row["enabled"], false);
        let saved = fs::read_to_string(conf_path()).unwrap();
        assert_eq!(
            act_update_rule(&request)["ok"],
            false,
            "stale editor must not overwrite"
        );
        assert_eq!(fs::read_to_string(conf_path()).unwrap(), saved);
        let mut fresh = row.clone();
        fresh.as_object_mut().unwrap().remove("index");
        let mut bad = request;
        bad["expected"] = fresh;
        bad["pattern"] = json!("not a domain");
        assert_eq!(act_update_rule(&bad)["ok"], false);
        assert_eq!(fs::read_to_string(conf_path()).unwrap(), saved);
    }

    #[test]
    fn stale_move_and_delete_cannot_mutate_a_different_rule() {
        let _env = TestEnv::new(SAMPLE);
        assert_eq!(
            act_add_rule(
                &json!({"rule_type":"domain-suffix","pattern":"example.invalid","target":"proxy"})
            )["ok"],
            true
        );
        let saved = fs::read_to_string(conf_path()).unwrap();
        for action in ["move_rule", "del_rule"] {
            assert_eq!(
                dispatch_inner(action, &json!({"index":0,"dir":1,"expected":{}}))["ok"],
                false
            );
            assert_eq!(fs::read_to_string(conf_path()).unwrap(), saved);
        }
    }

    #[test]
    fn explicit_rule_priority_is_validated_and_persisted() {
        let _env = TestEnv::new(SAMPLE);
        assert_eq!(
            act_add_rule(
                &json!({"rule_type":"domain-suffix","pattern":"example.invalid","target":"proxy","priority":10})
            )["ok"],
            true
        );
        let mut expected = act_get_rules()["rules"][0].clone();
        expected.as_object_mut().unwrap().remove("index");
        let mut request = json!({"index":0,"expected":expected,"rule_type":"domain-suffix","pattern":"example.invalid","target":"proxy","priority":42});
        let before = fs::read_to_string(conf_path()).unwrap();
        for bad in [
            json!(2147483648_i64),
            json!(-2147483649_i64),
            json!(1.5),
            json!("42"),
        ] {
            request["priority"] = bad;
            assert_eq!(act_update_rule(&request)["ok"], false);
            assert_eq!(fs::read_to_string(conf_path()).unwrap(), before);
        }
        request["priority"] = json!(42);
        assert_eq!(act_update_rule(&request)["ok"], true);
        assert_eq!(act_get_rules()["rules"][0]["priority"], 42);
        assert_eq!(
            act_batch_add_rules(
                &json!({"text":"DOMAIN-SUFFIX,batch.invalid,DIRECT","priority":-12})
            )["ok"],
            true
        );
        assert!(
            act_get_rules()["rules"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["pattern"] == "batch.invalid" && r["priority"] == -12)
        );
        let before = fs::read_to_string(conf_path()).unwrap();
        assert_eq!(
            act_add_rule(
                &json!({"rule_type":"domain-suffix","pattern":"bad.invalid","target":"proxy","priority":2147483648_i64})
            )["ok"],
            false
        );
        assert_eq!(fs::read_to_string(conf_path()).unwrap(), before);
    }

    #[test]
    fn rule_snapshot_matches_status_and_does_not_hide_corrupt_config() {
        let env = TestEnv::new(SAMPLE);
        let snapshot = act_get_rules();
        assert_eq!(snapshot["revision"], act_status()["rules_revision"]);
        assert_eq!(snapshot["proxy_mode"], "rule");
        let mut map = load_conf().unwrap();
        map.insert(ykey("proxy_mode"), Y::String("direct".into()));
        save_conf(&map).unwrap();
        let latest = act_get_rules();
        assert_eq!(latest["proxy_mode"], "direct");
        assert_ne!(latest["revision"], snapshot["revision"]);
        fs::write(env.dir.join("config.yaml"), "rules: [invalid]").unwrap();
        assert_eq!(act_get_rules()["ok"], false);
        fs::write(env.dir.join("config.yaml"), "rules: invalid").unwrap();
        assert_eq!(act_get_rules()["ok"], false);
        fs::write(env.dir.join("config.yaml"), "rules: [").unwrap();
        assert_eq!(act_get_rules()["ok"], false);
    }

    #[test]
    fn rule_presets_are_opt_in_idempotent_and_preserve_custom_rules() {
        let env=TestEnv::new(&SAMPLE.replace("rules: []","rules:\n  - {rule_type: domain-suffix, pattern: '.OPENAI.COM', target: reject, priority: 99, enabled: false, group: Custom}\n  - {rule_type: domain-suffix, pattern: chatgpt.com, target: direct, source: 192.0.2.8, priority: 10}"));
        fs::write(env.dir.join("stopped"), "").unwrap();
        let before = fs::read(conf_path()).unwrap();
        let catalog = dispatch("get_rules", &json!({}));
        assert_eq!(catalog["presets"].as_array().unwrap().len(), 7);
        assert_eq!(
            fs::read(conf_path()).unwrap(),
            before,
            "viewing defaults must not save or activate them"
        );
        let custom = load_routing_rules(&load_conf().unwrap());
        let revision = catalog["revision"].clone();
        let result = dispatch(
            "add_rule_preset",
            &json!({"preset":"ai","expected_revision":revision}),
        );
        assert_eq!(result["ok"], true);
        assert_eq!(result["added"], 8);
        assert_eq!(result["skipped"], 1);
        let saved = load_conf().unwrap();
        let rules = load_routing_rules(&saved);
        assert_eq!(
            rule_edit_snapshot(&rules[0]),
            rule_edit_snapshot(&custom[0])
        );
        assert_eq!(
            rule_edit_snapshot(&rules[1]),
            rule_edit_snapshot(&custom[1])
        );
        assert!(
            rules[2..]
                .iter()
                .all(|r| !r.enabled && r.priority == -10 && r.source.is_empty())
        );
        assert_eq!(node_count(&saved), 2);
        assert_eq!(conf_seq(&saved, "subscriptions").len(), 1);
        assert!(env.dir.join("stopped").exists());
        assert!(!read_only_action("add_rule_preset"));
        let before = fs::read(conf_path()).unwrap();
        for invalid in [J::Null, json!("false")] {
            assert_eq!(
                dispatch(
                    "set_rule_group_enabled",
                    &json!({"group":result["group"],"enabled":invalid})
                )["ok"],
                false
            );
            assert_eq!(fs::read(conf_path()).unwrap(), before);
        }
        assert_eq!(
            dispatch(
                "add_rule_preset",
                &json!({"preset":"ai","expected_revision":revision})
            )["ok"],
            false,
            "stale catalog is rejected"
        );
        assert_eq!(
            dispatch(
                "add_rule_preset",
                &json!({"preset":"ai","enabled":true,"expected_revision":route_trace::rules_revision(&saved)})
            )["ok"],
            false
        );
        let duplicate = dispatch(
            "add_rule_preset",
            &json!({"preset":"ai","expected_revision":route_trace::rules_revision(&saved)}),
        );
        assert_eq!(duplicate["added"], 0);
        assert_eq!(fs::read(conf_path()).unwrap(), before);
        let group = result["group"].as_str().unwrap();
        let snapshots: Vec<_> = rules
            .iter()
            .filter(|r| r.group == group)
            .map(rule_edit_snapshot)
            .collect();
        assert_eq!(
            dispatch(
                "set_rule_group_enabled",
                &json!({"group":group,"enabled":true,"expected_rules":[]})
            )["ok"],
            false
        );
        assert_eq!(fs::read(conf_path()).unwrap(), before);
        assert_eq!(
            dispatch(
                "set_rule_group_enabled",
                &json!({"group":group,"enabled":true,"expected_rules":snapshots})
            )["ok"],
            true
        );
        let rules = load_routing_rules(&load_conf().unwrap());
        assert!(!rules[0].enabled);
        assert!(rules[2..].iter().all(|r| r.enabled));
        assert!(
            env.dir.join("stopped").exists(),
            "enabling rules does not start a manually stopped proxy"
        );
    }

    #[test]
    fn all_rule_presets_persist_disabled_and_respect_core_routing() {
        use sockrocket_core::router::{RouteAction, Router, rule_mode_ruleset};
        let _env = TestEnv::new(SAMPLE);
        let catalog = rule_presets::catalog();
        let mut count = 0;
        for p in catalog.as_array().unwrap() {
            assert_eq!(p["enabled"], false);
            count += p["domains"].as_array().unwrap().len();
            let result = dispatch(
                "add_rule_preset",
                &json!({"preset":p["id"],"expected_revision":route_trace::rules_revision(&load_conf().unwrap())}),
            );
            assert_eq!(result["ok"], true, "{result}");
        }
        let rules = load_routing_rules(&load_conf().unwrap());
        assert_eq!(rules.len(), count);
        assert!(rules.iter().all(|r| !r.enabled));
        let router = Router::new(rule_mode_ruleset(&rules));
        assert_eq!(router.route("www.apple.com", 443), RouteAction::Proxy);
        assert_eq!(router.route("www.baidu.com", 443), RouteAction::Direct);
        assert_eq!(
            dispatch(
                "set_rule_group_enabled",
                &json!({"group":"预设 · Apple 直连","enabled":true})
            )["ok"],
            true
        );
        let rules = load_routing_rules(&load_conf().unwrap());
        assert_eq!(
            Router::new(rule_mode_ruleset(&rules)).route("www.apple.com", 443),
            RouteAction::Direct
        );
        let bytes = fs::read(conf_path()).unwrap();
        assert_eq!(
            dispatch(
                "set_rule_group_enabled",
                &json!({"group":"预设 · Apple 直连","enabled":true})
            )["ok"],
            true
        );
        assert_eq!(
            fs::read(conf_path()).unwrap(),
            bytes,
            "unchanged toggle must not rewrite config"
        );
        assert_eq!(
            dispatch(
                "set_rule_group_enabled",
                &json!({"group":"预设 · Apple 直连","enabled":false})
            )["ok"],
            true
        );
        assert_eq!(
            Router::new(rule_mode_ruleset(&load_routing_rules(
                &load_conf().unwrap()
            )))
            .route("www.apple.com", 443),
            RouteAction::Proxy
        );
    }

    #[test]
    fn batch_rule_changes_are_atomic_and_keep_disabled_metadata() {
        let _env = TestEnv::new(SAMPLE);
        assert_eq!(
            act_add_rule(
                &json!({"rule_type":"domain-suffix","pattern":"example.invalid","target":"proxy","group":"Demo","priority":7,"name":"Demo rule"})
            )["ok"],
            true
        );
        assert_eq!(
            act_set_rule_group_enabled(&json!({"group":"Demo","enabled":false}))["ok"],
            true
        );
        let saved = fs::read_to_string(conf_path()).unwrap();
        let invalid = "DOMAIN-SUFFIX,media.example.invalid,PROXY\nINVALID-RULE,bad,PROXY";
        for response in [
            act_batch_add_rules(&json!({"group":"Demo","text":invalid})),
            act_replace_rule_group(&json!({"group":"Demo","text":invalid})),
        ] {
            assert_eq!(response["ok"], false, "{response}");
            assert!(response["msg"].as_str().unwrap().contains("2"));
            assert_eq!(fs::read_to_string(conf_path()).unwrap(), saved);
        }
        assert_eq!(
            act_replace_rule_group(
                &json!({"group":"Demo","text":"DOMAIN-SUFFIX,example.invalid,DIRECT\nDOMAIN-SUFFIX,media.example.invalid,PROXY"})
            )["ok"],
            true
        );
        let result = act_get_rules();
        let rows = result["rules"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter()
                .all(|r| r["enabled"] == false && r["priority"] == 7)
        );
        assert_eq!(rows[0]["name"], "Demo rule");
        let saved = fs::read_to_string(conf_path()).unwrap();
        assert_eq!(
            act_replace_rule_group(
                &json!({"group":"Demo","expected_rules":[],"text":"DOMAIN-SUFFIX,other.invalid,PROXY"})
            )["ok"],
            false
        );
        assert_eq!(fs::read_to_string(conf_path()).unwrap(), saved);
    }

    #[test]
    fn rule_move_follows_effective_priority_and_keeps_final_phase() {
        let _env = TestEnv::new(SAMPLE);
        let mut map = load_conf().unwrap();
        map.insert(ykey("rules"), serde_yaml::from_str::<Y>("- {rule_type: domain-suffix, pattern: low.invalid, target: proxy, priority: 1}\n- {rule_type: final, pattern: '*', target: proxy, priority: 99}\n- {rule_type: domain-suffix, pattern: high.invalid, target: direct, priority: 10}").unwrap());
        save_conf(&map).unwrap();
        assert_eq!(act_move_rule(&json!({"index":0,"dir":-1}))["ok"], true);
        let result = act_get_rules();
        let rows = result["rules"].as_array().unwrap();
        assert_eq!(rows[0]["pattern"], "low.invalid");
        assert_eq!(rows[1]["pattern"], "high.invalid");
        assert_eq!(rows[2]["rule_type"], "final");
        let saved = fs::read_to_string(conf_path()).unwrap();
        assert_eq!(act_move_rule(&json!({"index":1,"dir":1}))["ok"], false);
        assert_eq!(fs::read_to_string(conf_path()).unwrap(), saved);
    }

    #[test]
    fn save_config_validates_and_preserves_unknown_keys() {
        let _env = TestEnv::new(SAMPLE);
        let bad = B64.encode("listen_addr: 127.0.0.1\nnodes: [{name: x}]\n");
        let r = act_save_config(&json!({"config": bad}));
        assert_eq!(r["ok"], J::Bool(false), "schema violation rejected: {r}");
        assert!(fs::read_to_string(conf_path()).unwrap().contains("Node A"));

        let invalid_target = B64.encode(SAMPLE.replace("rules: []", "rules:\n  - rule_type: domain-suffix\n    pattern: example.invalid\n    target: group:missing\n    enabled: true"));
        let r = act_save_config(&json!({"config": invalid_target}));
        assert_eq!(r["ok"], J::Bool(false), "missing group rejected: {r}");
        assert!(fs::read_to_string(conf_path()).unwrap().contains("Node A"));

        let bad_index = B64.encode(SAMPLE.replace("active_node: 0", "active_node: 99"));
        let r = act_save_config(&json!({"config": bad_index}));
        assert_eq!(r["ok"], J::Bool(false), "unusable node index rejected: {r}");

        let bad_dns = B64.encode(format!(
            "{SAMPLE}\ndns:\n  strategy: ipv6_only\nipv6_mode: compatibility\n"
        ));
        let r = act_save_config(&json!({"config": bad_dns}));
        assert_eq!(r["ok"], J::Bool(false), "unusable DNS policy rejected: {r}");
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
        // Missing proxy_mode key → rule (Merlin historical behaviour).
        assert_eq!(s["proxy_mode"], J::from("rule"), "{s}");
    }

    #[test]
    fn set_proxy_mode_writes_key() {
        let _env = TestEnv::new(SAMPLE);
        // No live service in the test harness — save only.
        let r = act_set_proxy_mode(&json!({"proxy_mode": "global"}));
        assert_eq!(r["ok"], J::Bool(true), "{r}");
        assert_eq!(r["proxy_mode"], J::from("global"), "{r}");
        let map = load_conf().unwrap();
        assert_eq!(conf_str(&map, "proxy_mode").as_deref(), Some("global"));
        assert!(!conf_bool(&map, "cn_ipset_direct", true));
        let s = act_status();
        assert_eq!(s["proxy_mode"], J::from("global"), "{s}");
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

    #[test]
    fn set_toggles_proxy_off_keeps_dns_hijack() {
        // Regression: closing TUN must NOT force dns_hijack=false (that pushed
        // LAN onto ISP long TTLs and made re-enable wait ~1min for bare IPs).
        let cfg = SAMPLE.replace(
            "mode: \"tun\"",
            "mode: \"tun\"\ntransparent_proxy: true\ndns_hijack: true",
        );
        let _env = TestEnv::new(&cfg);
        // sockrocket.sh absent → proxy-off is a no-op; keys must still be correct.
        let _ = act_set_toggles(&json!({"transparent_proxy": false}));
        let map = load_conf().unwrap();
        assert_eq!(
            conf_str(&map, "transparent_proxy").as_deref(),
            Some("false"),
            "{map:?}"
        );
        assert_eq!(
            conf_str(&map, "dns_hijack").as_deref(),
            Some("true"),
            "dns_hijack must stay on when TUN is turned off"
        );
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

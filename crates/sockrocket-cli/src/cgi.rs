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

mod devices;
mod http;
mod network_settings;
mod proxy_groups;
mod route_trace;
mod rule_presets;
mod runtime;
mod speedtest_jobs;
mod storage;
mod subscription_jobs;
use runtime::CgiRuntime;
pub use speedtest_jobs::worker as speedtest_worker;
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
use sockrocket_core::config::storage::{parse_yaml_document, render_yaml_mapping};
use sockrocket_core::same_rule_group;

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
            parse_post_body(&body).map_err(|error| {
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

fn parse_post_body(body: &str) -> serde_json::Result<J> {
    if body.is_empty() {
        Ok(J::Null)
    } else {
        serde_json::from_slice(body.as_bytes())
    }
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

fn running_pid() -> Option<(u32, String)> {
    let text = fs::read_to_string(pid_path()).ok()?;
    let pid = text.trim().parse::<u32>().ok()?;
    // Match the process table used by the Merlin lifecycle controller.
    let proc = env::var_os("SOCKROCKET_PROC_BASE")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/proc".into());
    let comm = fs::read_to_string(proc.join(pid.to_string()).join("comm")).ok()?;
    comm.contains("sockrocket-cli").then_some((pid, text))
}

fn is_running() -> bool {
    running_pid().is_some()
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
    let mut lines: Vec<&str> = content.lines().rev().take(n).collect();
    lines.reverse();
    lines.join("\n")
}

fn message_response(ok: bool, message: String) -> J {
    let mut fields = serde_json::Map::new();
    fields.insert("ok".into(), J::Bool(ok));
    fields.insert("msg".into(), J::String(message));
    J::Object(fields)
}

fn ok_msg(message: impl Into<String>) -> J {
    message_response(true, message.into())
}

fn err_msg(message: impl Into<String>) -> J {
    message_response(false, message.into())
}

fn save_failed(error: &dyn std::fmt::Display) -> J {
    err_msg(format!("Save failed: {error}"))
}

// === Config (YAML, preserving unknown keys) ===

fn ykey(k: &str) -> Y {
    Y::String(k.to_string())
}

fn load_conf() -> Result<Mapping> {
    let content = fs::read_to_string(conf_path()).context("Config file not found")?;
    let mut value = parse_yaml_document(&content).context("Failed to parse config")?;
    sockrocket_core::config::node_identity::migrate_references(&mut value)?;
    match value {
        Y::Mapping(mut m) => {
            devices::migrate_bindings(&mut m)?;
            Ok(m)
        }
        Y::Null => {
            let mut m = Mapping::new();
            devices::migrate_bindings(&mut m)?;
            Ok(m)
        }
        _ => bail!("config root must be a mapping"),
    }
}

fn save_conf(map: &Mapping) -> Result<()> {
    let groups = sockrocket_core::config::model::proxy_groups_from_yaml(map)
        .map_err(|_| anyhow::anyhow!("Invalid proxy groups"))?;
    let rules = load_routing_rules(map);
    sockrocket_core::config::group::validate_targets(&groups, &rules)
        .map_err(anyhow::Error::msg)?;
    sockrocket_core::config::model::validate_device_rule_groups(
        &devices::links(map)?,
        &rules,
        &groups,
    )
    .map_err(anyhow::Error::msg)?;
    let text = render_yaml_mapping(map)?;
    storage::atomic_write(&text)
}

fn conf_str(map: &Mapping, key: &str) -> Option<String> {
    match map.get(key) {
        Some(Y::String(s)) => Some(s.clone()),
        Some(Y::Number(n)) => Some(n.to_string()),
        Some(Y::Bool(b)) => Some(b.to_string()),
        _ => None,
    }
}

fn conf_u16(map: &Mapping, key: &str, default: u16) -> u16 {
    map.get(key)
        .and_then(|v| v.as_u64())
        .and_then(|n| u16::try_from(n).ok())
        .filter(|port| *port > 0)
        .unwrap_or(default)
}

/// Read a boolean config key with a fallback. Accepts YAML booleans and the
/// quoted/string forms the router template may contain.
fn conf_bool(map: &Mapping, key: &str, default: bool) -> bool {
    match map.get(key) {
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
    map.get(key).and_then(|v| v.as_u64()).map(|n| n as usize)
}

fn conf_seq<'a>(map: &'a Mapping, key: &str) -> &'a [Y] {
    map.get(key)
        .and_then(Y::as_sequence)
        .map_or(&[], Vec::as_slice)
}

fn node_count(map: &Mapping) -> usize {
    conf_seq(map, "nodes").len()
}

fn node_display_field(node: &Y, field: &str) -> String {
    match node.get(field) {
        Some(Y::String(text)) => text.clone(),
        Some(Y::Number(number)) => number.to_string(),
        _ => String::new(),
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
            | "speedtest_start"
            | "speedtest_job"
            | "ip_check"
            | "check_update"
            | "update_status"
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
            | "fetch_rule_list"
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
    if sockrocket_dir().join(".update-transaction").exists() && !read_only_action(action) {
        if fs::read_to_string(sockrocket_dir().join("update-status"))
            .is_ok_and(|status| status.starts_with("rollback-failed:"))
        {
            return json!({"ok":false,"error_code":"update_recovery_required","msg":"升级回滚未完成，备份已保留。请查看升级状态和诊断日志。"});
        }
        return json!({"ok":false,"error_code":"update_in_progress","msg":"软件更新中，暂不能修改配置或服务，请稍后重试"});
    }
    let _transaction = if matches!(
        action,
        "stop" | "subscription_start" | "add_sub" | "update_subs"
    ) || read_only_action(action)
    {
        None
    } else {
        match storage::MutationGuard::acquire() {
            Ok(guard) => Some(guard),
            Err(e) => return err_msg(format!("Configuration transaction failed: {e}")),
        }
    };
    dispatch_inner(action, post)
}

fn dispatch_inner(action: &str, post: &J) -> J {
    match action {
        "status" => act_status(post.get("revision_scope").and_then(J::as_str)),
        "start" => act_start(),
        "stop" => act_stop(),
        "restart" => act_restart(),
        "nodes" => act_nodes(false),
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
        "stats" => act_stats(post.get("traffic_scope").and_then(J::as_str)),
        "set_mode" => act_set_mode(post),
        "set_proxy_mode" => act_set_proxy_mode(post),
        "set_toggles" => act_set_toggles(post),
        "logs_clear" => act_logs_clear(),
        "ip_check" => act_ip_check(),
        "speedtest_node" => act_speedtest_node(post),
        "speedtest_start" => speedtest_jobs::start(post),
        "speedtest_job" => speedtest_jobs::status(),
        "node_details" => act_nodes(true),
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
        "save_device_binding" => devices::save_binding(post),
        "save_device_alias" => devices::save_alias(post),
        "delete_device" => devices::delete(post),
        "save_builtin_route" => proxy_groups::save_builtin_route(post),
        "save_device_policy" => proxy_groups::device(post),
        "network_settings" => network_settings::get(),
        "save_network_settings" => network_settings::save(post),
        "dns_test" => network_settings::test(post),
        "get_rules" => act_get_rules(),
        "fetch_rule_list" => act_fetch_rule_list(post),
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
        "apply_update" => act_apply_update(post),
        "update_status" => act_update_status(),
        "diagnose" => act_diagnose(),
        _ => json!({"error": "Unknown action"}),
    }
}

fn act_status(revision_scope: Option<&str>) -> J {
    let map = load_conf().unwrap_or_default();
    let daemon = running_pid();
    let running = daemon.is_some();
    let pid = daemon.as_ref().map_or("", |(_, text)| text.trim());
    let (transparent_proxy, dns_hijack) = effective_toggles(&map);
    // Health-check runtime state, written by the daemon process on every
    // probe/switch event (tmpfs file). Null when the daemon has never run
    // with health_check.enabled.
    let health = fs::read_to_string(std::env::temp_dir().join("sockrocket_health.json"))
        .ok()
        .and_then(|s| serde_json::from_slice::<J>(s.as_bytes()).ok())
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
        "rules_revision": (!matches!(revision_scope, Some("none" | "network")))
            .then(|| route_trace::rules_revision(&map)),
        "network_revision": (!matches!(revision_scope, Some("none" | "rules")))
            .then(|| network_settings::revision(&map)),
        "ipv6_mode":conf_str(&map,"ipv6_mode").unwrap_or_else(||"compatibility".into()),
        "ipv6_active":network_settings::ipv6_active(running, transparent_proxy, conf_str(&map,"ipv6_mode").as_deref()==Some("dual_stack")),
        "transparent_proxy": transparent_proxy,
        "dns_hijack": dns_hijack,
        "cn_ipset_direct": conf_bool(&map, "cn_ipset_direct", false),
        "version": version_string(),
        "health": health,
        "watchdog": daemon.as_ref().map(|(pid, _)| crate::daemon_health::summary(*pid)).unwrap_or(J::Null),
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
        return err_msg(error.to_string());
    }
    std::thread::sleep(Duration::from_secs(1));
    if is_running() {
        ok_msg("Sockrocket started")
    } else {
        let last = tail_lines(&log_path(), 5).replace('\n', " ");
        err_msg(format!("Failed to start: {}", last))
    }
}

fn act_stop() -> J {
    // run_sh blocks until sockrocket.sh finishes, so the verdict below reflects the
    // real post-stop state rather than a sleep-and-hope guess.
    if let Err(error) = run_sh("stop") {
        return err_msg(error.to_string());
    }
    // Belt: if DNS hijack / MARK rules somehow survived stop, force restore.
    if dnsmasq_hijack_conf().exists()
        && let Err(error) = run_sh("network-restore")
    {
        return err_msg(error.to_string());
    }
    let stopped = !is_running();
    let hijack_gone = !dnsmasq_hijack_conf().exists();
    if stopped && hijack_gone {
        ok_msg("Sockrocket stopped — direct (non-proxy) network restored")
    } else {
        let last = tail_lines(&log_path(), 5).replace('\n', " ");
        err_msg(format!(
            "Stop not fully effective (process: {}, DNS hijack: {}). Retry or check logs: {}",
            if stopped { "stopped" } else { "still running" },
            if hijack_gone {
                "cleared"
            } else {
                "still active"
            },
            last
        ))
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
        return err_msg(error.to_string());
    }
    std::thread::sleep(Duration::from_secs(1));
    if is_running() {
        ok_msg("Sockrocket restarted")
    } else {
        err_msg("Restart failed, check log")
    }
}

fn act_nodes(with_details: bool) -> J {
    let Ok(map) = load_conf() else {
        return err_msg("无法读取节点配置，请检查配置文件后刷新。");
    };
    let nodes: Vec<J> = conf_seq(&map, "nodes")
        .iter()
        .enumerate()
        .map(|(i, node)| {
            let name = node_display_field(node, "name");
            if !with_details {
                return J::String(name);
            }
            // Older configurations store the protocol as a plain string.
            let protocol = match node.get("protocol") {
                Some(Y::String(name)) => name.as_str(),
                Some(Y::Mapping(config)) => config.get("type").and_then(Y::as_str).unwrap_or("?"),
                _ => "?",
            };
            json!({
                "index": i,
                "test_id": speedtest_jobs::key(i, node),
                "name": name,
                "protocol": protocol,
                "server": node_display_field(node, "server"),
                "port": node.get("port").and_then(Y::as_u64).unwrap_or(0) as u16,
            })
        })
        .collect();
    json!({"nodes": nodes, "active": conf_usize(&map, "active_node").unwrap_or(0)})
}

fn verify_node_selection(map: &Mapping, index: usize, post: &J) -> Result<(), J> {
    let node = map
        .get("nodes")
        .and_then(Y::as_sequence)
        .and_then(|seq| seq.get(index));
    let Some(node) = node else {
        return Err(err_msg("节点已不存在，请刷新列表。"));
    };
    if let Some(expected) = post.get("expected_id")
        && expected.as_str() != Some(&speedtest_jobs::key(index, node))
    {
        return Err(err_msg("节点列表已变化，请刷新后重新选择。"));
    }
    Ok(())
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
    if let Err(error) = verify_node_selection(&map, index, post) {
        return error;
    }
    set_active_node(&mut map, index);
    match save_conf(&map) {
        Ok(()) => {
            // Do NOT restart: the daemon's ConfigWatcher hot-reloads
            // active_node and swaps the outbound while listeners stay up.
            // A full restart drops SOCKS/HTTP mid-flight and makes "switch
            // then probe" look like the new node is dead.
            ok_msg("Active node updated")
        }
        Err(e) => save_failed(&e),
    }
}

fn act_log() -> J {
    json!({"log_base64": B64.encode(tail_lines(&log_path(), 150))})
}

fn content_digest(content: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(content))
}

fn act_config() -> J {
    match fs::read(conf_path()) {
        Ok(content) => {
            json!({"revision": content_digest(&content), "config_base64": B64.encode(content)})
        }
        Err(_) => err_msg("无法读取高级配置，请检查配置文件后重新加载。"),
    }
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
        return err_msg(format!("Config validation failed: {e}"));
    }
    // dispatch holds the same config lock used by subscription refresh commits.
    if let Some(expected) = post.get("expected_revision") {
        let Ok(current) = fs::read(conf_path()) else {
            return err_msg("无法核对当前配置，请重新加载。");
        };
        if expected.as_str() != Some(&content_digest(&current)) {
            return err_msg("配置已被其他操作更新，请保留草稿并重新加载后再保存。");
        }
        if current == text.as_bytes() {
            return json!({"ok": true, "revision": expected, "msg": "配置没有变化"});
        }
    }
    let revision = content_digest(text.as_bytes());
    if let Err(error) =
        sockrocket_core::config::storage::atomic_write_private(&conf_path(), &text, true)
    {
        return err_msg(format!("Failed to persist config: {error}"));
    }
    if is_running()
        && let Err(error) = run_sh("restart")
    {
        return json!({"ok": false, "saved": true, "revision": revision,
                      "msg": format!("配置已保存，服务重启失败，请查看诊断。{error}")});
    }
    json!({"ok": true, "saved": true, "revision": revision, "msg": "Config saved and validated"})
}

fn validate_advanced_config(text: &str) -> Result<()> {
    let raw = parse_yaml_document(text)?;
    let map = raw.as_mapping();
    let ipset_direct = map.is_some_and(|map| conf_bool(map, "cn_ipset_direct", false));
    let transparent = map.is_none_or(|map| effective_toggles(map).0);
    let dns = sockrocket_core::config::dns::DnsSettings::from_yaml(&raw);
    let config = sockrocket_core::AppConfig::from_yaml_value(raw)?;
    if !config.nodes.is_empty() {
        anyhow::ensure!(
            config
                .active_node
                .is_none_or(|index| index < config.nodes.len()),
            "active_node is out of range"
        );
    }
    anyhow::ensure!(
        config.builtin_route == sockrocket_core::config::model::BuiltinRoute::Direct
            || !ipset_direct,
        "国内 IP 直连加速与内置分组出口冲突"
    );
    let dns = dns?;
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
        return err_msg("无法读取订阅配置，请检查配置文件后刷新。");
    };
    let mut node_counts = std::collections::HashMap::<String, usize>::new();
    let mut shared_nodes = 0usize;
    for value in conf_seq(&map, "nodes") {
        let Ok(node) = sockrocket_core::Node::from_yaml_value(value.clone()) else {
            continue;
        };
        let sources = sockrocket_core::config::dedup::subscription_sources(&node);
        if sources.len() > 1 {
            shared_nodes += 1;
        }
        for source in sources {
            *node_counts.entry(source).or_default() += 1;
        }
    }
    let subs: Vec<J> = conf_seq(&map, "subscriptions")
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let get = |k: &str| match s.get(k) {
                Some(Y::String(v)) => v.clone(),
                _ => String::new(),
            };
            let url = get("url");
            let count = node_counts.get(&url).copied().unwrap_or(0);
            json!({"index": i, "id": content_digest(url.as_bytes()), "name": get("name"), "url": url,
                   "node_count": count,
                   "format": match get("format") { f if f.is_empty() => "auto".into(), f => f }})
        })
        .collect();
    json!({"subs": subs, "shared_nodes": shared_nodes})
}

fn sub_entry(name: &str, url: &str, format: &str) -> Y {
    let mut m = Mapping::new();
    m.insert(ykey("name"), Y::String(name.to_string()));
    m.insert(ykey("url"), Y::String(url.to_string()));
    m.insert(ykey("format"), Y::String(format.to_string()));
    m.insert(
        ykey("refresh_revision"),
        Y::String(sockrocket_core::config::storage::new_subscription_revision()),
    );
    Y::Mapping(m)
}

/// Fetch + parse a subscription URL on a current-thread runtime (CGI / API
/// handlers are sync). Hard-capped at [`SUB_FETCH_TIMEOUT_SECS`] so the Web UI
/// never waits forever on a hung airport URL.
fn local_socks_addr() -> Option<std::net::SocketAddr> {
    let port = load_app_config()
        .ok()
        .map(|config| config.socks_port)
        .filter(|port| *port != 0)
        .unwrap_or(1080);
    Some(std::net::SocketAddr::from(([127, 0, 0, 1], port)))
}

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
        refresh_revision: String::new(),
    };
    let rt = CgiRuntime::new().map_err(|e| format!("Failed to start fetch runtime: {e}"))?;
    let timeout = Duration::from_secs(sockrocket_core::SUB_FETCH_TIMEOUT_SECS);
    rt.block_on(async {
        match tokio::time::timeout(
            timeout,
            sockrocket_core::config::subscription::fetch_subscription_report_with(
                &sub,
                local_socks_addr(),
            ),
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
    sockrocket_core::AppConfig::from_yaml_str(&content)
        .map_err(|e| format!("Config parse failed: {e}"))
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
    let (name, revision) = {
        // Only the read/modify/commit step needs the configuration lock.
        // Downloading under this lock made every other edit time out.
        let Ok(_transaction) = storage::MutationGuard::acquire() else {
            return err_msg("Configuration transaction busy; retry shortly");
        };
        let Ok(mut map) = load_conf() else {
            return err_msg("Config file not found");
        };
        let mut seq = match map.remove("subscriptions") {
            Some(Y::Sequence(s)) => s,
            _ => Vec::new(),
        };
        if seq
            .iter()
            .any(|item| item.get("url").and_then(Y::as_str) == Some(url))
        {
            return json!({"ok":false,"error_code":"subscription_exists","stage":"validate_subscription",
                "cause":"DuplicateSubscription","msg":"该订阅已存在，请更新已有订阅。",
                "hint":"在“我的订阅”中点击“全部更新”，无需重复添加；不会覆盖已有节点。"});
        }
        let name = if name.is_empty() {
            let used: std::collections::HashSet<_> = seq
                .iter()
                .filter_map(|entry| entry.get("name").and_then(Y::as_str))
                .collect();
            let mut number = seq.len() + 1;
            while used.contains(format!("订阅 {number}").as_str()) {
                number += 1;
            }
            format!("订阅 {number}")
        } else {
            name.to_owned()
        };
        let entry = sub_entry(&name, url, format);
        let revision = entry
            .get("refresh_revision")
            .and_then(Y::as_str)
            .expect("new subscriptions have a refresh revision")
            .to_owned();
        seq.push(entry);
        map.insert(ykey("subscriptions"), Y::Sequence(seq));
        if let Err(e) = save_conf(&map) {
            return save_failed(&e);
        }
        (name, revision)
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
            subscription_jobs::progress(1, 1, "saving");
            let nodes = report.nodes;
            let fetched = nodes.len();
            let saved = sockrocket_core::config::storage::commit_subscription_refresh(
                &conf_path(),
                vec![(
                    sockrocket_core::config::storage::SubscriptionRefreshSnapshot {
                        url: url.to_string(),
                        revision,
                    },
                    nodes,
                )],
            )
            .map_err(|error| error.to_string());
            match saved {
                Ok(commit) if commit.applied_urls.iter().any(|source| source == url) => {
                    let total = commit.config.nodes.len();
                    subscription_jobs::progress(1, 1, "applying");
                    if let Err(error) = run_sh("restart") {
                        return err_msg(error.to_string());
                    }
                    json!({
                        "ok": true,
                        "msg": format!("已导入 {fetched} 个节点，共 {total} 个，设置已应用{warning}"),
                        "node_count": total,
                        "compatibility_notes": compatibility_notes
                    })
                }
                Ok(_) => json!({"ok":false,"error_code":"subscription_removed",
                    "msg":"订阅在下载期间被删除或已由另一任务更新；过期下载已丢弃，现有节点保持不变"}),
                Err(e) => err_msg(format!("Parsed {fetched} node(s) but failed to save: {e}")),
            }
        }
        Err(e) => json!({
            "ok": false,
            "msg": format!("Subscription saved, but fetch/parse failed: {e}"),
            "node_count": 0
        }),
    }
}

fn verify_subscription_selection(post: &J, url: &str) -> Result<(), J> {
    if let Some(expected) = post.get("expected_id")
        && expected.as_str() != Some(&content_digest(url.as_bytes()))
    {
        return Err(err_msg("订阅列表已变化，请刷新后重新选择。"));
    }
    Ok(())
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
    if let Err(error) = verify_subscription_selection(post, &url) {
        return error;
    }
    sockrocket_core::remove_subscription_nodes(&mut cfg, &url);
    match persist_subscription_state(&cfg) {
        Ok(_) => ok_msg("Subscription deleted"),
        Err(e) => save_failed(&e),
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
    let mut seq = match map.remove("subscriptions") {
        Some(Y::Sequence(s)) => s,
        _ => Vec::new(),
    };
    if index >= seq.len() {
        return err_msg("Subscription index out of range");
    }
    match &mut seq[index] {
        Y::Mapping(m) => {
            let url = m.get("url").and_then(Y::as_str).unwrap_or_default();
            if let Err(error) = verify_subscription_selection(post, url) {
                return error;
            }
            m.insert(ykey("name"), Y::String(name.to_string()));
        }
        _ => return err_msg("Invalid subscription entry"),
    }
    map.insert(ykey("subscriptions"), Y::Sequence(seq));
    match save_conf(&map) {
        Ok(()) => json!({"ok": true, "msg": "Subscription renamed", "name": name}),
        Err(e) => save_failed(&e),
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
        let get = |k: &str| match s.get(k) {
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
                refreshed.push((
                    sockrocket_core::config::storage::SubscriptionRefreshSnapshot {
                        url,
                        revision: get("refresh_revision"),
                    },
                    report.nodes,
                ));
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

    subscription_jobs::progress(subs.len(), subs.len(), "saving");
    let fetched_by_url: std::collections::HashMap<_, _> = refreshed
        .iter()
        .map(|(source, nodes)| (source.url.clone(), nodes.len()))
        .collect();
    let commit = match sockrocket_core::config::storage::commit_subscription_refresh(
        &conf_path(),
        refreshed,
    ) {
        Ok(cfg) => cfg,
        Err(error) => {
            return err_msg(format!("Fetched nodes but save failed: {error}"));
        }
    };
    let applied = &commit.applied_urls;
    if applied.is_empty() {
        return json!({"ok":false,"error_code":"subscriptions_removed",
            "msg":"订阅在下载期间被删除或已由另一任务更新；过期下载已丢弃，现有节点保持不变"});
    }
    let fetched: usize = applied.iter().map(|url| fetched_by_url[url]).sum();
    let dropped = fetched_by_url.len() - applied.len();
    if dropped > 0 {
        skipped_subscriptions += dropped;
        errors.push(format!(
            "{dropped} 个订阅在下载期间被删除或已由另一任务更新，其过期节点已丢弃"
        ));
    }
    let total = commit.config.nodes.len();
    subscription_jobs::progress(subs.len(), subs.len(), "applying");
    if let Err(error) = run_sh("restart") {
        return err_msg(error.to_string());
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
/// Resolve through the running daemon, independent of firmware curl features.
fn latency_via_active_socks() -> Result<u32, String> {
    let start = std::time::Instant::now();
    match crate::proxy_http::get(
        socks_port(),
        "www.gstatic.com",
        "/generate_204",
        Duration::from_secs(15),
    ) {
        Ok((200 | 204, _)) => Ok(start.elapsed().as_millis().min(u32::MAX as u128) as u32),
        _ => Err("Connection failed or timed out".into()),
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
    let parsed: J = match crate::proxy_http::get(
        socks_port(),
        "ip-api.com",
        "/json?fields=query,country",
        Duration::from_secs(12),
    ) {
        Ok((200, body)) => serde_json::from_slice(&body).unwrap_or(J::Null),
        _ => J::Null,
    };
    let ip = parsed.get("query").and_then(|v| v.as_str()).unwrap_or("");
    let country = parsed.get("country").and_then(|v| v.as_str()).unwrap_or("");
    if ip.is_empty() {
        json!({"ok": false, "ip": "", "country": "", "msg": "IP check failed; make sure the proxy is running"})
    } else {
        json!({"ok": true, "ip": ip, "country": country})
    }
}

fn use_tcp_probe(node: &sockrocket_core::Node, http: bool) -> bool {
    !http
        && !matches!(
            &node.protocol,
            sockrocket_core::ProxyProtocol::Tuic { .. }
                | sockrocket_core::ProxyProtocol::Hysteria2 { .. }
                | sockrocket_core::ProxyProtocol::Wireguard { .. }
        )
}

async fn probe_node_latency(node: &sockrocket_core::Node, quick: bool) -> Result<u32> {
    if quick {
        return sockrocket_core::tcp_latency_test(&node.server, node.port, 3).await;
    }
    let outbound = sockrocket_core::create_outbound_unwarmed(node)?;
    sockrocket_core::quick_http_latency_test(&outbound, 5).await
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
    let node = match sockrocket_core::Node::from_yaml_value(node.clone()) {
        Ok(n) => n,
        Err(e) => {
            return json!({"ok": false, "latency": -1, "msg": format!("Failed to parse config for node {index}: {e}")});
        }
    };
    let quick = use_tcp_probe(&node, post.get("mode").and_then(J::as_str) == Some("http"));
    let server = node.server.clone();
    let port = node.port;
    let result = std::thread::spawn(move || {
        let rt = match CgiRuntime::new() {
            Ok(rt) => rt,
            Err(e) => return Err(anyhow::anyhow!("runtime: {e}")),
        };
        rt.block_on(async {
            // Independent requests share limits across the API process.
            let slots = if quick {
                static TCP_SEM: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
                TCP_SEM.get_or_init(|| tokio::sync::Semaphore::new(16))
            } else {
                speedtest_semaphore()
            };
            let _permit = slots
                .acquire()
                .await
                .map_err(|_| anyhow::anyhow!("speedtest cancelled"))?;
            probe_node_latency(&node, quick).await
        })
    })
    .join();
    match result {
        Ok(Ok(ms)) => {
            json!({"ok": true, "latency": ms, "server": server, "port": port, "path": if quick { "tcp" } else { "http" }})
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
            grpc: None,
            xhttp: None,
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
        let valid = sockrocket_core::Node::from_yaml_value(Y::Mapping(node.clone()))
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
    let mut seq = match map.remove("nodes") {
        Some(Y::Sequence(s)) => s,
        _ => Vec::new(),
    };
    seq.push(Y::Mapping(node));
    map.insert(ykey("nodes"), Y::Sequence(seq));
    match save_conf(&map) {
        Ok(()) => ok_msg(format!("Node added: {name}")),
        Err(e) => save_failed(&e),
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
    if let Err(error) = verify_node_selection(&map, index, post) {
        return error;
    }
    let mut seq = match map.remove("nodes") {
        Some(Y::Sequence(s)) => s,
        _ => Vec::new(),
    };
    seq.remove(index);
    let count = seq.len();
    map.insert(ykey("nodes"), Y::Sequence(seq));
    if count == 0 {
        map.remove("active_node");
    } else if let Some(active) = conf_usize(&map, "active_node") {
        let shifted = if index < active { active - 1 } else { active };
        set_active_node(&mut map, shifted.min(count - 1));
    }
    match save_conf(&map) {
        Ok(()) => ok_msg("Node deleted"),
        Err(e) => save_failed(&e),
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
                return err_msg(error.to_string());
            }
            ok_msg("Mode set; takes effect after restart")
        }
        Err(e) => save_failed(&e),
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
        return save_failed(&e);
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
            return err_msg(error.to_string());
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
    if want_cn == Some(true)
        && conf_str(&map, "proxy_mode").is_some_and(|mode| mode.eq_ignore_ascii_case("global"))
    {
        return err_msg("全局模式下不能开启国内 IP 绕过 TUN");
    }
    let (cur_proxy, mut cur_dns) = effective_toggles(&map);
    if want_cn == Some(true)
        && proxy_groups::builtin_route(&map).ok()
            != Some(sockrocket_core::config::model::BuiltinRoute::Direct)
    {
        return err_msg("内置国内 / 局域网分组使用直连时才能开启国内直连加速");
    }
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
            Err(e) => return err_msg(format!("Failed to read config: {e}")),
        };
        map.insert(ykey("transparent_proxy"), Y::Bool(new_proxy));
        if let Err(e) = save_conf(&map) {
            return save_failed(&e);
        }
        if new_proxy {
            // Full transparent path: TUN + DNS hijack. Fake-IP only exists
            // with --tun; without TUN, hijack still serves short-TTL real IPs
            // (safe). TUN without hijack leaves clients on polluted ISP DNS.
            map.insert(ykey("dns_hijack"), Y::Bool(true));
            if let Err(e) = save_conf(&map) {
                return save_failed(&e);
            }
            // restart brings up --tun + iptables + dnsmasq per the keys.
            // do_start rewrites `mode` on disk while we are in here.
            if let Err(error) = run_sh("restart") {
                return err_msg(error.to_string());
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
                    Err(e) => return err_msg(format!("Failed to read config: {e}")),
                };
                map.insert(ykey("transparent_proxy"), Y::Bool(false));
                // Keep DNS hijack on so LAN stays on short-TTL sockrocket DNS
                // (no Fake-IP without --tun) instead of ISP long-TTL.
                map.insert(ykey("dns_hijack"), Y::Bool(true));
                let _ = save_conf(&map);
                // Config now says TUN off — tear MARK rules and restart plain
                // daemon; DNS hijack is re-applied by do_start when wanted.
                if let Err(error) = run_sh("proxy-off") {
                    return err_msg(error.to_string());
                }
                return err_msg(
                    "Failed to enable transparent proxy: TUN/iptables/DNS hijack not ready (check the tun module and logs)",
                );
            }
        } else {
            // Turn TUN/iptables off but KEEP dns_hijack. Without --tun the
            // daemon serves real IPs (no Fake-IP) with short TTL — LAN stays
            // online, and re-enabling TUN recovers in seconds instead of
            // waiting out ISP DNS TTLs (~60s+).
            if let Err(error) = run_sh("proxy-off") {
                return err_msg(error.to_string());
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
                    Err(e) => return err_msg(format!("Failed to read config: {e}")),
                };
                map.insert(ykey("transparent_proxy"), Y::Bool(true));
                let _ = save_conf(&map);
                return err_msg("Failed to disable transparent proxy: iptables MARK still present");
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
                    return err_msg(error.to_string());
                }
                if !dnsmasq_hijack_conf().exists() {
                    return err_msg(
                        "Transparent proxy is off, but DNS hijack is missing — LAN may be on ISP DNS. Check that the DNS port is listening, then toggle DNS hijack on.",
                    );
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
            Err(e) => return err_msg(format!("Failed to read config: {e}")),
        };
        map.insert(ykey("dns_hijack"), Y::Bool(new_dns));
        if let Err(e) = save_conf(&map) {
            return save_failed(&e);
        }
        if let Err(error) = run_sh(if new_dns { "dns-on" } else { "dns-off" }) {
            return err_msg(error.to_string());
        }
        let installed = dnsmasq_hijack_conf().exists();
        if installed != new_dns {
            let mut map = match load_conf() {
                Ok(m) => m,
                Err(e) => return err_msg(format!("Failed to read config: {e}")),
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
            Err(e) => return err_msg(format!("Failed to read config: {e}")),
        };
        map.insert(ykey("cn_ipset_direct"), Y::Bool(new_cn));
        if let Err(e) = save_conf(&map) {
            return save_failed(&e);
        }
        let chain_up = run_cmd(
            "iptables",
            &["-t", "mangle", "-L", "SOCKROCKET_MANGLE", "-n"],
        )
        .is_some();
        if chain_up {
            let action = if new_cn { "ipset-on" } else { "ipset-off" };
            let script = iptables_sh().to_string_lossy().to_string();
            let succeeded = run_cmd("sh", &[&script, action]).is_some();
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
                    "-g",
                    "SOCKROCKET_CN_FLOW",
                ],
            )
            .is_some();
            if !succeeded || applied != new_cn {
                let mut map = match load_conf() {
                    Ok(m) => m,
                    Err(e) => return err_msg(format!("Failed to read config: {e}")),
                };
                map.insert(ykey("cn_ipset_direct"), Y::Bool(cur_cn));
                if save_conf(&map).is_ok() {
                    let restore = if cur_cn { "ipset-on" } else { "ipset-off" };
                    let _ = run_cmd("sh", &[&script, restore]);
                }
                return err_msg(
                    "Failed to toggle CN direct acceleration: ipset rules not applied (make sure ipset is available and the service is started)",
                );
            }
        }
    }

    ok_msg("Toggles updated")
}

fn act_logs_clear() -> J {
    let _ = fs::write(log_path(), "");
    ok_msg("Log cleared")
}

/// Subscription table = current members' node history, not the side ledger.
/// The side ledger misses traffic recorded before a node was linked to a subscription.
fn subscription_month_rows(
    nodes: &[sockrocket_core::NodeMonthlyTraffic],
    traffic: &sockrocket_core::traffic::SavedTraffic,
    month_keys: &[String],
) -> Vec<sockrocket_core::NodeMonthlyTraffic> {
    let Ok(cfg) = load_app_config() else {
        return traffic.subscription_months(month_keys);
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

fn act_stats(scope: Option<&str>) -> J {
    let pid = running_pid().map(|(pid, _)| pid);
    let mut uptime = "stopped".to_string();
    if let Some(pid_val) = pid
        && let (Ok(stat), Ok(proc_stat)) = (
            fs::read_to_string(format!("/proc/{}/stat", pid_val)),
            fs::read_to_string("/proc/stat"),
        )
    {
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
    let mut response = json!({"ok":true,"uptime":uptime});
    if scope == Some("none") {
        return response;
    }
    let session = scope != Some("monthly");
    let monthly = scope != Some("session");
    let full = session && monthly;
    if full {
        let (ipv4, ipv6) = firewall_hooks(run_cmd);
        response["pid"] = json!(pid.unwrap_or(0));
        response["dns_active"] = json!(dnsmasq_hijack_conf().exists());
        response["fw_active"] = json!(ipv4);
        response["fw_ipv6"] = json!(ipv6);
    }
    let traffic_path = sockrocket_dir().join("traffic.yaml");
    let traffic = sockrocket_core::traffic::SavedTraffic::read(&traffic_path).unwrap_or_default();
    let mut summary = J::Null;
    if session {
        let nodes = traffic
            .nodes()
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
        summary["nodes"] = J::Array(nodes);
    }
    if monthly {
        let month_keys = sockrocket_core::recent_month_keys(3);
        let recent = |rows: Vec<sockrocket_core::NodeMonthlyTraffic>, include_id: bool| {
            J::Array(
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
                    .collect(),
            )
        };
        let recent_months = traffic.node_months(&month_keys);
        let recent_subscriptions = subscription_month_rows(&recent_months, &traffic, &month_keys);
        summary["recent_month_keys"] = json!(month_keys);
        summary["recent_months"] = recent(recent_months, true);
        summary["recent_subscriptions"] = recent(recent_subscriptions, false);
    }
    if full {
        summary["session_started"] = json!(traffic.session_started());
    }
    response["traffic"] = summary;
    response
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

fn firewall_hooks(
    command: impl Fn(&str, &[&str]) -> Option<String>,
) -> (Option<bool>, Option<bool>) {
    let mounted = |tool, chain: &str| {
        command(tool, &["-t", "mangle", "-n", "-L", "PREROUTING"]).map(|listing| {
            listing
                .lines()
                .any(|line| line.split_whitespace().next() == Some(chain))
        })
    };
    (
        mounted("iptables", "SOCKROCKET_MANGLE"),
        mounted("ip6tables", "SOCKROCKET_IPV6_MANGLE"),
    )
}

fn memory_usage(meminfo: &str) -> (Option<u64>, Option<u64>, &'static str) {
    let kb = |key: &str| -> Option<u64> {
        meminfo.lines().find_map(|line| {
            line.strip_prefix(key)?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
    };
    let total = kb("MemTotal:").filter(|v| *v > 0);
    let estimate = || {
        Some(
            kb("MemFree:")?
                .saturating_add(kb("Buffers:")?)
                .saturating_add(kb("Cached:")?),
        )
    };
    let (available, source) = match (total, kb("MemAvailable:")) {
        (Some(_), Some(value)) => (Some(value), "measured"),
        (Some(_), None) => match estimate() {
            Some(value) => (Some(value), "estimated"),
            None => (None, "unknown"),
        },
        _ => (None, "unknown"),
    };
    (
        total,
        total.zip(available).map(|(t, a)| t.saturating_sub(a)),
        source,
    )
}

fn wan_addresses(mut command: impl FnMut(&str, &[&str]) -> Option<String>) -> Vec<String> {
    use std::net::IpAddr;
    fn add(addresses: &mut Vec<IpAddr>, value: &str) {
        if let Ok(ip) = value.trim().parse::<IpAddr>()
            && !ip.is_unspecified()
            && !ip.is_loopback()
            && !addresses.contains(&ip)
        {
            addresses.push(ip);
        }
    }
    let mut addresses = Vec::new();
    // Local route lookups report the kernel-selected source, including PPP interfaces.
    for (family, destination) in [("-4", "192.0.2.1"), ("-6", "2001:db8::1")] {
        if let Some(route) = command("ip", &[family, "route", "get", destination])
            && let Some(src) = route.split_whitespace().skip_while(|t| *t != "src").nth(1)
        {
            add(&mut addresses, src);
        }
    }
    let has_ipv4_route = addresses.iter().any(IpAddr::is_ipv4);
    let mut nvram = |key: &str| {
        command("nvram", &["get", key])
            .unwrap_or_default()
            .trim()
            .to_owned()
    };
    if nvram("wans_mode") == "lb" {
        for unit in [0, 1] {
            if nvram(&format!("wan{unit}_state_t")) == "2" {
                add(&mut addresses, &nvram(&format!("wan{unit}_ipaddr")));
            }
        }
    } else if !has_ipv4_route {
        let unit = if nvram("wan1_primary") == "1" { 1 } else { 0 };
        let state = nvram(&format!("wan{unit}_state_t"));
        if state.is_empty() || state == "2" {
            add(&mut addresses, &nvram(&format!("wan{unit}_ipaddr")));
        }
    }
    addresses.sort_by_key(IpAddr::is_ipv6);
    addresses.iter().map(ToString::to_string).collect()
}

fn act_sysinfo() -> J {
    let meminfo = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let (mem_total, mem_used, mem_source) = memory_usage(&meminfo);
    let mem_pct = mem_used
        .zip(mem_total)
        .map(|(used, total)| ((used as u128 * 100 + total as u128 / 2) / total as u128) as u64);
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
    let wan_addresses = wan_addresses(run_cmd);
    // Per-process RSS so the UI can show Sockrocket's own footprint vs
    // system-wide MemAvailable pressure (often ~40–50% on Merlin even idle).
    let daemon_rss_kb = running_pid()
        .map(|(_, text)| vm_rss_kb(text.trim()))
        .unwrap_or(0);
    let api_rss_kb = vm_rss_kb(&std::process::id().to_string());
    json!({
        "ok": true,
        "mem_total_kb": mem_total,
        "mem_used_kb": mem_used,
        "mem_pct": mem_pct,
        "memory_source": mem_source,
        "daemon_rss_kb": daemon_rss_kb,
        "api_rss_kb": api_rss_kb,
        "load": load,
        "uptime": format!("{}h {}m", uptime_s / 3600, (uptime_s % 3600) / 60),
        "wan_ip": wan_addresses.first().map(String::as_str).unwrap_or(""),
        "wan_addresses": wan_addresses,
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
        return err_msg(format!("Domain {domain} already exists"));
    }
    let mut seq = match map.remove("dns_direct_domains") {
        Some(Y::Sequence(s)) => s,
        _ => Vec::new(),
    };
    seq.push(Y::String(domain.to_string()));
    map.insert(ykey("dns_direct_domains"), Y::Sequence(seq));
    match save_conf(&map) {
        Ok(()) => ok_msg(format!("Added: {domain}")),
        Err(e) => save_failed(&e),
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
    let mut seq = match map.remove("dns_direct_domains") {
        Some(Y::Sequence(s)) => s,
        _ => Vec::new(),
    };
    if index < seq.len() {
        seq.remove(index);
    }
    map.insert(ykey("dns_direct_domains"), Y::Sequence(seq));
    match save_conf(&map) {
        Ok(()) => ok_msg("Deleted"),
        Err(e) => save_failed(&e),
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
                return err_msg(error.to_string());
            }
            ok_msg(format!(
                "DNS server updated to {server}; takes effect after restart"
            ))
        }
        Err(e) => save_failed(&e),
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
            .get("source")
            .and_then(Y::as_str)
            .unwrap_or("")
            .to_string(),
        name: r
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        group: r
            .get("group")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        rule_type: r.get("rule_type")?.as_str()?.to_string(),
        pattern: r.get("pattern")?.as_str()?.to_string(),
        target: r
            .get("target")
            .and_then(|v| v.as_str())
            .unwrap_or("proxy")
            .to_string(),
        enabled: r.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
        priority: r.get("priority").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
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
    map.get("rules")
        .and_then(|v| v.as_sequence())
        .map(|seq| seq.iter().filter_map(yaml_to_routing_rule).collect())
        .unwrap_or_default()
}

fn prune_device_links(map: &mut Mapping, rules: &[sockrocket_core::RoutingRule]) {
    if let Ok(mut links) = devices::links(map) {
        links.retain(|b| {
            rules
                .iter()
                .any(|r| r.is_global_destination() && r.group.trim() == b.group)
        });
        devices::store_links(map, &links);
    }
}

fn save_routing_rules(map: &mut Mapping, rules: Vec<sockrocket_core::RoutingRule>) {
    prune_device_links(map, &rules);
    let seq: Vec<Y> = rules.into_iter().map(routing_rule_to_yaml).collect();
    map.insert(ykey("rules"), Y::Sequence(seq));
}

fn rule_kind(rule_type: &str) -> &str {
    if rule_type.eq_ignore_ascii_case("match") || rule_type.eq_ignore_ascii_case("final") {
        "final"
    } else {
        rule_type
    }
}

// The Merlin group editor manages destination rules. Older API callers can
// still edit complete groups, including final rules.
fn includes_group_rule(post: &J, rule: &sockrocket_core::RoutingRule) -> bool {
    post["destinations_only"].as_bool() != Some(true) || rule_kind(&rule.rule_type) != "final"
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
        .get("rules")
        .is_some_and(|v| !matches!(v, Y::Null | Y::Sequence(_)))
        || parsed.len() != conf_seq(&map, "rules").len()
    {
        return err_msg("部分规则配置损坏，未返回不完整的规则列表，请检查诊断");
    }
    // Keep editor snapshots unchanged. Index this separate list by saved rule
    // number so the diagram uses the same accepted rules and actions as routing.
    let compiled = sockrocket_core::router::RuleSet::from_config(&parsed);
    let mut compiled_targets = vec![None; parsed.len()];
    for entry in compiled.rules() {
        if let Some(index) = entry.config_index {
            compiled_targets[index] = Some(&entry.action);
        }
    }
    let revision = route_trace::revision_for_rules(&map, &parsed);
    let rules: Vec<J> = parsed
        .into_iter()
        .enumerate()
        .map(|(i, rule)| {
            let mut row = rule_edit_snapshot(&rule);
            row["index"] = json!(i);
            row
        })
        .collect();
    let Ok(links) = devices::links(&map) else {
        return err_msg("设备规则组关联配置无效");
    };
    let Ok(builtin_route) = proxy_groups::builtin_route(&map) else {
        return err_msg("内置分组出口配置无效");
    };
    let active_node_name = conf_seq(&map, "nodes")
        .get(conf_usize(&map, "active_node").unwrap_or(0))
        .and_then(|node| node.get("name"))
        .and_then(Y::as_str)
        .map(str::to_owned);
    let (groups, nodes) = proxy_groups::rule_catalogs(&map);
    json!({"ok": true, "active_node_name":active_node_name, "builtin_route":builtin_route, "devices":devices::catalog(&map), "device_rule_groups":links.iter().filter(|b| !b.source.is_empty()).collect::<Vec<_>>(), "global_rule_groups":links.iter().filter(|b| b.source.is_empty()).collect::<Vec<_>>(), "rules": rules, "compiled_targets":compiled_targets, "revision": revision, "presets":rule_presets::catalog(), "proxy_groups":groups, "nodes":nodes, "proxy_mode":conf_str(&map,"proxy_mode").unwrap_or_else(||"rule".into()).to_ascii_lowercase()})
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
    let mut seq = match map.remove("rules") {
        Some(Y::Sequence(s)) => s,
        _ => Vec::new(),
    };
    let source = match requested_rule_source(post, "") {
        Ok(value) => value,
        Err(error) => return err_msg(&error),
    };
    let group = post["group"].as_str().unwrap_or("").trim();
    let dup = seq.iter().any(|r| {
        r.get("group").and_then(Y::as_str).unwrap_or("").trim() == group
            && r.get("rule_type").and_then(|v| v.as_str()) == Some(rule_type)
            && r.get("pattern").and_then(|v| v.as_str()) == Some(pattern.as_str())
            && r.get("source").and_then(Y::as_str).unwrap_or("") == source
    });
    if dup {
        return err_msg(format!("Rule already exists: {rule_type} {pattern}"));
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
        .position(|r| r.get("priority").and_then(|v| v.as_i64()).unwrap_or(0) as i32 <= priority)
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
                return err_msg(error.to_string());
            }
            ok_msg(format!(
                "Added {rule_type} {pattern} -> {target} (takes effect after service restart)"
            ))
        }
        Err(e) => save_failed(&e),
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
            && r.group.trim() == post["group"].as_str().unwrap_or(&old.group).trim()
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
                return err_msg(error.to_string());
            }
            ok_msg("规则已保存")
        }
        Err(e) => save_failed(&e),
    }
}

fn act_del_rule(post: &J) -> J {
    let Some(index) = post
        .get("index")
        .and_then(|v| v.as_u64())
        .and_then(|n| usize::try_from(n).ok())
    else {
        return err_msg("Invalid parameters");
    };
    let Ok(mut map) = load_conf() else {
        return err_msg("Config file not found");
    };
    let previous_sources = accelerated_rule_sources(&map);
    let Some(seq) = map.get_mut("rules").and_then(Y::as_sequence_mut) else {
        return err_msg("Rule index out of range");
    };
    if index >= seq.len() {
        return err_msg("Rule index out of range");
    }
    if stale_rule_request(post, &seq[index]) {
        return err_msg("规则已变化，请刷新后重试");
    }
    seq.remove(index);
    let remaining = load_routing_rules(&map);
    prune_device_links(&mut map, &remaining);
    save_hot_rules(&map, previous_sources)
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
    let mut seq = match map.remove("rules") {
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
    let final_rule = |r: &Y| {
        matches!(
            r.get("rule_type").and_then(Y::as_str),
            Some("final" | "match")
        )
    };
    let phase = |r: &Y| {
        u8::from(r.get("source").and_then(Y::as_str).unwrap_or("").is_empty()) * 2
            + u8::from(final_rule(r))
    };
    let mut order: Vec<_> = seq
        .iter()
        .enumerate()
        .map(|(index, rule)| {
            let priority = rule.get("priority").and_then(Y::as_i64).unwrap_or(0);
            (phase(rule), std::cmp::Reverse(priority), index)
        })
        .collect();
    order.sort_unstable();
    let index = order
        .iter()
        .position(|&(_, _, original)| original == index)
        .expect("validated index");
    seq = order
        .into_iter()
        .map(|(_, _, original)| std::mem::take(&mut seq[original]))
        .collect();
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
                return err_msg(error.to_string());
            }
            ok_msg("Order updated (takes effect after service restart)")
        }
        Err(e) => save_failed(&e),
    }
}

/// Read a remote list for preview without changing saved configuration.
fn act_fetch_rule_list(post: &J) -> J {
    let url = post.get("url").and_then(J::as_str).unwrap_or("").trim();
    let target = post.get("target").and_then(J::as_str).unwrap_or("proxy");
    let group = post.get("group").and_then(J::as_str).unwrap_or("").trim();
    if !proxy_groups::valid_target(target) {
        return err_msg("请选择有效的规则出口");
    }
    let result = (|| -> Result<_> {
        let rt = CgiRuntime::new()?;
        rt.block_on(sockrocket_core::config::rule_list::fetch_rule_list(
            url,
            target,
            group,
            if is_running() {
                local_socks_addr()
            } else {
                None
            },
        ))
    })();
    match result {
        Ok(preview) => {
            json!({"ok":true,"text":preview.text,"count":preview.count,"skipped":preview.skipped})
        }
        Err(error) => err_msg(format!("{error:#}")),
    }
}

fn parse_pasted_rules(
    text: &str,
    rule_type: &str,
    target: &str,
    group: &str,
) -> Result<Vec<sockrocket_core::RoutingRule>, J> {
    // Unified Clash-style parse (`TYPE,pattern,TARGET`). Plain domains/CIDRs
    // still auto-detect; typed paste without TYPE is accepted when `rule_type`
    // is set (WebUI type dropdown) for one-line-per-pattern convenience.
    let has_rule_types = || {
        text.lines().map(str::trim).any(|l| {
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
        })
    };
    let parsed = if sockrocket_core::rule_type_supports_bulk(rule_type) && !has_rule_types() {
        sockrocket_core::parse_typed_patterns(text, rule_type, target, group)
    } else {
        sockrocket_core::parse_batch_rules_in_group(text, target, group)
    };
    if let Some(error) = parsed.errors.first() {
        return Err(err_msg(format!(
            "第 {} 行：{}；未保存任何更改",
            error.line, error.message
        )));
    }
    Ok(parsed.rules)
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
    let group_only = post.get("group_only").and_then(J::as_bool) == Some(true);
    let replace = !group_only
        && post
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
    let mut incoming = match parse_pasted_rules(text, rule_type, default_target, group) {
        Ok(rules) if rules.is_empty() => return err_msg("No valid rules"),
        Ok(rules) => rules,
        Err(error) => return error,
    };
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
    for rule in &mut incoming {
        if !includes_group_rule(post, rule) {
            return err_msg("默认出口请在设备分流策略中设置");
        }
        if group_only {
            if group.is_empty() || !source.is_empty() || !rule.is_global_destination() {
                return err_msg("请选择具名规则组并导入目的规则；设备来源和默认出口请在绑定处配置");
            }
            rule.group = group.to_string();
        }
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
                    r.group.trim() == rule.group.trim()
                        && rule_kind(&r.rule_type) == rule_kind(&rule.rule_type)
                        && r.pattern == rule.pattern
                        && r.source == rule.source
                })
                .map_or(0, |r| r.priority)
        };
    }
    let n_in = incoming.len();
    let stats =
        sockrocket_core::config::rules_batch::merge_rules(&mut existing, incoming, conflict, true);
    let summary = format!(
        "共解析 {n_in} 条，新增 {} 条，替换 {} 条，跳过重复 {} 条。",
        stats.added, stats.replaced, stats.skipped
    );
    if stats.added == 0 && stats.replaced == 0 {
        return ok_msg(summary);
    }
    let previous_sources = accelerated_rule_sources(&map);
    save_routing_rules(&mut map, existing);
    let mut result = save_hot_rules(&map, previous_sources);
    if result["ok"] == true {
        result["msg"] = json!(format!(
            "{summary}{}",
            result["msg"].as_str().unwrap_or_default()
        ));
    }
    result
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
                return err_msg(error.to_string());
            }
            json!({
                "ok": true,
                "msg": format!(
                    "Scene「{group_label}」: +{} of {} rules ({} already present)",
                    stats.added, n, stats.skipped
                )
            })
        }
        Err(e) => save_failed(&e),
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
        if expected != &J::Array(current) {
            return err_msg("分组规则已变化，请刷新后重新选择出口");
        }
    }
    let changed = match sockrocket_core::assign_group_outlet(&mut rules, &group, target) {
        Ok(changed) => changed,
        Err(_) => return err_msg("无法设置这个分组的出口"),
    };
    if changed == 0 {
        return ok_msg("分组出口未变化");
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
        Err(e) => save_failed(&e),
    }
}

fn accelerated_rule_sources(map: &Mapping) -> std::collections::BTreeSet<String> {
    if !effective_toggles(map).0
        || !conf_bool(map, "cn_ipset_direct", false)
        || conf_str(map, "proxy_mode").is_some_and(|m| !m.eq_ignore_ascii_case("rule"))
    {
        return Default::default();
    }
    let links = devices::links(map).unwrap_or_default();
    let rules = load_routing_rules(map);
    let dual_stack = conf_str(map, "ipv6_mode").as_deref() == Some("dual_stack");
    sockrocket_core::router::device_policy_sources(&rules, &links)
        .filter(|source| dual_stack || !source.contains(':'))
        .map(str::to_owned)
        .collect()
}

/// Existing streams keep their outbound. The daemon's config watcher publishes
/// the new router for subsequent connections without rebinding any listener.
fn save_hot_rules(map: &Mapping, previous_sources: std::collections::BTreeSet<String>) -> J {
    if let Err(error) = save_conf(map) {
        return err_msg(format!("保存规则失败：{error}"));
    }
    if !is_running() {
        return json!({"ok":true,"saved":true,"applied":false,"msg":"规则已保存，下次启动时生效。"});
    }
    if accelerated_rule_sources(map) != previous_sources
        && let Err(error) = run_sh("refresh-rule-sources")
    {
        return json!({"ok":false,"saved":true,"applied":false,"error_code":"rule_apply_failed",
            "msg":"规则已保存，国内直连加速的设备范围未能同步，请查看诊断。","detail":error.to_string()});
    }
    json!({"ok":true,"saved":true,"reload_pending":true,"msg":"规则已保存，代理将在后台热加载，新连接使用更新后的规则。"})
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
            .filter(|r| includes_group_rule(post, r) && same_rule_group(&r.group, &group))
            .map(rule_edit_snapshot)
            .collect();
        if expected != &J::Array(current) {
            return err_msg("分组规则已变化，请刷新后重新选择开启或停用");
        }
    }
    let mut n = 0usize;
    let mut changed = false;
    for r in &mut rules {
        if includes_group_rule(post, r) && same_rule_group(&r.group, &group) {
            changed |= r.enabled != enabled;
            r.enabled = enabled;
            n += 1;
        }
    }
    if n == 0 {
        return err_msg("No rules in that group");
    }
    if !changed {
        return ok_msg("分组已处于所选状态");
    }
    let previous_sources = accelerated_rule_sources(&map);
    save_routing_rules(&mut map, rules);
    save_hot_rules(&map, previous_sources)
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

    let mut incoming = if text.trim().is_empty() {
        if !matches!(rule_type, "match" | "final") {
            return err_msg("Paste the group's patterns first");
        }
        match sockrocket_core::make_typed_rule("match", "*", target, &new_group) {
            Ok(rule) => vec![rule],
            Err(error) => return err_msg(&error),
        }
    } else {
        match parse_pasted_rules(text, rule_type, target, &new_group) {
            Ok(rules) if rules.is_empty() => return err_msg("No valid patterns in the group"),
            Ok(rules) => rules,
            Err(error) => return error,
        }
    };
    if incoming.iter().any(|r| !includes_group_rule(post, r)) {
        return err_msg("默认出口请在设备分流策略中设置");
    }
    let Ok(mut map) = load_conf() else {
        return err_msg("Config file not found");
    };
    let mut rules = load_routing_rules(&map);
    let members: Vec<_> = rules
        .iter()
        .filter(|r| includes_group_rule(post, r) && same_rule_group(&r.group, &old_group))
        .collect();
    if members.is_empty() {
        return err_msg("分组已不存在，请刷新后重试");
    }
    if let Some(expected) = post.get("expected_rules") {
        let current: Vec<_> = members.iter().map(|r| rule_edit_snapshot(r)).collect();
        if expected != &J::Array(current) {
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
        rule.group.clone_from(&new_group);
        if post.get("source").is_some() || rule.source.is_empty() {
            rule.source.clone_from(&source);
        }
        let previous = members.iter().find(|r| {
            rule_kind(&r.rule_type) == rule_kind(&rule.rule_type)
                && r.pattern == rule.pattern
                && r.source == rule.source
        });
        rule.enabled = if post.get("enabled").is_some() {
            enabled
        } else {
            previous.map_or(default_enabled, |r| r.enabled)
        };
        rule.priority = if post.get("priority").is_some() {
            priority
        } else {
            previous.map_or(default_priority, |r| r.priority)
        };
        if let Some(previous) = previous {
            rule.name.clone_from(&previous.name);
        }
    }
    if let Err(e) = sockrocket_core::config::rule_groups::replace_rule_group_where(
        &mut rules,
        &old_group,
        incoming,
        |r| includes_group_rule(post, r),
    ) {
        return err_msg(&e);
    }
    let previous_sources = accelerated_rule_sources(&map);
    if old_group != new_group
        && let Ok(mut links) = devices::links(&map)
    {
        for b in &mut links {
            if b.group == old_group {
                b.group.clone_from(&new_group);
            }
        }
        devices::store_links(&mut map, &links);
    }
    save_routing_rules(&mut map, rules);
    save_hot_rules(&map, previous_sources)
}

fn act_delete_rule_group(post: &J) -> J {
    let Some(group) = post.get("group").and_then(J::as_str) else {
        return err_msg("请选择要删除的规则组");
    };
    let Ok(mut map) = load_conf() else {
        return err_msg("Config file not found");
    };
    if post.get("expected_revision").and_then(J::as_str) != Some(&route_trace::rules_revision(&map))
    {
        return err_msg("规则或关联配置已变化，请刷新后重新确认删除");
    }
    let previous_sources = accelerated_rule_sources(&map);
    let Some(rows) = map.get_mut("rules").and_then(Y::as_sequence_mut) else {
        return err_msg("无法读取规则列表");
    };
    if rows.iter().any(|row| yaml_to_routing_rule(row).is_none()) {
        return err_msg("规则配置损坏，请先修复后再删除规则组");
    }
    let before = rows.len();
    rows.retain(|row| {
        !includes_group_rule(post, &yaml_to_routing_rule(row).unwrap())
            || !same_rule_group(row.get("group").and_then(Y::as_str).unwrap_or(""), group)
    });
    let removed = before - rows.len();
    if removed == 0 {
        return err_msg("该组没有可删除的规则");
    }
    let remaining = load_routing_rules(&map);
    prune_device_links(&mut map, &remaining);
    save_hot_rules(&map, previous_sources)
}

fn check_merlin_update(
    runtime: &CgiRuntime,
    asset: Option<&str>,
) -> Result<sockrocket_core::UpdateCheck> {
    runtime.block_on(sockrocket_core::check_for_update_with_proxy(
        env!("CARGO_PKG_VERSION"),
        asset,
        is_running().then(local_socks_addr).flatten(),
    ))
}

fn act_check_update() -> J {
    let prefer = merlin_update_asset();
    let rt = match CgiRuntime::new() {
        Ok(rt) => rt,
        Err(e) => return err_msg(format!("Runtime error: {e}")),
    };
    match check_merlin_update(&rt, prefer.as_deref()) {
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
        Err(e) => err_msg(format!("Update check failed: {e}")),
    }
}

/// Download the matching musl CLI binary from GitHub Releases and replace the
/// installed binary (config preserved). Restarts the service when it was running.
fn merlin_update_asset() -> Option<String> {
    let platform = fs::read_to_string(sockrocket_dir().join("platform"))
        .ok()
        .or_else(|| {
            Command::new("dbus")
                .args(["get", "softcenter_arch"])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        })?;
    sockrocket_core::merlin_package_asset(platform.trim())
}

// POSIX df -P keeps the available-block column stable on Merlin BusyBox.
fn ensure_update_space(path: &std::path::Path, required: u64) -> anyhow::Result<()> {
    let output = Command::new("df").arg("-Pk").arg(path).output()?;
    anyhow::ensure!(
        output.status.success(),
        "Cannot determine free space for {}",
        path.display()
    );
    let text = String::from_utf8_lossy(&output.stdout);
    let available = text
        .lines()
        .last()
        .and_then(|line| line.split_whitespace().nth(3))
        .and_then(|v| v.parse::<u64>().ok())
        .and_then(|v| v.checked_mul(1024))
        .ok_or_else(|| anyhow::anyhow!("Cannot parse free space for {}", path.display()))?;
    anyhow::ensure!(
        available >= required,
        "更新空间不足：{} 需要 {} KiB，可用 {} KiB，还差 {} KiB；现有安装未修改",
        path.display(),
        required.div_ceil(1024),
        available / 1024,
        required.saturating_sub(available).div_ceil(1024)
    );
    Ok(())
}

fn expanded_package_bytes(archive: &std::path::Path, members: usize) -> Result<u64> {
    // -O streams the regular files to stdout, without writing any package data.
    let mut tar = Command::new("tar")
        .arg("-xOzf")
        .arg(archive)
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    let mut payload = tar
        .stdout
        .take()
        .context("Cannot inspect expanded package")?;
    let bytes = std::io::copy(&mut payload, &mut std::io::sink());
    drop(payload);
    anyhow::ensure!(tar.wait()?.success(), "Cannot measure expanded package");
    // A partial last block per member also covers the directories on Merlin tmpfs.
    Ok(bytes?.saturating_add((members as u64).saturating_mul(4096)))
}

fn save_update_status(status: &str) -> Result<()> {
    sockrocket_core::config::storage::atomic_write_private(
        &sockrocket_dir().join("update-status"),
        status,
        false,
    )
}

fn update_lock() -> std::io::Result<fs::File> {
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(sockrocket_dir().join(".update-worker-lock"))
}

fn act_update_status() -> J {
    let root = sockrocket_dir();
    let mut active = root.join(".update-transaction").is_dir();
    let job_id = fs::read_to_string(root.join("update-job-id")).unwrap_or_default();
    let mut status =
        fs::read_to_string(root.join("update-status")).unwrap_or_else(|_| "idle".into());
    active |= status.trim() == "queued";
    if !job_id.is_empty()
        && matches!(
            status.trim(),
            "checking" | "downloading" | "verifying" | "queued" | "preparing"
        )
        && !root.join(".update-transaction").is_dir()
    {
        // The detached worker keeps the launch lock on stdin until the installer exits.
        // On reboot or worker exit the kernel releases it.
        if update_lock().is_ok_and(|lock| lock.try_lock_shared().is_ok()) {
            active = false;
            status = "failed: 更新进程已中断".into();
        } else {
            active = true;
        }
    }
    if status.starts_with("rollback-failed:") {
        active = false;
    }
    let stage = fs::read_to_string(root.join(".update-transaction/stage")).unwrap_or_default();
    json!({"ok":true,"active":active,"status":status.trim(),"stage":stage.trim(),"job_id":job_id.trim()})
}

fn act_apply_update(post: &J) -> J {
    let id = post
        .get("request_id")
        .and_then(J::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{:032x}", rand::random::<u128>()));
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return err_msg("Invalid update request ID");
    }
    let launch = (|| -> Result<J> {
        let root = sockrocket_dir();
        let id_path = root.join("update-job-id");
        if fs::read_to_string(&id_path).ok().as_deref() == Some(&id) {
            return Ok(json!({"ok":true,"job_id":id}));
        }
        let lock = update_lock()?;
        lock.try_lock()
            .map_err(|_| anyhow::anyhow!("已有软件更新任务正在运行"))?;
        anyhow::ensure!(
            !root.join(".update-transaction").exists(),
            "已有更新事务正在运行"
        );
        save_update_status("checking")?;
        sockrocket_core::config::storage::atomic_write_private(&id_path, &id, false)?;
        let child = Command::new(std::env::current_exe()?)
            .arg("--merlin-update-worker")
            .env("SOCKROCKET_DIR", &root)
            .stdin(lock)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match child {
            Ok(mut child) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(e) => {
                save_update_status("failed: 无法启动更新进程")?;
                return Err(e.into());
            }
        }
        Ok(json!({"ok":true,"job_id":id,"msg":"更新任务已启动"}))
    })();
    launch.unwrap_or_else(|e| err_msg(e.to_string()))
}

pub fn merlin_update_worker() -> Result<()> {
    let result = (|| -> Result<()> {
        let asset =
            merlin_update_asset().ok_or_else(|| anyhow::anyhow!("无法识别当前机型的软件包"))?;
        let rt = CgiRuntime::new()?;
        let check = check_merlin_update(&rt, Some(&asset))?;
        if check.availability != sockrocket_core::UpdateAvailability::Available {
            return save_update_status("up-to-date");
        }
        let url = check
            .asset_url
            .ok_or_else(|| anyhow::anyhow!("Release has no matching full plugin package"))?;
        let size = check.asset_size.filter(|s| *s > 0).ok_or_else(|| {
            anyhow::anyhow!("Release asset size unavailable; cannot preflight update space")
        })?;
        let temporary = std::env::temp_dir();
        let work = temporary.join(format!("sockrocket-update-{}", rand::random::<u64>()));
        fs::create_dir(&work)?;
        let prepare = (|| -> anyhow::Result<()> {
            let archive = work.join("package.tar.gz");
            ensure_update_space(&work, size)?;
            save_update_status("downloading")?;
            rt.block_on(sockrocket_core::download_file_with_proxy_fallback(
                &url,
                &archive,
                if is_running() {
                    local_socks_addr()
                } else {
                    None
                },
            ))?;
            save_update_status("verifying")?;
            let listing = Command::new("tar").arg("-tzvf").arg(&archive).output()?;
            anyhow::ensure!(listing.status.success(), "Invalid package archive");
            for line in String::from_utf8_lossy(&listing.stdout).lines() {
                anyhow::ensure!(
                    line.starts_with('-') || line.starts_with('d'),
                    "Package contains links or special files"
                );
            }
            let names = Command::new("tar").arg("-tzf").arg(&archive).output()?;
            anyhow::ensure!(names.status.success(), "Cannot inspect archive");
            for name in String::from_utf8_lossy(&names.stdout).lines() {
                anyhow::ensure!(
                    name.starts_with("sockrocket/") && !name.split('/').any(|p| p == ".."),
                    "Unsafe archive path"
                );
            }
            // df already accounts for the downloaded archive at this point.
            let members = String::from_utf8_lossy(&names.stdout).lines().count();
            ensure_update_space(&work, expanded_package_bytes(&archive, members)?)?;
            anyhow::ensure!(
                Command::new("tar")
                    .arg("-xzf")
                    .arg(&archive)
                    .arg("-C")
                    .arg(&work)
                    .status()?
                    .success(),
                "Extraction failed"
            );
            fs::remove_file(&archive).context("Cannot release temporary package space")?;
            let pkg = work.join("sockrocket");
            let platform = fs::read_to_string(pkg.join(".valid"))?;
            anyhow::ensure!(
                sockrocket_core::merlin_package_asset(platform.trim()).as_ref() == Some(&asset),
                "Platform mismatch"
            );
            let output = Command::new(pkg.join("bin/sockrocket-cli"))
                .arg("--version")
                .output()?;
            anyhow::ensure!(
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout)
                        .split_whitespace()
                        .any(|v| v.trim_start_matches('v') == check.latest.trim_start_matches('v')),
                "Executable version validation failed"
            );
            // Copy worker outside the plugin tree so its own replacement is safe.
            fs::write(
                work.join("worker.sh"),
                include_str!("../../../merlin/scripts/update.sh"),
            )?;
            // The worker checks flash headroom under the update lock before
            // copying its snapshot, then checks again once the backup is durable.
            save_update_status("queued")?;
            // Replace the old binary before installation so unlinking it frees flash.
            // Only the supervising shell retains the lock on stdin. The installer
            // and any services that it starts must not inherit that descriptor.
            let mut install = Command::new("sh");
            install
                .arg("-c")
                .arg(
                    r#"
                sh "$1" "$2" "$3" </dev/null
                rc=$?
                if [ "$rc" -ne 0 ]; then
                    case "$(cat "$SOCKROCKET_DIR/update-status")" in
                        failed:*|rolled-back:*|rollback-failed:*) ;;
                        *) echo 'failed: 安装未完成' > "$SOCKROCKET_DIR/update-status" ;;
                    esac
                fi
                exit "$rc"
            "#,
                )
                .arg("merlin-update")
                .arg(work.join("worker.sh"))
                .arg(&pkg)
                .arg(&work)
                .env("SOCKROCKET_DIR", sockrocket_dir());
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                Err(install.exec().into())
            }
            #[cfg(not(unix))]
            {
                anyhow::ensure!(install.status()?.success(), "安装未完成");
                Ok(())
            }
        })();
        if let Err(e) = prepare {
            let _ = fs::remove_dir_all(&work);
            return Err(e);
        }
        Ok(())
    })();
    if let Err(ref error) = result {
        let status = fs::read_to_string(sockrocket_dir().join("update-status")).unwrap_or_default();
        if !status.starts_with("rolled-back:")
            && !status.starts_with("rollback-failed:")
            && !status.starts_with("failed:")
        {
            save_update_status(&format!("failed: {error}"))?;
        }
    }
    result
}

fn act_diagnose() -> J {
    let running = is_running();
    let config_valid = load_app_config().is_ok();
    let ipt_count = run_cmd(
        "iptables",
        &["-t", "mangle", "-L", "SOCKROCKET_MANGLE", "-n"],
    )
    .map(|out| {
        out.lines()
            .filter(|line| line.split_whitespace().next() == Some("MARK"))
            .count()
    });
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
    let (fw_active, fw_ipv6) = firewall_hooks(run_cmd);
    let log = tail_lines(&log_path(), 500);
    let mut errors: Vec<_> = log
        .lines()
        .rev()
        .filter(|l| {
            let l = l.to_lowercase();
            l.contains("error") || l.contains("fail")
        })
        .take(10)
        .collect();
    errors.reverse();
    let last_errors = errors.join("\n");
    json!({
        "ok": true,
        "running": running,
        "config_valid": config_valid,
        "iptables_rules": ipt_count,
        "dns_rules": dns_rules,
        "dnsmasq_running": dnsmasq_running,
        "fw_active": fw_active,
        "fw_ipv6": fw_ipv6,
        "last_errors_base64": B64.encode(last_errors),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpack_space_counts_files_and_detects_tar_failure() {
        let env = TestEnv::new(SAMPLE);
        fs::write(env.dir.join("payload"), vec![7; 8193]).unwrap();
        let archive = env.dir.join("package.tar.gz");
        assert!(
            Command::new("tar")
                .arg("-czf")
                .arg(&archive)
                .arg("-C")
                .arg(&env.dir)
                .arg("payload")
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(expanded_package_bytes(&archive, 1).unwrap(), 8193 + 4096);
        fs::write(&archive, b"invalid compressed archive").unwrap();
        assert!(expanded_package_bytes(&archive, 1).is_err());
    }

    #[test]
    fn monitor_memory_sources() {
        let legacy = "MemTotal: 100000 kB\nMemFree: 10000 kB\nBuffers: 5000 kB\nCached: 15000 kB\n";
        assert_eq!(
            memory_usage(legacy),
            (Some(100000), Some(70000), "estimated")
        );
        assert_eq!(
            memory_usage(&format!("{legacy}MemAvailable: 0 kB")),
            (Some(100000), Some(100000), "measured")
        );
        assert_eq!(
            memory_usage("MemTotal: 128 kB"),
            (Some(128), None, "unknown")
        );
    }

    #[test]
    fn monitor_wan_failover_and_load_balancing() {
        let wan = |mode: &str| {
            wan_addresses(|cmd, args| match (cmd, args) {
                ("ip", ["-6", ..]) => Some("dev eth1 src 2001:db8::2".into()),
                ("nvram", ["get", key]) => Some(
                    match *key {
                        "wans_mode" => mode,
                        "wan1_primary" => "1",
                        "wan0_state_t" | "wan1_state_t" => "2",
                        "wan0_ipaddr" => "192.0.2.3",
                        "wan1_ipaddr" => "192.0.2.4",
                        _ => "",
                    }
                    .into(),
                ),
                _ => None,
            })
        };
        assert_eq!(wan("fo"), ["192.0.2.4", "2001:db8::2"]);
        assert_eq!(wan("lb"), ["192.0.2.3", "192.0.2.4", "2001:db8::2"]);
        assert_eq!(
            wan_addresses(|_, args| match args {
                ["-6", ..] => Some("dev eth1 src 2001:db8::3".into()),
                _ => None,
            }),
            ["2001:db8::3"]
        );
    }

    #[test]
    fn monitor_firewall_distinguishes_unavailable_from_unmounted() {
        assert_eq!(
            firewall_hooks(|cmd, _| (cmd == "iptables")
                .then(|| "SOCKROCKET_MANGLE all -- anywhere anywhere".into())),
            (Some(true), None)
        );
        assert_eq!(
            firewall_hooks(|_, _| Some("Chain PREROUTING (policy ACCEPT)".into())),
            (Some(false), Some(false))
        );
    }

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
    fn default_groups_save_independently_and_route_after_reload() {
        let _env = TestEnv::new(&SAMPLE.replace("rules: []", "cn_ipset_direct: true\nrules:\n  - {rule_type: final, pattern: '*', target: direct, group: Original}\n  - {rule_type: final, pattern: '*', target: reject, enabled: false, group: Backup}"));
        let before = act_get_rules();
        assert_eq!(before["builtin_route"], "direct");
        let save = |action, body: J| dispatch(action, &body);
        let revision = || route_trace::rules_revision(&load_conf().unwrap());
        let trace = |host| {
            dispatch(
                "trace_route",
                &json!({"destination":host,"resolve_dns":false}),
            )["trace"]["action"]
                .clone()
        };
        assert_eq!(
            save(
                "save_builtin_route",
                json!({"target":"reject","revision":revision()})
            )["ok"],
            true
        );
        assert_eq!(
            save(
                "save_builtin_route",
                json!({"target":"direct","revision":before["revision"]})
            )["ok"],
            false
        );
        assert_eq!(act_get_rules()["builtin_route"], "reject");
        assert_eq!(act_get_rules()["rules"], before["rules"]);
        assert!(!conf_bool(&load_conf().unwrap(), "cn_ipset_direct", true));
        assert_eq!(trace("baidu.com"), "reject");
        assert_eq!(trace("192.0.2.1"), "direct");
        assert_eq!(
            save(
                "save_device_policy",
                json!({"target":"proxy","all_devices":true,"revision":revision()})
            )["ok"],
            true
        );
        assert_eq!(trace("192.0.2.1"), "proxy");
        assert_eq!(trace("baidu.com"), "reject");
        let after = act_get_rules();
        assert_eq!(after["rules"][0]["group"], "Original");
        assert_eq!(after["rules"][1], before["rules"][1]);
    }

    #[test]
    fn saving_fallback_requires_an_explicit_scope() {
        let env = TestEnv::new(SAMPLE);
        let revision = route_trace::rules_revision(&load_conf().unwrap());
        let before = fs::read(env.dir.join("config.yaml")).unwrap();
        for post in [
            json!({"target":"reject","revision":revision}),
            json!({"source":"","all_devices":false,"target":"reject","revision":revision}),
        ] {
            assert_eq!(dispatch("save_device_policy", &post)["ok"], false);
            assert_eq!(fs::read(conf_path()).unwrap(), before);
        }
        assert_eq!(
            dispatch(
                "save_device_policy",
                &json!({"source":"","all_devices":true,
            "target":"reject","revision":revision})
            )["ok"],
            true
        );
        let rules = load_routing_rules(&load_conf().unwrap());
        assert_eq!(rules[0].name, "全局设备兜底");
        assert!(rules[0].group.is_empty());
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
    fn update_status_tracks_each_preinstall_phase_and_worker_exit() {
        let env = TestEnv::new(SAMPLE);
        let id = "00112233445566778899aabbccddeeff";
        let lock = update_lock().unwrap();
        lock.try_lock().unwrap();
        fs::write(env.dir.join("update-job-id"), id).unwrap();
        for phase in ["checking", "downloading", "verifying", "queued"] {
            save_update_status(phase).unwrap();
            let status = act_update_status();
            assert_eq!(status["active"], true);
            assert_eq!(status["job_id"], id);
            assert_eq!(status["status"], phase);
        }
        assert_eq!(act_apply_update(&json!({"request_id":id}))["job_id"], id);
        assert_eq!(act_apply_update(&json!({}))["ok"], false);
        drop(lock);
        let interrupted = act_update_status();
        assert_eq!(interrupted["active"], false);
        assert!(
            interrupted["status"]
                .as_str()
                .unwrap()
                .starts_with("failed:")
        );
    }

    #[test]
    fn update_blocks_configuration_writes_and_reports_terminal_states() {
        let _env = TestEnv::new(SAMPLE);
        let root = sockrocket_dir();
        fs::create_dir(root.join(".update-transaction")).unwrap();
        fs::write(root.join("update-status"), "preparing").unwrap();
        assert!(storage::MutationGuard::acquire().is_err());
        let response = dispatch("save_config", &json!({}));
        assert_eq!(response["error_code"], "update_in_progress");
        assert_eq!(dispatch("update_status", &json!({}))["active"], true);
        fs::write(
            root.join("update-status"),
            "rollback-failed: backup retained",
        )
        .unwrap();
        assert_eq!(dispatch("update_status", &json!({}))["active"], false);
        assert_eq!(
            dispatch("save_config", &json!({}))["error_code"],
            "update_recovery_required"
        );
        fs::remove_dir(root.join(".update-transaction")).unwrap();
        assert!(storage::MutationGuard::acquire().is_ok());
        fs::write(root.join("update-status"), "success: plugin updated").unwrap();
        assert_eq!(
            dispatch("update_status", &json!({}))["status"],
            "success: plugin updated"
        );
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
        assert_eq!(result["active_node"], act_status(None)["active_node"]);
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
        let s = act_status(None);
        assert_eq!(s["running"], J::Bool(false));
        assert_eq!(s["active_node"], J::from(0));
        assert_eq!(s["node_count"], J::from(2));
        assert_eq!(s["sub_count"], J::from(1));
        assert_eq!(s["mode"], J::from("tun"));
        assert_eq!(s["socks_port"], J::from("1080"));
    }

    #[test]
    fn stats_scopes_match_their_legacy_fields() {
        let env = TestEnv::new(SAMPLE);
        let month = sockrocket_core::recent_month_keys(1).pop().unwrap();
        fs::write(
            env.dir.join("traffic.yaml"),
            format!(
                "session_started: 42\nsession:\n  direct: {{name: LAN, up: 10, down: 20}}\nhistory:\n  direct:\n    name: LAN\n    months:\n      {month}: {{up: 13, down: 21}}\n"
            ),
        )
        .unwrap();
        let legacy = dispatch("stats", &J::Null);
        for key in [
            "ok",
            "pid",
            "uptime",
            "dns_active",
            "fw_active",
            "fw_ipv6",
            "traffic",
        ] {
            assert!(legacy.get(key).is_some(), "missing legacy field {key}");
        }
        let traffic = &legacy["traffic"];
        assert_eq!(traffic["session_started"], 42);
        assert_eq!(traffic["nodes"][0]["session_total"], 30);
        assert_eq!(traffic["recent_months"][0]["total"], 34);
        assert_eq!(
            dispatch("stats", &json!({"traffic_scope":"future"})),
            legacy
        );
        let scope = |value| dispatch("stats", &json!({"traffic_scope":value}));
        let uptime = &legacy["uptime"];
        assert_eq!(scope("none"), json!({"ok":true,"uptime":uptime}));
        assert_eq!(
            scope("session"),
            json!({"ok":true,"uptime":uptime,"traffic":{"nodes":traffic["nodes"]}})
        );
        assert_eq!(
            scope("monthly"),
            json!({"ok":true,"uptime":uptime,"traffic":{
                "recent_month_keys":traffic["recent_month_keys"],
                "recent_months":traffic["recent_months"],
                "recent_subscriptions":traffic["recent_subscriptions"]
            }})
        );
    }

    #[test]
    fn status_revision_scope_preserves_legacy_snapshots() {
        let _env = TestEnv::new(SAMPLE);
        let full = dispatch("status", &J::Null);
        assert!(full["rules_revision"].is_string() && full["network_revision"].is_string());
        for (scope, rules, network) in [
            ("rules", true, false),
            ("network", false, true),
            ("none", false, false),
            ("unrecognized", true, true),
        ] {
            let status = dispatch("status", &json!({ "revision_scope": scope }));
            let mut expected = full.clone();
            if !rules {
                expected["rules_revision"] = J::Null;
            }
            if !network {
                expected["network_revision"] = J::Null;
            }
            assert_eq!(status, expected, "{scope}");
        }
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
        let d = dispatch("node_details", &J::Null);
        let nodes = d["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0]["name"], J::from("Node A"));
        assert_eq!(nodes[0]["protocol"], J::from("vless"));
        assert_eq!(nodes[0]["server"], J::from("a.example.com"));
        assert_eq!(nodes[0]["port"], J::from(443));
        assert_eq!(nodes[1]["protocol"], J::from("hysteria2"));
        let names = dispatch("nodes", &J::Null);
        assert_eq!(names["active"], d["active"]);
        assert_eq!(names["nodes"], json!([nodes[0]["name"], nodes[1]["name"]]));
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
    fn fresh_merlin_global_routes_follow_selected_node() {
        let _env = TestEnv::new(include_str!("../../../merlin/config.yaml.template"));
        let add_node = |name| {
            dispatch(
                "add_node",
                &json!({"name":name,"server":"server.example.invalid","port":443,"protocol":"vless","uuid":"00000000-0000-4000-8000-000000000001"}),
            )
        };
        let trace = |host| {
            dispatch(
                "trace_route",
                &json!({"destination":host,"resolve_dns":false}),
            )
        };
        assert_eq!(add_node("First")["ok"], true);
        assert_eq!(dispatch("set_node", &json!({"index":0}))["ok"], true);
        let before = act_get_rules();
        let links = before["global_rule_groups"].as_array().unwrap();
        assert_eq!(
            links
                .iter()
                .map(|b| b["group"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["内网直连", "GEO 直连", "CN 直连"]
        );
        for (host, group) in [
            ("192.168.1.2", "内网直连"),
            ("223.5.5.5", "GEO 直连"),
            ("sample.cn", "CN 直连"),
        ] {
            let decision = trace(host);
            assert_eq!(decision["trace"]["action"], "direct");
            let index = decision["trace"]["matched_rule"].as_u64().unwrap() as usize;
            assert_eq!(before["rules"][index]["group"], group);
        }
        let destination = "203.0.113.44";
        let first = trace(destination);
        assert_eq!(first["trace"]["action"], "proxy");
        assert_eq!(first["node_name"], "First");
        assert_eq!(
            before["rules"][first["trace"]["matched_rule"].as_u64().unwrap() as usize]["name"],
            "全局设备兜底"
        );
        assert_eq!(add_node("Second")["ok"], true);
        assert_eq!(dispatch("set_node", &json!({"index":1}))["ok"], true);
        assert_eq!(trace(destination)["node_name"], "Second");
        let after = act_get_rules();
        assert_eq!(before["active_node_name"], "First");
        assert_eq!(after["active_node_name"], "Second");
        assert_eq!(before["rules"], after["rules"]);
        assert_eq!(before["global_rule_groups"], after["global_rule_groups"]);
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
    fn subscription_list_counts_nodes_shared_across_sources() {
        let _env = TestEnv::new(SAMPLE);
        let mut config = load_app_config().unwrap();
        let first = config.subscriptions[0].url.clone();
        let second = "https://example.invalid/second".to_string();
        config.subscriptions.push(sockrocket_core::Subscription {
            name: "Sub 2".into(),
            url: second.clone(),
            format: "auto".into(),
            last_updated: None,
            refresh_interval_hours: 24,
            refresh_revision: String::new(),
        });
        let node = |server| {
            sockrocket_core::parse_proxy_uri(&format!(
                "ss://aes-128-gcm:synthetic@{server}:8388#synthetic"
            ))
            .unwrap()
        };
        sockrocket_core::apply_subscription_refresh(
            &mut config,
            vec![
                (first, vec![node("shared.invalid"), node("first.invalid")]),
                (second, vec![node("shared.invalid"), node("second.invalid")]),
            ],
        );
        persist_subscription_state(&config).unwrap();
        let result = act_subscriptions();
        assert_eq!(result["subs"][0]["node_count"], 2);
        assert_eq!(result["subs"][1]["node_count"], 2);
        assert_eq!(result["shared_nodes"], 1);
        assert_eq!(load_app_config().unwrap().nodes.len(), 5);
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
        let d = act_nodes(true);
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
    fn stale_node_actions_cannot_retarget_or_move_the_active_exit() {
        let _env = TestEnv::new(SAMPLE);
        let mut map = load_conf().unwrap();
        let nodes = map
            .get_mut(ykey("nodes"))
            .unwrap()
            .as_sequence_mut()
            .unwrap();
        let mut third = nodes[0].clone();
        third["name"] = Y::String("Node C".into());
        nodes.push(third);
        set_active_node(&mut map, 1);
        save_conf(&map).unwrap();

        let id = act_nodes(true)["nodes"][0]["test_id"].clone();
        let row = json!({"index": 0, "expected_id": id});
        assert_eq!(dispatch("del_node", &row)["ok"], true);
        let updated = act_nodes(true);
        assert_eq!(updated["active"], 0);
        assert_eq!(updated["nodes"][0]["name"], "Node B");
        let before = fs::read(conf_path()).unwrap();
        for action in ["del_node", "set_node"] {
            assert_eq!(dispatch(action, &row)["ok"], false);
            assert_eq!(dispatch(action, &json!({"index": 8}))["ok"], false);
        }
        assert_eq!(fs::read(conf_path()).unwrap(), before);
        for _ in 0..2 {
            assert_eq!(dispatch("del_node", &json!({"index":0}))["ok"], true);
        }
        assert_eq!(dispatch("set_node", &json!({"index":0}))["ok"], false);
        assert!(conf_usize(&load_conf().unwrap(), "active_node").is_none());

        fs::write(conf_path(), "nodes: {0: malformed}").unwrap();
        for action in ["set_node", "del_node"] {
            assert_eq!(dispatch(action, &json!({"index":0}))["ok"], false);
        }
    }

    #[test]
    fn stale_subscription_actions_leave_the_replacement_untouched() {
        let _env = TestEnv::new(SAMPLE);
        let current = act_subscriptions()["subs"][0]["id"].clone();
        let old = json!({"index":0,"expected_id":"removed","name":"Renamed"});
        let before = fs::read(conf_path()).unwrap();
        for action in ["rename_sub", "del_sub"] {
            assert_eq!(dispatch(action, &old)["ok"], false);
        }
        assert_eq!(fs::read(conf_path()).unwrap(), before);
        assert_eq!(
            dispatch(
                "rename_sub",
                &json!({"index":0,"expected_id":current,"name":"Renamed"})
            )["ok"],
            true
        );
        assert_eq!(
            dispatch("del_sub", &json!({"index":0,"expected_id":current}))["ok"],
            true
        );
    }

    #[test]
    fn advanced_config_preserves_newer_edits_and_reports_unreadable_files() {
        let _env = TestEnv::new(SAMPLE);
        let first = dispatch("config", &J::Null);
        let draft = json!({"config":first["config_base64"],"expected_revision":first["revision"]});
        let unchanged = dispatch("save_config", &draft);
        assert_eq!(unchanged["ok"], true);
        assert!(unchanged.get("saved").is_none());
        let replacement = B64.encode(format!("{SAMPLE}\n# keep comment\n"));
        let saved = dispatch(
            "save_config",
            &json!({"config":replacement,"expected_revision":first["revision"]}),
        );
        assert_eq!(saved["saved"], true);
        assert_ne!(saved["revision"], first["revision"]);
        assert_eq!(dispatch("save_config", &draft)["ok"], false);
        assert_eq!(dispatch("config", &J::Null)["config_base64"], replacement);
        fs::remove_file(conf_path()).unwrap();
        for action in ["config", "nodes", "node_details", "subscriptions"] {
            assert_eq!(dispatch(action, &J::Null)["ok"], false, "{action}");
        }
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
        assert_eq!(snapshot["revision"], act_status(None)["rules_revision"]);
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
    fn deleting_device_preserves_shared_and_other_device_policies() {
        let _env=TestEnv::new(&SAMPLE.replace("rules: []","rules:\n  - {rule_type: domain, group: Shared, pattern: example.com, target: direct}\n  - {rule_type: final, source: 192.0.2.8, pattern: '*', target: reject}\n  - {rule_type: final, source: 192.0.2.9, pattern: '*', target: direct}"));
        let revision = || act_get_rules()["revision"].clone();
        assert_eq!(
            dispatch(
                "save_device_alias",
                &json!({"source":"192.0.2.8","name":"TV","previous":""})
            )["ok"],
            true
        );
        assert_eq!(
            dispatch(
                "save_device_binding",
                &json!({"source":"192.0.2.8","groups":["Shared"],"target":"proxy","revision":revision()})
            )["ok"],
            true
        );
        let before = revision();
        assert_eq!(
            dispatch("delete_device", &json!({"source":"","revision":before}))["ok"],
            false
        );
        assert_eq!(
            dispatch(
                "delete_device",
                &json!({"source":"192.0.2.8","revision":"stale"})
            )["ok"],
            false
        );
        assert_eq!(
            dispatch(
                "delete_device",
                &json!({"source":"192.0.2.8","revision":before})
            )["ok"],
            true
        );
        let snapshot = act_get_rules();
        assert_eq!(snapshot["rules"].as_array().unwrap().len(), 2);
        assert!(
            snapshot["device_rule_groups"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(
            !snapshot["devices"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["alias"] == "TV")
        );
        assert_eq!(
            dispatch(
                "save_device_alias",
                &json!({"source":"192.0.2.8","name":"New TV","previous":""})
            )["ok"],
            true
        );
    }

    #[test]
    fn emby_presets_import_exact_domains_without_activating() {
        let _env = TestEnv::new(SAMPLE);
        for (id, count, target) in [
            ("emby_jp", 70, "proxy"),
            ("emby_us", 43, "proxy"),
            ("emby_direct", 80, "direct"),
        ] {
            let preset = rule_presets::catalog()
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["id"] == id)
                .unwrap()
                .clone();
            assert_eq!(preset["rule_type"], "domain");
            assert_eq!(preset["domains"].as_array().unwrap().len(), count);
            let add = || {
                dispatch(
                    "add_rule_preset",
                    &json!({"preset":id,"expected_revision":act_get_rules()["revision"]}),
                )
            };
            let saved = add();
            assert_eq!(saved["ok"], true);
            assert!(saved["added"].as_u64().unwrap() > 0);
            let rules = load_routing_rules(&load_conf().unwrap());
            assert!(
                rules
                    .iter()
                    .filter(|r| r.group == preset["group"].as_str().unwrap())
                    .all(|r| r.rule_type == "domain" && !r.enabled && r.target == target)
            );
            assert_eq!(add()["added"], 0);
        }
    }

    #[test]
    fn preset_can_be_added_to_selected_group() {
        let _env = TestEnv::new(&SAMPLE);
        fs::write(conf_path().parent().unwrap().join("stopped"), "").unwrap();
        let add = || {
            dispatch(
                "add_rule_preset",
                &json!({"preset":"ai", "group":"My AI", "expected_revision":act_get_rules()["revision"]}),
            )
        };
        assert_eq!(add()["added"], 9);
        let rules = load_routing_rules(&load_conf().unwrap());
        assert!(rules.iter().all(|r| r.group == "My AI" && !r.enabled));
        assert_eq!(add()["added"], 0);
    }

    #[test]
    fn rule_presets_are_opt_in_idempotent_and_preserve_custom_rules() {
        let env=TestEnv::new(&SAMPLE.replace("rules: []","rules:\n  - {rule_type: domain-suffix, pattern: '.OPENAI.COM', target: reject, priority: 99, enabled: false, group: Custom}\n  - {rule_type: domain-suffix, pattern: chatgpt.com, target: direct, source: 192.0.2.8, priority: 10}"));
        fs::write(env.dir.join("stopped"), "").unwrap();
        let before = fs::read(conf_path()).unwrap();
        let catalog = dispatch("get_rules", &json!({}));
        assert_eq!(catalog["presets"].as_array().unwrap().len(), 20);
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

            let result = dispatch(
                "add_rule_preset",
                &json!({"preset":p["id"],"expected_revision":route_trace::rules_revision(&load_conf().unwrap())}),
            );
            assert_eq!(result["ok"], true, "{result}");
            assert_eq!(
                result["added"].as_u64().unwrap() + result["skipped"].as_u64().unwrap(),
                p["domains"].as_array().unwrap().len() as u64
            );
            count += result["added"].as_u64().unwrap() as usize;
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
    fn imported_group_is_inactive_until_a_source_binds_it() {
        let _env = TestEnv::new(&SAMPLE.replace("rules: []","rules:\n  - {rule_type: domain, group: Legacy, pattern: video.invalid, target: direct}"));
        let winner = |source: &str| {
            dispatch(
                "trace_route",
                &json!({"destination":"video.invalid","source":source,"resolve_dns":false}),
            )["trace"]["action"]
                .clone()
        };
        let bind = |source: &str, target: &str, remove: bool| {
            let revision = act_get_rules()["revision"].clone();
            dispatch(
                "save_device_binding",
                &json!({"revision":revision,"source":source,"all_devices":source.is_empty(),"group":"Imported","target":target,"priority":10,"remove":remove}),
            )
        };
        assert_eq!(act_get_rules()["global_rule_groups"][0]["group"], "Legacy");
        assert_eq!(
            dispatch(
                "batch_add_rules",
                &json!({"text":"DOMAIN,video.invalid,REJECT","group_only":true,"group":"Imported"})
            )["ok"],
            true
        );
        assert_eq!(act_get_rules()["rules"].as_array().unwrap().len(), 2);
        assert_eq!(winner(""), "direct");
        assert_eq!(bind("192.0.2.8", "reject", false)["ok"], true);
        assert_eq!(winner("192.0.2.8"), "reject");
        assert_eq!(winner("192.0.2.9"), "direct");
        assert_eq!(bind("", "proxy", false)["ok"], true);
        assert_eq!(winner("192.0.2.8"), "reject");
        assert_eq!(winner("192.0.2.9"), "proxy");
        assert_eq!(bind("", "", true)["ok"], true);
        assert_eq!(winner("192.0.2.9"), "direct");
        assert_eq!(winner("192.0.2.8"), "reject");
    }

    #[test]
    fn device_binding_priority_changes_the_winner_only_for_that_source() {
        let _env = TestEnv::new(&SAMPLE.replace(
            "rules: []",
            "rules:\n  - {rule_type: domain, group: Media, pattern: tv.invalid, target: direct, priority: 100}\n  - {rule_type: domain, group: Japan, pattern: tv.invalid, target: proxy}",
        ));
        let save = |source: &str, changes: J| {
            let mut body = changes;
            body["source"] = json!(source);
            body["revision"] = json!(route_trace::rules_revision(&load_conf().unwrap()));
            dispatch("save_device_binding", &body)
        };
        let tv = "192.0.2.8";
        let laptop = "192.0.2.9";
        for src in [tv, laptop] {
            assert_eq!(
                save(src, json!({"groups":["Media","Japan"],"target":"direct"}))["ok"],
                true
            );
        }
        let winner = |source: &str| {
            let trace = dispatch(
                "trace_route",
                &json!({"source":source,"destination":"tv.invalid","resolve_dns":false}),
            );
            (
                trace["trace"]["matched_rule"].clone(),
                trace["trace"]["action"].clone(),
            )
        };
        assert_eq!(winner(tv), (json!(0), json!("direct")));
        assert_eq!(
            save(tv, json!({"group":"Japan","priority":20,"target":"reject"}))["ok"],
            true
        );
        assert_eq!(winner(tv), (json!(1), json!("reject")));
        assert_eq!(winner(laptop), (json!(0), json!("direct")));
        assert_eq!(winner(""), (json!(0), json!("direct")));
        // Older clients can update the outlet without resetting the saved priority.
        assert_eq!(
            save(tv, json!({"group":"Japan","target":"proxy"}))["ok"],
            true
        );
        assert_eq!(winner(tv), (json!(1), json!("proxy")));
        for invalid in [json!(-1), json!(65536), json!(1.5)] {
            assert_eq!(
                save(tv, json!({"group":"Japan","target":"","priority":invalid}))["ok"],
                false
            );
        }
        assert_eq!(
            save(tv, json!({"group":"Japan","target":"proxy","priority":0}))["ok"],
            true
        );
        assert_eq!(winner(tv), (json!(0), json!("direct")));
    }

    #[test]
    fn group_editor_preserves_device_and_global_fallbacks() {
        let _env = TestEnv::new(&SAMPLE.replace(
            "rules: []",
            "rules:\n  - {rule_type: domain, group: Mixed, pattern: tv.invalid, target: proxy}\n  - {rule_type: final, group: Mixed, pattern: '*', target: direct}\n  - {rule_type: MATCH, group: Mixed, source: 192.0.2.8/32, pattern: '*', target: reject}",
        ));
        let fallbacks = || {
            load_routing_rules(&load_conf().unwrap())
                .into_iter()
                .filter(|r| rule_kind(&r.rule_type) == "final")
                .map(|r| rule_edit_snapshot(&r))
                .collect::<Vec<_>>()
        };
        let original = fallbacks();
        assert_eq!(original.len(), 2);
        let mut request = json!({"group":"Mixed","destinations_only":true,"enabled":false});
        assert_eq!(act_set_rule_group_enabled(&request)["ok"], true);
        request["text"] = json!("MATCH,PROXY");
        assert_eq!(act_batch_add_rules(&request)["ok"], false);
        assert_eq!(act_replace_rule_group(&request)["ok"], false);
        request["text"] = json!("DOMAIN,new.invalid,DIRECT");
        assert_eq!(act_replace_rule_group(&request)["ok"], true);
        request["expected_revision"] = json!(route_trace::rules_revision(&load_conf().unwrap()));
        assert_eq!(act_delete_rule_group(&request)["ok"], true);
        assert_eq!(fallbacks(), original);
    }

    #[test]
    fn deleting_a_group_only_removes_confirmed_rules_and_links() {
        let _env = TestEnv::new(&SAMPLE.replace(
            "rules: []",
            "rules:\n  - {rule_type: domain, group: Video, pattern: tv.invalid, target: proxy}\n  - {rule_type: domain, pattern: stay.invalid, target: direct, custom_key: preserved}",
        ));
        let revision = || route_trace::rules_revision(&load_conf().unwrap());
        let old = revision();
        assert_eq!(
            dispatch(
                "save_device_binding",
                &json!({"revision":old,"group":"Video","source":"192.0.2.8","target":"direct"})
            )["ok"],
            true
        );
        let before = fs::read(conf_path()).unwrap();
        for request in [
            json!({"group":"Video","expected_revision":old}),
            json!({"expected_revision":revision()}),
            json!({"group":"Video"}),
        ] {
            assert_eq!(dispatch("delete_rule_group", &request)["ok"], false);
            assert_eq!(fs::read(conf_path()).unwrap(), before);
        }
        assert_eq!(
            dispatch(
                "delete_rule_group",
                &json!({"group":"Video","expected_revision":revision()})
            )["ok"],
            true
        );
        let map = load_conf().unwrap();
        assert!(devices::links(&map).unwrap().is_empty());
        let rows = conf_seq(&map, "rules");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["custom_key"].as_str(), Some("preserved"));
        assert_eq!(
            dispatch(
                "delete_rule_group",
                &json!({"group":"","expected_revision":revision()})
            )["ok"],
            true
        );
    }

    #[test]
    fn device_bindings_follow_edits_and_aliases_are_independent_of_routing() {
        let _env = TestEnv::new(&SAMPLE.replace(
            "rules: []",
            "rules:\n  - {rule_type: domain, group: Video, pattern: tv.invalid, target: direct}",
        ));
        let revision = || route_trace::rules_revision(&load_conf().unwrap());
        let trace = |ip: &str, host: &str| {
            dispatch(
                "trace_route",
                &json!({"source":ip,"destination":host,"resolve_dns":false}),
            )["trace"]["action"]
                .clone()
        };
        assert_eq!(act_get_rules()["compiled_targets"], json!(["direct"]));
        let original = revision();
        let bind = dispatch(
            "save_device_binding",
            &json!({"revision":original,"source":"192.0.2.8","group":"Video","target":"reject"}),
        );
        assert_eq!(bind["ok"], true, "{bind}");
        assert_eq!(load_routing_rules(&load_conf().unwrap()).len(), 1);
        assert_eq!(trace("192.0.2.8", "tv.invalid"), "reject");
        assert_eq!(trace("192.0.2.9", "tv.invalid"), "direct");
        let stale = dispatch(
            "save_device_binding",
            &json!({"revision":original,"source":"192.0.2.8","group":"Video","target":"","remove":true}),
        );
        assert_eq!(stale["ok"], false);
        let added = dispatch(
            "batch_add_rules",
            &json!({"group":"Video","text":"DOMAIN,new.invalid,DIRECT","target":"proxy"}),
        );
        assert_eq!(added["ok"], true, "{added}");
        assert_eq!(trace("192.0.2.8", "new.invalid"), "reject");
        for (group, host) in [("Music", "music.invalid"), ("Work", "work.invalid")] {
            let added = dispatch(
                "batch_add_rules",
                &json!({"group":group,"text":format!("DOMAIN,{host},DIRECT"),"target":"proxy"}),
            );
            assert_eq!(added["ok"], true, "{added}");
        }
        let add_groups = |groups: &[&str]| {
            dispatch(
                "save_device_binding",
                &json!({"revision":revision(),"source":"192.0.2.8","groups":groups,"target":"direct"}),
            )
        };
        assert_eq!(add_groups(&["Music", "Missing"])["ok"], false);
        assert_eq!(
            act_get_rules()["device_rule_groups"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(add_groups(&["Music", "Work", "Video", "Music"])["ok"], true);
        assert_eq!(
            act_get_rules()["device_rule_groups"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        let changed = dispatch(
            "save_device_binding",
            &json!({"revision":revision(),"source":"192.0.2.8","group":"Music","target":"proxy"}),
        );
        assert_eq!(changed["ok"], true);
        assert_eq!(trace("192.0.2.8", "tv.invalid"), "reject");
        assert_eq!(trace("192.0.2.8", "music.invalid"), "proxy");
        assert_eq!(trace("192.0.2.8", "work.invalid"), "direct");
        let saved_config = fs::read(conf_path()).unwrap();
        let alias = json!({"source":"192.0.2.8","key":"mac:02:00:00:00:00:01","name":"客厅电视","previous":""});
        assert_eq!(dispatch("save_device_alias", &alias)["ok"], true);
        assert_eq!(
            dispatch("save_device_alias", &alias)["ok"],
            false,
            "stale rename must not overwrite"
        );
        assert_eq!(fs::read(conf_path()).unwrap(), saved_config);
        assert!(
            act_get_rules()["devices"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["alias"] == "客厅电视")
        );
        let removed = dispatch(
            "save_device_binding",
            &json!({"revision":revision(),"source":"192.0.2.8","group":"Video","target":"","remove":true}),
        );
        assert_eq!(removed["ok"], true);
        assert_eq!(trace("192.0.2.8", "new.invalid"), "proxy");
        assert_eq!(
            act_get_rules()["device_rule_groups"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(trace("192.0.2.8", "music.invalid"), "proxy");
        for group in ["Music", "Work"] {
            let removed = dispatch(
                "save_device_binding",
                &json!({"revision":revision(),"source":"192.0.2.8","group":group,"target":"","remove":true}),
            );
            assert_eq!(removed["ok"], true);
        }
        assert_eq!(trace("192.0.2.8", "new.invalid"), "direct");
    }

    #[test]
    fn rule_link_preview_is_read_only_and_import_stays_in_selected_group() {
        use std::io::Write;
        let _env = TestEnv::new(SAMPLE);
        assert!(read_only_action("fetch_rule_list"));
        let before = fs::read(conf_path()).unwrap();
        for url in ["file:///etc/passwd", "https://"] {
            assert_eq!(
                dispatch("fetch_rule_list", &json!({"url":url}))["ok"],
                false
            );
            assert_eq!(fs::read(conf_path()).unwrap(), before);
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request).unwrap();
            let body = "# > test\nDOMAIN,example.invalid\n# @group Foreign\nDOMAIN,example.invalid\nGEOIP,CN,DIRECT";
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let target = format!("node:{}", "a".repeat(64));
        let preview = dispatch(
            "fetch_rule_list",
            &json!({"url":format!("http://{address}/rules.list"),"target":target,"group":"Emby"}),
        );
        server.join().unwrap();
        assert_eq!(preview["ok"], true, "{preview}");
        assert_eq!(preview["count"], 3);
        assert_eq!(
            fs::read(conf_path()).unwrap(),
            before,
            "reading must not save or restart"
        );
        let text = format!("# @group Foreign\n{}", preview["text"].as_str().unwrap());
        let added = dispatch(
            "batch_add_rules",
            &json!({"group":"Emby","text":text,"target":target,"group_only":true,"replace":false}),
        );
        assert_eq!(added["ok"], true, "{added}");
        let rows = act_get_rules()["rules"].as_array().unwrap().clone();
        assert_eq!(rows.len(), 2, "duplicates are skipped");
        assert!(rows.iter().all(|r| r["group"] == "Emby"));
        assert_eq!(rows[0]["target"], target);
        assert_eq!(rows[1]["target"], "direct");
        let saved = fs::read(conf_path()).unwrap();
        assert_eq!(
            dispatch(
                "batch_add_rules",
                &json!({"group":"Emby","text":text,"target":target,"group_only":true,"replace":false})
            )["ok"],
            true
        );
        assert_eq!(
            fs::read(conf_path()).unwrap(),
            saved,
            "repeated import must preserve existing rules"
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

        let legacy_socks = B64.encode(format!(
            "{}\ndns:\n  strategy: ipv6_only\nipv6_mode: compatibility\n",
            SAMPLE.replace("mode: \"tun\"", "mode: \"socks\"")
        ));
        let r = act_save_config(&json!({"config": legacy_socks}));
        assert_eq!(
            r["ok"],
            J::Bool(true),
            "legacy SOCKS mode remains valid: {r}"
        );

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
        let d = act_nodes(true);
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
        let s = act_status(None);
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
        let s = act_status(None);
        assert_eq!(s["proxy_mode"], J::from("global"), "{s}");
    }

    #[test]
    fn status_derives_toggles_from_legacy_mode() {
        let _env = TestEnv::new(&SAMPLE.replace("mode: \"tun\"", "mode: \"socks\""));
        let s = act_status(None);
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
        let s = act_status(None);
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
    fn stale_toggle_cannot_enable_cn_bypass_after_switching_to_global() {
        let _env = TestEnv::new(SAMPLE);
        assert_eq!(
            act_set_proxy_mode(&json!({"proxy_mode":"global"}))["ok"],
            true
        );
        let before = load_conf().unwrap();
        let reply = act_set_toggles(&json!({"cn_ipset_direct":true,"transparent_proxy":false}));
        assert_eq!(reply["ok"], false);
        assert_eq!(load_conf().unwrap(), before);
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

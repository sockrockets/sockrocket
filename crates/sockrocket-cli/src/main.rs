mod cgi;

use std::env;
use std::fs;
use std::net::IpAddr;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use sockrocket_core::{
    AppConfig, ConfigWatchEvent, ConfigWatcher, DnsResolver, HealthCheckSetup, HealthEvent,
    OutboundFactory, ProxyProtocol, ProxyService, Router, RoutingOutbound, RoutingRule, RuleSet,
    SharedOutbound, SwappableOutbound, TunProxy, create_outbound, fetch_subscription,
    normalize_node_names, resolve_to_ips, setup_tun_routes, tun_requirements, tun_supported,
};

fn main() -> Result<()> {
    // CGI mode: invoked as `sockrocket.cgi` by the router httpd (AsusWRT-Merlin web
    // UI backend), or explicitly as `sockrocket-cli cgi`. Synchronous, no tokio
    // runtime — the router CPU pays for one process spawn per call and
    // nothing more.
    if cgi::is_cgi() {
        return cgi::run();
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("create tokio runtime")?
        .block_on(async_main())
}

async fn async_main() -> Result<()> {
    // Diagnostics: with SOCKROCKET_LOG_FILE set, write tracing output to that file
    // instead of stdout (which is unreliable when spawned without a console).
    if let Ok(path) = env::var("SOCKROCKET_LOG_FILE") {
        let file = fs::File::create(&path).with_context(|| format!("create {}", path))?;
        tracing_subscriber::fmt()
            .with_writer(move || file.try_clone().expect("log file clone"))
            .with_ansi(false)
            .with_max_level(tracing::Level::DEBUG)
            .init();
    } else {
        // Default to `info` when RUST_LOG is unset — fmt::init()'s bare
        // EnvFilter defaults to ERROR-only, which silently swallowed all
        // startup/diagnostic lines on the router ("daemon logs nothing").
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
            .init();
    }

    let args: Vec<String> = env::args().collect();

    // Handle --version / -V: print version and exit (used by merlin/install.sh)
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("sockrocket-cli {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // Handle --init: generate a default config and exit
    if args.iter().any(|a| a == "--init") {
        let path = args
            .iter()
            .position(|a| a == "--init")
            .and_then(|i| args.get(i + 1))
            .map(|s| s.as_str())
            .unwrap_or("config.yaml");
        return init_config(path);
    }

    // Hidden utility: print the curated China IPv4 CIDR list (one per line)
    // and exit. Used by the router's iptables.sh to load a kernel ipset for
    // hardware-NAT direct routing of domestic traffic.
    if args.iter().any(|a| a == "--dump-cn-cidrs") {
        for cidr in sockrocket_core::china_ipv4_cidrs() {
            println!("{cidr}");
        }
        return Ok(());
    }

    // API mode: `sockrocket-cli api [port]` — standalone HTTP JSON backend for the
    // router Web UI. Runs independently of the proxy daemon (the router's
    // httpd cannot execute user CGIs, so the UI needs this to reach
    // start/stop even while the proxy is stopped).
    if let Some(pos) = args.iter().position(|a| a == "api") {
        let port = args
            .get(pos + 1)
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(cgi::DEFAULT_API_PORT);
        return cgi::serve_api(port).await;
    }

    // Feature flags
    let enable_tun = args.iter().any(|a| a == "--tun");
    let setup_routes = args.iter().any(|a| a == "--tun-routes");

    let config_path = parse_config_path()?;
    let config = load_config(&config_path)
        .await
        .with_context(|| format!("failed to load config from {}", config_path))?;

    let active_index = resolve_active_index(&config)?;
    let router = build_router(&config.rules);
    let outbound = build_outbound(&config, active_index, router.as_ref())?;
    let selected = active_index.and_then(|index| config.nodes.get(index));

    // Swappable wrapper: health-check failover and config hot-reload replace
    // the inner outbound without rebinding the local listeners.
    let swappable = Arc::new(SwappableOutbound::new(outbound.clone()));
    let shared_outbound = SharedOutbound(swappable.clone());

    // Start HTTP/SOCKS5 proxy service
    let mut service = ProxyService::with_outbound(shared_outbound.clone());
    service
        .start(&config.listen_addr, config.socks_port, config.http_port)
        .await?;

    // Fake-IP pool, shared by the DNS listener (hands out fake addresses for
    // international names) and the TUN proxy (maps them back to domains and
    // dials by name). TUN mode only: socks/http clients must keep receiving
    // real addresses.
    let fakeip_pool = enable_tun.then(sockrocket_core::FakeIpPool::new);

    // Proxy-server hostnames, shared with the DNS resolver so node servers
    // always resolve direct/real (never fake-IP). Repopulated on config
    // reload; the resolver reads through the shared handle.
    let server_domains = Arc::new(std::sync::RwLock::new(collect_server_domains(
        &config.nodes,
    )));

    // Start the UDP DNS listener (backing the router's dnsmasq split-DNS
    // config, which forwards LAN queries to 127.0.0.1:<dns_port>). The port
    // and direct-upstream server live as router-only keys in config.yaml and
    // are read Value-level here: core's AppConfig does not model them.
    let dns_port = read_dns_port(&config_path);
    let dns_listener = match dns_port {
        Some(port) => {
            // International domains resolve via DNS-over-TCP through the
            // active proxy node: plain UDP 8.8.8.8 from the router's WAN is
            // GFW-poisoned (fake answers for google/youtube), while the node
            // exit is clean. Direct mode (no node) keeps plain UDP upstreams.
            let mut dns_config = sockrocket_core::china_split_dns_config();
            if active_index.is_some() {
                dns_config = sockrocket_core::china_split_dns_config_proxy_tls();
            }
            let mut resolver = DnsResolver::with_config(dns_config);
            resolver.set_server_domains(server_domains.clone());
            if active_index.is_some() {
                resolver.set_outbound(shared_outbound.clone());
                tracing::info!(
                    "DNS: international domains resolve via DNS-over-TLS through the proxy node"
                );
            }
            if let Some(pool) = &fakeip_pool {
                resolver.set_fakeip(pool.clone());
                tracing::info!("DNS: fake-IP mode active (198.18.0.0/16) for transparent proxy");
            }
            let shutdown = Arc::new(tokio::sync::Notify::new());
            let task = tokio::spawn({
                let shutdown = shutdown.clone();
                async move {
                    if let Err(e) =
                        sockrocket_core::serve_dns_udp("127.0.0.1", port, resolver, shutdown).await
                    {
                        tracing::warn!("DNS listener failed: {e:#}");
                    }
                }
            });
            Some((shutdown, task))
        }
        None => None,
    };

    if let Some(node) = selected {
        tracing::info!(
            "Sockrocket CLI started with node '{}' ({})",
            node.name,
            protocol_name(&node.protocol)
        );
    } else {
        tracing::warn!("Sockrocket CLI started without proxy node, outbound=direct");
    }

    tracing::info!(
        "Listening: SOCKS5={}:{}, HTTP={}:{}",
        config.listen_addr,
        config.socks_port,
        config.listen_addr,
        config.http_port
    );

    // Node health check + auto-switch (opt-in via `health_check.enabled`).
    let mut outbound_factory = make_outbound_factory(router.clone());
    if config.health_check.enabled {
        service.enable_health_check(HealthCheckSetup {
            config: config.health_check.clone(),
            nodes: config.nodes.clone(),
            active_node: active_index,
            swappable: swappable.clone(),
            outbound_factory: outbound_factory.clone(),
        });
        tracing::info!(
            "Health check enabled: every {}s, {} consecutive failure(s) trigger failover (auto_switch={})",
            config.health_check.interval_secs,
            config.health_check.failure_threshold,
            config.health_check.auto_switch
        );
        if let Some(mut rx) = service.health_events() {
            let health_config_path = config_path.clone();
            tokio::spawn(async move {
                while rx.changed().await.is_ok() {
                    match rx.borrow().clone() {
                        Some(HealthEvent::ProbeOk { latency_ms }) => {
                            tracing::debug!("health probe ok ({} ms)", latency_ms);
                            update_health_state(|s| {
                                s.last_probe_ms = Some(latency_ms);
                                s.last_probe_at = Some(unix_now());
                                s.consecutive_failures = 0;
                            });
                        }
                        Some(HealthEvent::ProbeFailed {
                            consecutive_failures,
                            error,
                        }) => {
                            tracing::warn!(
                                "health probe failed ({} in a row): {}",
                                consecutive_failures,
                                error
                            );
                            update_health_state(|s| {
                                s.consecutive_failures = consecutive_failures;
                                s.last_probe_at = Some(unix_now());
                                s.last_error = Some(error);
                            });
                        }
                        Some(HealthEvent::AutoSwitched {
                            node_index,
                            node_name,
                            latency_ms,
                        }) => {
                            tracing::info!(
                                "auto-switched to node '{}' ({} ms)",
                                node_name,
                                latency_ms
                            );
                            update_health_state(|s| {
                                s.current_node = Some(node_index);
                                s.current_node_name = Some(node_name.clone());
                                s.consecutive_failures = 0;
                                s.last_probe_ms = Some(latency_ms);
                                s.last_probe_at = Some(unix_now());
                                s.switch_count += 1;
                                s.last_switch = Some(SwitchRecord {
                                    at: unix_now(),
                                    node_index,
                                    node_name: node_name.clone(),
                                    latency_ms,
                                });
                            });
                            // Persist the failover target so a daemon restart
                            // comes up on the working node instead of the dead
                            // one (which would cost failure_threshold probes
                            // of downtime before switching again).
                            if let Err(e) = persist_active_node(&health_config_path, node_index) {
                                tracing::warn!("failed to persist active_node: {e:#}");
                            }
                        }
                        Some(HealthEvent::SwitchFailed { reason }) => {
                            tracing::warn!("auto-switch failed: {}", reason);
                            update_health_state(|s| {
                                s.last_error = Some(reason);
                            });
                        }
                        None => {}
                    }
                }
            });
        }
        // Seed the state file so the UI can show the active node before any
        // probe event fires. A stale current_node from a previous run is
        // discarded — it referred to that run's node list.
        update_health_state(|s| {
            s.current_node = active_index;
            s.current_node_name = selected.map(|n| n.name.clone());
        });
    }

    // Hot-reload: poll config.yaml for edits and apply the runtime-changeable
    // parts without rebinding the listeners.
    let mut config_watcher = ConfigWatcher::spawn(
        &config_path,
        &config,
        std::time::Duration::from_millis(2500),
    );

    // Optionally start TUN proxy
    let tun_proxy = if enable_tun {
        if !tun_supported() {
            tracing::warn!(
                "TUN mode not supported on this system: {}",
                tun_requirements()
            );
            None
        } else {
            match TunProxy::start(shared_outbound.clone(), fakeip_pool.clone(), router.clone()) {
                Ok(tun) => {
                    let route_info = tun.route_info();
                    tracing::info!(
                        "TUN device started: {} — TCP+UDP+ICMP proxy active",
                        route_info.tun_name
                    );

                    if setup_routes {
                        // Collect proxy server IPs to bypass routing loop
                        let bypass_ips = collect_proxy_ips(&config, active_index).await;
                        match setup_tun_routes(&route_info, &bypass_ips) {
                            Ok(guard) => {
                                tracing::info!(
                                    "System default route → TUN (desktop mode, {} IPs bypassed)",
                                    bypass_ips.len()
                                );
                                // The guard restores routes on drop — keep it
                                // out of scope for the whole process lifetime.
                                // Teardown goes through the globally saved
                                // state via emergency_restore_routes() at
                                // shutdown (and best-effort on hard kill).
                                std::mem::forget(guard);
                            }
                            Err(e) => tracing::warn!("Route setup failed: {}", e),
                        }
                    } else {
                        tracing::info!(
                            "TUN device: {} (router mode — use iptables/TPROXY to route traffic in)",
                            route_info.tun_name
                        );
                        tracing::info!(
                            "  TUN IP: 198.18.0.1  Add: ip route add <LAN> dev {}",
                            route_info.tun_name
                        );
                    }

                    Some(tun)
                }
                Err(e) => {
                    tracing::error!("Failed to start TUN proxy: {}", e);
                    None
                }
            }
        }
    } else {
        None
    };

    tracing::info!("Press Ctrl+C to stop");
    loop {
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                result?;
                break;
            }
            event = config_watcher.next() => {
                let Some(event) = event else { break };
                match event {
                    ConfigWatchEvent::ParseError(error) => {
                        // Bad edit: keep the running config untouched.
                        tracing::error!(
                            "config reload failed, keeping previous config: {}",
                            error
                        );
                    }
                    ConfigWatchEvent::Reloaded(reloaded) => {
                        for field in &reloaded.requires_restart {
                            tracing::warn!(
                                "config field '{}' changed — restart sockrocket-cli for it to take effect",
                                field
                            );
                        }
                        match apply_reloaded_config(reloaded.config).await {
                            Ok((new_config, new_index, new_outbound)) => {
                                let node_name = new_index
                                    .and_then(|i| new_config.nodes.get(i))
                                    .map(|n| n.name.clone())
                                    .unwrap_or_else(|| "direct".to_string());
                                swappable.set(new_outbound);
                                if let Ok(mut set) = server_domains.write() {
                                    *set = collect_server_domains(&new_config.nodes);
                                }
                                outbound_factory = make_outbound_factory(build_router(&new_config.rules));
                                if new_config.health_check.enabled {
                                    service
                                        .restart_health_check(HealthCheckSetup {
                                            config: new_config.health_check.clone(),
                                            nodes: new_config.nodes.clone(),
                                            active_node: new_index,
                                            swappable: swappable.clone(),
                                            outbound_factory: outbound_factory.clone(),
                                        })
                                        .await;
                                }
                                tracing::info!(
                                    "config reloaded: {} node(s), active='{}' (listeners unchanged)",
                                    new_config.nodes.len(),
                                    node_name
                                );
                            }
                            Err(e) => {
                                tracing::error!(
                                    "reloaded config is not usable, keeping previous: {:#}",
                                    e
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    tracing::info!("Shutdown signal received");
    config_watcher.shutdown();

    // Stop the DNS listener before the proxy/TUN so dnsmasq's forwarded
    // queries fail fast instead of hitting a half-torn-down resolver.
    if let Some((shutdown, task)) = dns_listener {
        shutdown.notify_waiters();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), task).await;
    }

    // Stop TUN first, then proxy
    if let Some(tun) = tun_proxy {
        tun.stop().await;
    }
    service.stop().await;

    // Clean up system proxy and TUN routes
    let _ = sockrocket_core::clear_system_proxy();
    sockrocket_core::emergency_restore_routes();

    Ok(())
}

/// Read the router-only `dns_port` key from config.yaml.
///
/// Returns `None` when the key is absent or unusable, which disables the DNS
/// listener. Present but 0/out-of-range is also treated as "disabled" rather
/// than an error: the router's template always ships the key, but a desktop
/// config never does, and binding a wrong port would be worse than not
/// listening (dnsmasq would forward LAN queries into a black hole).
fn read_dns_port(path: &str) -> Option<u16> {
    let content = fs::read_to_string(path).ok()?;
    let value: serde_yaml::Value = serde_yaml::from_str(&content).ok()?;
    let port = value.get("dns_port")?.as_u64()?;
    if port == 0 || port > u16::MAX as u64 {
        tracing::warn!("dns_port {} out of range — DNS listener disabled", port);
        return None;
    }
    Some(port as u16)
}

/// Collect the lowercase hostname set of all configured proxy servers.
/// IP literals are skipped (they never go through DNS); bracketed IPv6
/// literals from URL parsing are stripped before the literal check.
fn collect_server_domains(nodes: &[sockrocket_core::Node]) -> std::collections::HashSet<String> {
    nodes
        .iter()
        .map(|n| n.server.trim_matches(['[', ']']).to_ascii_lowercase())
        .filter(|h| !h.is_empty() && h.parse::<std::net::IpAddr>().is_err())
        .collect()
}

/// Health-check runtime state, rewritten on every health event so the
/// separate API-bridge process can render it in the Web UI. Lives in the
/// system temp dir — /tmp on the router is tmpfs, so the once-a-minute
/// probe updates cause no flash wear.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct HealthState {
    current_node: Option<usize>,
    current_node_name: Option<String>,
    last_probe_ms: Option<u32>,
    last_probe_at: Option<u64>,
    consecutive_failures: u32,
    switch_count: u32,
    last_switch: Option<SwitchRecord>,
    last_error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SwitchRecord {
    at: u64,
    node_index: usize,
    node_name: String,
    latency_ms: u32,
}

fn health_state_path() -> std::path::PathBuf {
    std::env::temp_dir().join("sockrocket_health.json")
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Read-modify-write the health state file atomically (tmp rename).
fn update_health_state(f: impl FnOnce(&mut HealthState)) {
    let path = health_state_path();
    let mut state: HealthState = fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    f(&mut state);
    if let Ok(json) = serde_json::to_string(&state) {
        let tmp = path.with_extension("tmp");
        if fs::write(&tmp, &json).is_ok() {
            let _ = fs::rename(&tmp, &path);
        }
    }
}

/// Write the failover target back into config.yaml as `active_node`, so the
/// next daemon start comes up on the working node instead of the dead one
/// (which would cost `failure_threshold` probe intervals of downtime before
/// the health checker switched again). Value-level rewrite — same approach
/// as the API bridge's set_node action. The config watcher will see this
/// write and do one harmless reload that rebuilds the same outbound the
/// health checker already installed.
fn persist_active_node(path: &str, index: usize) -> Result<()> {
    let content = fs::read_to_string(path)?;
    let mut value: serde_yaml::Value = serde_yaml::from_str(&content)?;
    if let Some(mapping) = value.as_mapping_mut() {
        mapping.insert(
            serde_yaml::Value::String("active_node".to_string()),
            serde_yaml::Value::Number(index.into()),
        );
    }
    // Atomic replace — in-place write on JFFS can truncate config.yaml if the
    // router loses power mid-write, which then blackholes LAN DNS while the
    // hijack still points at a dead listener.
    let tmp = format!("{path}.tmp");
    fs::write(&tmp, serde_yaml::to_string(&value)?)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn parse_config_path() -> Result<String> {
    // Skip flag arguments (--tun, --tun-routes, etc.)
    let path = env::args().skip(1).find(|a| !a.starts_with("--"));
    match path {
        Some(p) => Ok(p),
        None => {
            // Try default path
            let default_path = "config.yaml";
            if std::path::Path::new(default_path).exists() {
                tracing::info!("Using default config: {}", default_path);
                Ok(default_path.to_string())
            } else {
                bail!(
                    "usage: sockrocket-cli <config.yaml> [--tun] [--tun-routes]\n\
                     flags:\n\
                     \x20 --tun           enable TUN proxy (TCP+UDP transparent proxy)\n\
                     \x20 --tun-routes    also set system default route through TUN (desktop mode)\n\
                     \x20 --init [path]   generate default config file"
                )
            }
        }
    }
}

/// Generate a default config file at the given path.
fn init_config(path: &str) -> Result<()> {
    if std::path::Path::new(path).exists() {
        bail!("config file already exists: {}", path);
    }
    let config = AppConfig::default();
    let content = serde_yaml::to_string(&config)?;
    fs::write(path, &content)?;
    println!("✓ Default config written to: {}", path);
    println!("  Edit the file to add subscriptions or nodes, then run:");
    println!("  sockrocket-cli {}", path);
    Ok(())
}

async fn load_config(path: &str) -> Result<AppConfig> {
    let content = fs::read_to_string(path)?;
    let parsed: AppConfig = serde_yaml::from_str(&content)?;
    let file_node_count = parsed.nodes.len();
    let has_subscriptions = !parsed.subscriptions.is_empty();
    let config = resolve_config(parsed).await?;
    // Persist subscription-fetched nodes back into the config file so other
    // consumers that only read the file (e.g. the router Web UI's node list
    // and active-node index) stay consistent with what sockrocket-cli runs. The
    // write is Value-level: only the "nodes" key is replaced, every other
    // (including router-specific) key is preserved verbatim.
    if has_subscriptions
        && config.nodes.len() != file_node_count
        && let Err(e) = persist_merged_nodes(path, &content, &config)
    {
        tracing::warn!("failed to persist subscription nodes: {e:#}");
    }
    Ok(config)
}

/// Replace only the "nodes" section of the config file with the merged node
/// list, preserving all other keys (mode, dns_*, comments are lost but
/// unknown keys survive).
fn persist_merged_nodes(path: &str, original_content: &str, config: &AppConfig) -> Result<()> {
    let mut value: serde_yaml::Value = serde_yaml::from_str(original_content)?;
    if let serde_yaml::Value::Mapping(map) = &mut value {
        map.insert(
            serde_yaml::Value::String("nodes".to_string()),
            serde_yaml::to_value(&config.nodes)?,
        );
        let tmp = format!("{path}.tmp");
        fs::write(&tmp, serde_yaml::to_string(&value)?)?;
        fs::rename(&tmp, path)?;
    }
    Ok(())
}

/// Merge subscription-fetched nodes into the config and normalize names.
///
/// De-duplication matters here because `load_config` persists the merged list
/// back into config.yaml: without it, every start would append the full
/// subscription on top of the previously persisted copy, doubling the node
/// list each time until /jffs fills up. Duplicates (same server/port/
/// credentials/transport, whatever the display name) collapse to the first
/// occurrence, which is the older entry.
async fn resolve_config(mut config: AppConfig) -> Result<AppConfig> {
    if !config.subscriptions.is_empty() {
        tracing::info!(
            "Fetching {} subscription(s) from config",
            config.subscriptions.len()
        );

        let mut merged_nodes = config.nodes.clone();
        for subscription in &config.subscriptions {
            match fetch_subscription(subscription).await {
                Ok(mut nodes) => {
                    if nodes.is_empty() {
                        tracing::warn!(
                            "Subscription '{}' fetched OK but produced 0 nodes (check format / content)",
                            subscription.name
                        );
                    } else {
                        tracing::info!(
                            "Fetched {} node(s) from subscription '{}'",
                            nodes.len(),
                            subscription.name
                        );
                    }
                    merged_nodes.append(&mut nodes);
                }
                Err(e) => {
                    // One bad URL must not prevent the daemon (or other subs) from starting.
                    tracing::error!(
                        "failed to fetch subscription '{}': {e:#}",
                        subscription.name
                    );
                }
            }
        }

        let before = merged_nodes.len();
        let removed = sockrocket_core::dedup_nodes(&mut merged_nodes);
        if removed > 0 {
            tracing::info!(
                "Deduplicated {} node(s) after merging subscriptions ({} → {})",
                removed,
                before,
                merged_nodes.len()
            );
        }
        config.nodes = merged_nodes;
    }

    // Normalize node names (URL decode)
    normalize_node_names(&mut config.nodes);

    Ok(config)
}

/// Validate a hot-reloaded config end-to-end (active index, outbound
/// construction) before it replaces the running one.
///
/// Intentionally does **not** re-fetch subscriptions. Merlin (and the GUI
/// config editor) write `active_node` / toggles often; each write used to
/// trigger a full subscription download + 178→89 dedup, saturating the
/// router's WAN/CPU and briefly swapping the outbound mid-traffic.
/// Subscription refresh stays explicit: process start (`load_config`) and
/// `update-subs` / the daily cron.
async fn apply_reloaded_config(
    mut config: AppConfig,
) -> Result<(AppConfig, Option<usize>, SharedOutbound)> {
    normalize_node_names(&mut config.nodes);
    let index = resolve_active_index(&config)?;
    // NOTE: the TUN-side domain-rule override keeps the router built at
    // process start; reloaded rules apply to the routing outbound only
    // until the next restart.
    let outbound = build_outbound(&config, index, build_router(&config.rules).as_ref())?;
    Ok((config, index, outbound))
}

fn resolve_active_index(config: &AppConfig) -> Result<Option<usize>> {
    if config.nodes.is_empty() {
        return Ok(None);
    }

    match config.active_node {
        Some(index) if index < config.nodes.len() => Ok(Some(index)),
        Some(index) => bail!(
            "active_node {} is out of range for {} configured node(s)",
            index,
            config.nodes.len()
        ),
        None => {
            tracing::warn!("active_node is not set, using the first configured node");
            Ok(Some(0))
        }
    }
}

fn build_outbound(
    config: &AppConfig,
    active_index: Option<usize>,
    router: Option<&Arc<Router>>,
) -> Result<SharedOutbound> {
    let base_outbound = match active_index {
        Some(index) => create_outbound(&config.nodes[index])?,
        None => SharedOutbound::direct(),
    };

    Ok(wrap_with_rules(base_outbound, router))
}

/// Build the routing router once per process (None when no rules are
/// configured). Shared by the routing outbound and — in TUN mode — the
/// stream handler, which consults domain rules for bare-IP traffic whose
/// originating name is known from recent DNS answers.
fn build_router(rules: &[RoutingRule]) -> Option<Arc<Router>> {
    if rules.is_empty() {
        return None;
    }
    Some(Arc::new(
        Router::new(RuleSet::from_config(rules))
            .with_geoip(Arc::new(sockrocket_core::china_geoip_db())),
    ))
}

/// Wrap a node outbound with rule-based routing when a router exists.
fn wrap_with_rules(base_outbound: SharedOutbound, router: Option<&Arc<Router>>) -> SharedOutbound {
    match router {
        Some(router) => SharedOutbound(Arc::new(RoutingOutbound::new(
            router.clone(),
            base_outbound,
        ))),
        None => base_outbound,
    }
}

/// Factory used by the health monitor to rebuild the full (routing-aware)
/// outbound for a failover target node.
fn make_outbound_factory(router: Option<Arc<Router>>) -> OutboundFactory {
    Arc::new(move |node| {
        let base = create_outbound(node).ok()?;
        Some(wrap_with_rules(base, router.as_ref()))
    })
}

/// Resolve proxy server hostnames to IP addresses (v4 and v6) for TUN route bypass.
async fn collect_proxy_ips(config: &AppConfig, active_index: Option<usize>) -> Vec<IpAddr> {
    let mut ips = Vec::new();
    if let Some(idx) = active_index
        && let Some(node) = config.nodes.get(idx)
    {
        let resolved = resolve_to_ips(&node.server);
        if resolved.is_empty() {
            tracing::warn!("Could not resolve proxy server: {}", node.server);
        }
        ips.extend(resolved);
    }
    ips
}

fn protocol_name(protocol: &ProxyProtocol) -> &'static str {
    match protocol {
        ProxyProtocol::Shadowsocks { .. } => "shadowsocks",
        ProxyProtocol::VMess { .. } => "vmess",
        ProxyProtocol::VLess { .. } => "vless",
        ProxyProtocol::Tuic { .. } => "tuic",
        ProxyProtocol::Trojan { .. } => "trojan",
        ProxyProtocol::Hysteria2 { .. } => "hysteria2",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sockrocket_core::{Node, TransportConfig, TransportType};

    fn sample_node(name: &str) -> Node {
        Node {
            name: name.to_string(),
            server: "127.0.0.1".to_string(),
            port: 1080,
            protocol: ProxyProtocol::Shadowsocks {
                cipher: "aes-256-gcm".to_string(),
                password: "secret".to_string(),
                udp: false,
                shadow_tls: None,
            },
            transport: Some(TransportConfig {
                transport_type: TransportType::Tcp,
                tls: None,
                ws: None,
                reality: None,
            }),
            latency_ms: None,
            tags: vec![],
            extra: Default::default(),
        }
    }

    #[test]
    fn resolve_active_index_allows_empty_nodes() {
        let config = AppConfig::default();
        assert_eq!(resolve_active_index(&config).unwrap(), None);
    }

    #[test]
    fn resolve_active_index_uses_explicit_index() {
        let config = AppConfig {
            nodes: vec![sample_node("a"), sample_node("b")],
            active_node: Some(1),
            ..AppConfig::default()
        };

        assert_eq!(resolve_active_index(&config).unwrap(), Some(1));
    }

    #[test]
    fn resolve_active_index_defaults_to_first_node() {
        let config = AppConfig {
            nodes: vec![sample_node("a"), sample_node("b")],
            active_node: None,
            ..AppConfig::default()
        };

        assert_eq!(resolve_active_index(&config).unwrap(), Some(0));
    }

    #[test]
    fn resolve_active_index_rejects_out_of_range_index() {
        let config = AppConfig {
            nodes: vec![sample_node("a")],
            active_node: Some(2),
            ..AppConfig::default()
        };

        assert!(resolve_active_index(&config).is_err());
    }

    #[test]
    fn read_dns_port_reads_router_key() {
        let dir = std::env::temp_dir().join(format!("sockrocket-dnsport-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");

        // Present → Some(port). This is the router template's shape.
        std::fs::write(&path, "mode: \"tun\"\ndns_port: 5300\nnodes: []\n").unwrap();
        assert_eq!(read_dns_port(path.to_str().unwrap()), Some(5300));

        // Absent (desktop config) → None, listener disabled.
        std::fs::write(&path, "mode: \"tun\"\nnodes: []\n").unwrap();
        assert_eq!(read_dns_port(path.to_str().unwrap()), None);

        // Zero / out of range → None rather than binding a bad port.
        std::fs::write(&path, "dns_port: 0\n").unwrap();
        assert_eq!(read_dns_port(path.to_str().unwrap()), None);
        std::fs::write(&path, "dns_port: 70000\n").unwrap();
        assert_eq!(read_dns_port(path.to_str().unwrap()), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dedup_merge_same_credentials_collapse() {
        // Same server/port/cipher/password as sample_node — only the name
        // differs. This is exactly what a re-fetched subscription produces
        // after the previous fetch was persisted into config.yaml.
        let mut nodes = vec![
            sample_node("file copy"), // from config.yaml (previously persisted)
            sample_node("sub name"),  // freshly fetched
        ];
        let removed = sockrocket_core::dedup_nodes(&mut nodes);
        assert_eq!(removed, 1, "one duplicate must be removed");
        assert_eq!(nodes.len(), 1);
        // First occurrence wins: the file copy (older, persisted) stays.
        assert_eq!(nodes[0].name, "file copy");
    }

    #[test]
    fn dedup_merge_distinct_servers_kept() {
        let mut other = sample_node("other");
        other.server = "10.0.0.2".to_string();
        let mut nodes = vec![sample_node("a"), other];
        let removed = sockrocket_core::dedup_nodes(&mut nodes);
        assert_eq!(removed, 0);
        assert_eq!(nodes.len(), 2);
    }

    #[test]
    fn persist_merged_nodes_replaces_only_nodes_key() {
        let dir =
            std::env::temp_dir().join(format!("sockrocket-persist-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        let original =
            "mode: \"tun\"\nlisten_addr: \"0.0.0.0\"\nnodes: []\ncustom_router_key: keepme\n";
        std::fs::write(&path, original).unwrap();

        let config = AppConfig {
            nodes: vec![sample_node("persisted")],
            ..AppConfig::default()
        };
        persist_merged_nodes(path.to_str().unwrap(), original, &config).unwrap();

        let written = std::fs::read_to_string(&path).unwrap();
        let value: serde_yaml::Value = serde_yaml::from_str(&written).unwrap();
        let map = value.as_mapping().unwrap();
        // Unknown/router-specific keys survive.
        assert!(map.contains_key(serde_yaml::Value::String("custom_router_key".into())));
        assert_eq!(
            map.get(serde_yaml::Value::String("mode".into())).unwrap(),
            "tun"
        );
        // nodes got replaced with the merged list.
        let nodes = map
            .get(serde_yaml::Value::String("nodes".into()))
            .unwrap()
            .as_sequence()
            .unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(
            nodes[0].get("name").and_then(|v| v.as_str()),
            Some("persisted")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

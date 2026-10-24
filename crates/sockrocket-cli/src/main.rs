mod cgi;

use std::env;
use std::fs;
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use sockrocket_core::{
    AppConfig, ConfigWatchEvent, ConfigWatcher, DnsResolver, HealthCheckSetup, HealthEvent,
    OutboundFactory, ProxyMode, ProxyProtocol, ProxyService, Router, RoutingOutbound,
    SharedOutbound, SwappableOutbound, TrafficLedger, TunProxy, apply_subscription_refresh,
    create_outbound, fetch_subscription, flush_system_dns_cache, node_public_id,
    normalize_node_names, publish_traffic_catalog, resolve_to_ips, set_catalog_active,
    set_route_journal_path, setup_tun_routes, tun_requirements, tun_supported,
};

fn main() -> Result<()> {
    if let Some(result) = sockrocket_core::network_guardian::run_if_requested() {
        return result;
    }
    // CGI mode: invoked as `sockrocket.cgi` by the router httpd (AsusWRT-Merlin web
    // UI backend), or explicitly as `sockrocket-cli cgi`. Synchronous, no tokio
    // runtime — the router CPU pays for one process spawn per call and
    // nothing more.
    if env::args().nth(1).as_deref() == Some("--subscription-worker") {
        return cgi::subscription_worker(
            &env::args()
                .nth(2)
                .ok_or_else(|| anyhow::anyhow!("Subscription job ID required"))?,
        );
    }
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

    if let Some(index) = args.iter().position(|arg| arg == "--dns-health") {
        let port = args
            .get(index + 1)
            .ok_or_else(|| anyhow::anyhow!("DNS port required"))?
            .parse::<u16>()?;
        return sockrocket_core::dns::check_dns_listener(([127, 0, 0, 1], port).into()).await;
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

    // Validated IPv4 source networks needing userspace policy before CN bypass.
    if let Some(index) = args.iter().position(|arg| {
        matches!(
            arg.as_str(),
            "--dump-rule-sources" | "--dump-rule-sources-v6"
        )
    }) {
        let ipv6 = args[index] == "--dump-rule-sources-v6";
        let path = args
            .get(index + 1)
            .ok_or_else(|| anyhow::anyhow!("Config path required"))?;
        let config = AppConfig::from_yaml_str(&fs::read_to_string(path)?)?;
        if config.proxy_mode == ProxyMode::Rule {
            let sources: std::collections::BTreeSet<_> = config
                .rules
                .iter()
                .filter(|rule| {
                    rule.enabled && !rule.source.is_empty() && rule.source.contains(':') == ipv6
                })
                .map(|rule| rule.source.as_str())
                .collect();
            for source in sources {
                println!("{source}");
            }
        }
        return Ok(());
    }

    if let Some(index) = args.iter().position(|arg| arg == "--supports-udp") {
        let path = args
            .get(index + 1)
            .ok_or_else(|| anyhow::anyhow!("Config path required"))?;
        let config = AppConfig::from_yaml_str(&fs::read_to_string(path)?)?;
        if config.proxy_mode == ProxyMode::Direct {
            return Ok(());
        }
        // LAN-wide UDP/443 follows the active node, or an unsourced rule whose
        // group can carry UDP. One unused member must not open that port.
        match sockrocket_core::config::group::lan_wide_udp(&config) {
            Ok(true) => return Ok(()),
            Ok(false) => bail!("UDP disabled or unsupported for this node"),
            Err(message) => bail!(message),
        }
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
    // A previous run may have been killed while it still owned the default
    // route. Restore that snapshot before any subscription fetch, which needs
    // a working network.
    if let Some(dir) = Path::new(&config_path).parent() {
        if setup_routes {
            sockrocket_core::network_guardian::ensure(
                dir.join("system-proxy-owner.yaml"),
                dir.join("tun-routes.yaml"),
            )?;
        }
        set_route_journal_path(dir.join("tun-routes.yaml"));
        sockrocket_core::emergency_restore_routes();
    }
    let config = load_config(&config_path)
        .await
        .with_context(|| format!("failed to load config from {}", config_path))?;

    let dns_value: serde_yaml::Value = serde_yaml::from_str(&fs::read_to_string(&config_path)?)?;
    let dns_settings = sockrocket_core::config::dns::DnsSettings::from_yaml(&dns_value)?;
    sockrocket_core::dns::configure_bootstrap(
        &dns_settings,
        config.nodes.iter().map(|n| n.server.clone()),
    )?;
    anyhow::ensure!(
        !(enable_tun
            && !setup_routes
            && config.ipv6_mode == sockrocket_core::config::dns::Ipv6Mode::Compatibility
            && dns_settings.strategy == sockrocket_core::config::dns::AddressStrategy::Ipv6Only),
        "IPv6-only DNS requires dual_stack in Merlin transparent mode"
    );
    let active_index = resolve_active_index(&config)?;
    let router = router_for_mode(&config);
    let outbound = build_outbound(&config, active_index, router.as_ref())?;
    tracing::info!(
        "Proxy mode: {}",
        match config.proxy_mode {
            ProxyMode::Global => "Global (all via proxy)",
            ProxyMode::Rule => "Rule (user rules + China-direct)",
            ProxyMode::Direct => "Direct (no proxy)",
        }
    );
    let selected = active_index.and_then(|index| config.nodes.get(index));

    // Swappable wrapper: health-check failover and config hot-reload replace
    // the inner outbound without rebinding the local listeners.
    let swappable = Arc::new(SwappableOutbound::new(outbound.clone()));
    let mut dns_config = read_router_dns_config(&config_path, &config.nodes)?;
    set_transparent_dns_flags(
        &mut dns_config,
        enable_tun && !setup_routes,
        config.ipv6_mode,
        &config_path,
    );
    swappable.publish(outbound.clone(), Arc::new(dns_config));
    let shared_outbound = SharedOutbound(swappable.clone());
    // IPv6 DNS answers and the IPv6 listener both follow the installed LAN
    // interception, never the saved mode alone.
    // The mode last accepted by startup or reload; the gate task must not
    // re-read a file the reload rejected.
    let mut applied_ipv6_mode = config.ipv6_mode;
    let dual_stack_mode = Arc::new(std::sync::atomic::AtomicBool::new(
        config.ipv6_mode == sockrocket_core::config::dns::Ipv6Mode::DualStack,
    ));
    let ipv6_dns_open = Arc::new(std::sync::atomic::AtomicBool::new(
        enable_tun && !setup_routes && merlin_ipv6_answers(config.ipv6_mode, &config_path),
    ));
    if enable_tun && !setup_routes {
        let watcher = swappable.clone();
        let watched_config = config_path.clone();
        let gate = ipv6_dns_open.clone();
        let dual_stack_mode = dual_stack_mode.clone();
        tokio::spawn(async move {
            use std::sync::atomic::Ordering::Relaxed;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let now = dual_stack_mode.load(Relaxed) && merlin_ipv6_dns_ready(&watched_config);
                if now == gate.load(Relaxed) {
                    continue;
                }
                if watcher.update_dns(|dns| set_ipv6_answer_flags(dns, true, now)) {
                    gate.store(now, Relaxed);
                    tracing::info!(
                        active = now,
                        "LAN DNS follows the installed IPv6 interception"
                    );
                }
            }
        });
    }

    // Start HTTP/SOCKS5 proxy service
    let mut service = ProxyService::with_outbound(shared_outbound.clone());
    let traffic_ledger = {
        let path = Path::new(&config_path)
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("traffic.yaml");
        TrafficLedger::load_or_new(path)
    };
    service.stats().set_ledger(Some(traffic_ledger.clone()));
    sync_traffic_node(&traffic_ledger, &config, active_index);
    service
        .start(&config.listen_addr, config.socks_port, config.http_port)
        .await?;

    // Fake-IP pool, shared by the DNS listener (hands out fake addresses for
    // international names) and the TUN proxy (maps them back to domains and
    // dials by name). TUN mode only: socks/http clients must keep receiving
    // real addresses. Persist like Clash Meta so TUN toggle/restart reuses
    // the same 198.18.x.x → domain map (clients with cached Fake-IPs recover
    // immediately instead of waiting on DNS TTL).
    let fakeip_store = {
        let p = std::path::Path::new(&config_path);
        p.parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("fakeip.store")
    };
    let fakeip_pool = enable_tun.then(|| sockrocket_core::FakeIpPool::load_or_new(&fakeip_store));

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
    let (dns_failure_tx, mut dns_failure_rx) = tokio::sync::oneshot::channel();
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
            // Merlin owns IPv4 policy routing in iptables.sh, rather than
            // --tun-routes. Do not advertise unreachable LAN IPv6 endpoints.
            if enable_tun && !setup_routes && !merlin_ipv6_answers(config.ipv6_mode, &config_path) {
                resolver.set_client_ipv4_only(true);
                tracing::info!(
                    "LAN DNS: IPv4-only transparent gateway; AAAA and HTTPS hints suppressed"
                );
            }
            resolver.set_server_domains(server_domains.clone());
            resolver.follow_runtime(shared_outbound.clone());
            if active_index.is_some() {
                tracing::info!(
                    "DNS: international domains resolve via DNS-over-TLS through the proxy node"
                );
            }
            if let Some(pool) = &fakeip_pool {
                resolver.set_fakeip(pool.clone());
                resolver.set_geoip(Arc::new(sockrocket_core::china_geoip_db()));
                tracing::info!(
                    "DNS: fake-IP mode active (198.18.0.0/16); resolve-then-geoip:CN → real/direct"
                );
            }
            let shutdown = Arc::new(tokio::sync::Notify::new());
            let task = tokio::spawn({
                let shutdown = shutdown.clone();
                let ipv6_open = ipv6_dns_open.clone();
                async move {
                    let result = if enable_tun && !setup_routes {
                        sockrocket_core::serve_dns_udp_and_ipv6(
                            "127.0.0.1",
                            port,
                            resolver,
                            shutdown,
                            ipv6_open,
                        )
                        .await
                    } else {
                        sockrocket_core::serve_dns_udp("127.0.0.1", port, resolver, shutdown).await
                    };
                    if let Err(e) = result {
                        tracing::error!("DNS listener failed: {e:#}");
                        let _ = dns_failure_tx.send(e.to_string());
                    }
                }
            });
            Some((shutdown, task))
        }
        None => None,
    };

    if let Some(node) = selected {
        tracing::info!(
            "Sockrocket CLI started with configured node ({})",
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
    let mut outbound_factory = make_outbound_factory(router.clone(), &config);
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
            let traffic_ledger = traffic_ledger.clone();
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
                            node_key,
                            latency_ms,
                        }) => {
                            tracing::info!(
                                "auto-switched to node index {} ({} ms)",
                                node_index,
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
                            // The outbound already points at the new node.
                            let target = set_catalog_active(&node_public_id(&node_key), &node_name);
                            traffic_ledger.set_current_node(
                                Some(target.id.clone()),
                                Some(target.name.clone()),
                            );
                            traffic_ledger.set_current_subscription(
                                target.subscription_id.clone(),
                                target.subscription_name.clone(),
                            );
                            if let Err(e) = persist_active_node(&health_config_path, &node_key) {
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

    let diagnostic_stats = service.stats().clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let snapshot = diagnostic_stats.runtime_diagnostics();
            update_health_state(|health| health.runtime = Some(snapshot));
        }
    });

    // Optionally start TUN proxy
    let mut tun_proxy = if enable_tun {
        if !tun_supported() {
            tracing::warn!(
                "TUN mode not supported on this system: {}",
                tun_requirements()
            );
            None
        } else {
            match TunProxy::start_with_stats(
                shared_outbound.clone(),
                fakeip_pool.clone(),
                router.clone(),
                service.stats().clone(),
            ) {
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

    // When TUN dies unexpectedly the process must exit: SOCKS/DNS can still
    // look healthy while transparent proxy is gone, and Merlin's supervisor
    // only respawns on process exit (not on "pid alive, stack dead").
    let mut tun_unexpected_exit = tun_proxy
        .as_mut()
        .and_then(|tun| tun.take_unexpected_exit());

    tracing::info!("Press Ctrl+C / SIGTERM to stop");
    // Merlin's sockrocket.sh stops us with SIGTERM. tokio::signal::ctrl_c only
    // covers SIGINT — without SIGTERM the process ignored stop for the full
    // graceful-wait window (~8s) before kill -9, dominating TUN toggle latency.
    #[cfg(unix)]
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("register SIGTERM handler")?;
    #[cfg(unix)]
    let wait_term = async { sigterm.recv().await };
    #[cfg(not(unix))]
    let wait_term = std::future::pending::<Option<()>>();
    tokio::pin!(wait_term);
    let tun_fatal = async {
        match &mut tun_unexpected_exit {
            Some(rx) => {
                let _ = rx.await;
                true
            }
            None => {
                std::future::pending::<()>().await;
                false
            }
        }
    };
    tokio::pin!(tun_fatal);
    let mut proxy_lifecycle = service.subscribe_state();
    loop {
        tokio::select! {
            changed = proxy_lifecycle.changed() => {
                if changed.is_err() || matches!(&*proxy_lifecycle.borrow(), sockrocket_core::ProxyState::Error(_)) {
                    tracing::error!("Proxy listener failed; shutting down for supervisor recovery");
                    break;
                }
            }
            result = tokio::signal::ctrl_c() => {
                result?;
                break;
            }
            _ = &mut wait_term => {
                break;
            }
            fatal = &mut tun_fatal => {
                if fatal {
                    tracing::error!(
                        "TUN accept loop died unexpectedly — exiting so supervisor can restart"
                    );
                }
                break;
            }
            failure = &mut dns_failure_rx, if dns_port.is_some() => {
                tracing::error!(
                    "DNS listener exited unexpectedly ({failure:?}) — shutting down so supervisor can recover"
                );
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
                        // Compare with the mode the firewall was last told about,
                        // not the one at process start: dual_stack and back to
                        // the original mode must still reapply the policy.
                        let mode_changed = reloaded.config.ipv6_mode != applied_ipv6_mode;
                        for field in &reloaded.requires_restart {
                            if field == "ipv6_mode" {
                                continue;
                            }
                            tracing::warn!(
                                "config field '{}' changed — restart sockrocket-cli for it to take effect",
                                field
                            );
                        }
                        let content = match fs::read_to_string(&config_path) {
                            Ok(content) => content,
                            Err(error) => {
                                tracing::error!(
                                    "DNS config reload skipped, keeping previous config: {error:#}"
                                );
                                continue;
                            }
                        };
                        let parsed: serde_yaml::Value = match serde_yaml::from_str(&content) {
                            Ok(value) => value,
                            Err(error) => {
                                tracing::error!(
                                    "DNS config reload skipped, keeping previous config: {error:#}"
                                );
                                continue;
                            }
                        };
                        let settings = match sockrocket_core::config::dns::DnsSettings::from_yaml(
                            &parsed,
                        ) {
                            Ok(settings) => settings,
                            Err(error) => {
                                tracing::error!(
                                    "DNS config reload skipped, keeping previous config: {error:#}"
                                );
                                continue;
                            }
                        };
                        let decision = ipv6_reload_decision(
                            enable_tun && !setup_routes,
                            mode_changed,
                            reloaded.config.ipv6_mode
                                == sockrocket_core::config::dns::Ipv6Mode::Compatibility,
                            settings.strategy
                                == sockrocket_core::config::dns::AddressStrategy::Ipv6Only,
                        );
                        if decision == Ipv6Reload::Keep {
                            tracing::error!(
                                "IPv6-only DNS requires dual_stack in Merlin transparent mode; keeping previous config"
                            );
                            continue;
                        }
                        match apply_reloaded_config(reloaded.config).await {
                            Ok((new_config, new_index, new_outbound)) => {
                                let mut dns_config = match settings.resolver_config(
                                    new_config.nodes.iter().map(|node| node.server.clone()),
                                ) {
                                    Ok(config) => config,
                                    Err(error) => {
                                        tracing::error!(
                                            "DNS config reload skipped, keeping previous config: {error:#}"
                                        );
                                        continue;
                                    }
                                };
                                set_transparent_dns_flags(
                                    &mut dns_config,
                                    enable_tun && !setup_routes,
                                    new_config.ipv6_mode,
                                    &config_path,
                                );
                                if let Err(error) = sockrocket_core::dns::configure_bootstrap(
                                    &settings,
                                    new_config.nodes.iter().map(|node| node.server.clone()),
                                ) {
                                    tracing::error!(
                                        "DNS config reload skipped, keeping previous config: {error:#}"
                                    );
                                    continue;
                                }
                                let answers = dns_config.dual_stack_fakeip;
                                swappable.publish(new_outbound, Arc::new(dns_config));
                                applied_ipv6_mode = new_config.ipv6_mode;
                                dual_stack_mode.store(
                                    new_config.ipv6_mode
                                        == sockrocket_core::config::dns::Ipv6Mode::DualStack,
                                    std::sync::atomic::Ordering::Relaxed,
                                );
                                if enable_tun && !setup_routes {
                                    ipv6_dns_open
                                        .store(answers, std::sync::atomic::Ordering::Relaxed);
                                }
                                let new_router = router_for_mode(&new_config);
                                if let Some(tun) = &tun_proxy {
                                    tun.replace_router(new_router.clone());
                                }
                                if let Ok(mut set) = server_domains.write() {
                                    *set = collect_server_domains(&new_config.nodes);
                                }
                                outbound_factory = make_outbound_factory(new_router, &new_config);
                                // Disabling health checks must also stop the old monitor.
                                {
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
                                sync_traffic_node(&traffic_ledger, &new_config, new_index);
                                tracing::info!(
                                    "config reloaded: {} node(s), active index {:?} (listeners unchanged)",
                                    new_config.nodes.len(),
                                    new_index
                                );
                                if decision == Ipv6Reload::Reapply {
                                    let path = config_path.clone();
                                    if std::thread::Builder::new()
                                        .name("ipv6-policy".into())
                                        .spawn(move || reapply_merlin_policy(&path))
                                        .is_err()
                                    {
                                        tracing::error!(
                                            "ipv6_mode changed, but the firewall reapply thread did not start"
                                        );
                                    }
                                }
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

    // Keep the map across a TUN restart (proxy-off starts a new process).
    // Router stop removes network settings but retains this private cache so
    // clients with cached Fake-IP DNS answers still work after the next start.
    if let Some(pool) = &fakeip_pool
        && let Err(e) = pool.persist()
    {
        tracing::warn!("fake-IP store persist on shutdown failed: {e}");
    }

    // Stop the DNS listener before the proxy/TUN so dnsmasq's forwarded
    // queries fail fast instead of hitting a half-torn-down resolver.
    if let Some((shutdown, task)) = dns_listener {
        // One listener owns this notification. Store a permit even if it
        // has not yet reached notified() (e.g. immediate startup shutdown).
        shutdown.notify_one();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), task).await;
    }

    // Stop TUN first, then proxy
    if let Some(tun) = tun_proxy {
        tun.stop().await;
    }
    service.stop().await;

    if let Err(error) = traffic_ledger.flush() {
        tracing::warn!("traffic ledger flush on shutdown failed: {error:#}");
    }

    // Routes first: a stalled proxy command must not leave traffic pointed at
    // a TUN device that is about to disappear. Only a proxy this process
    // recorded is restored — never one the user configured themselves.
    sockrocket_core::emergency_restore_routes();
    if let Some(dir) = Path::new(&config_path).parent() {
        let _ = sockrocket_core::managed_proxy::restore(&dir.join("system-proxy-owner.yaml"));
    }
    if let Err(error) = flush_system_dns_cache() {
        tracing::warn!("DNS cache flush on shutdown failed: {error}");
    }

    Ok(())
}

fn read_router_dns_config(
    path: &str,
    nodes: &[sockrocket_core::Node],
) -> Result<sockrocket_core::dns::DnsConfig> {
    let value: serde_yaml::Value = serde_yaml::from_str(&fs::read_to_string(path)?)?;
    let settings = sockrocket_core::config::dns::DnsSettings::from_yaml(&value)?;
    settings.resolver_config(nodes.iter().map(|node| node.server.clone()))
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
    #[serde(default)]
    runtime: Option<serde_json::Value>,
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
    static WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let path = health_state_path();
    let mut state: HealthState = fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    f(&mut state);
    if let Ok(json) = serde_json::to_string(&state) {
        let _ = sockrocket_core::config::storage::atomic_write_private(&path, &json, false);
    }
}

/// Write the failover target back into config.yaml as `active_node`, so the
/// next daemon start comes up on the working node instead of the dead one
/// (which would cost `failure_threshold` probe intervals of downtime before
/// the health checker switched again). Value-level rewrite — same approach
/// as the API bridge's set_node action. The config watcher will see this
/// write and do one harmless reload that rebuilds the same outbound the
/// health checker already installed.
fn persist_active_node(path: &str, key: &str) -> Result<()> {
    let _transaction = sockrocket_core::config::storage::MutationGuard::acquire(Path::new(path))?;
    let content = fs::read_to_string(path)?;
    let current = AppConfig::from_yaml_str(&content)?;
    let index = current
        .nodes
        .iter()
        .position(|node| sockrocket_core::config::dedup::node_fingerprint(node) == key)
        .ok_or_else(|| anyhow::anyhow!("Failover node was removed before persistence"))?;
    let mut value: serde_yaml::Value = serde_yaml::from_str(&content)?;
    if let Some(mapping) = value.as_mapping_mut() {
        mapping.insert(
            serde_yaml::Value::String("active_node".into()),
            serde_yaml::Value::Number(index.into()),
        );
    }
    sockrocket_core::config::storage::atomic_write_private(
        Path::new(path),
        &serde_yaml::to_string(&value)?,
        true,
    )
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
                tracing::info!("Using default configuration");
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
    sockrocket_core::config::storage::atomic_write_private(Path::new(path), &content, false)?;
    println!("✓ Default config written to: {}", path);
    println!("  Edit the file to add subscriptions or nodes, then run:");
    println!("  sockrocket-cli {}", path);
    Ok(())
}

async fn load_config(path: &str) -> Result<AppConfig> {
    let content = fs::read_to_string(path)?;
    let parsed =
        AppConfig::from_yaml_str(&content).with_context(|| format!("parse config {}", path))?;
    let file_node_count = parsed.nodes.len();
    let has_subscriptions = !parsed.subscriptions.is_empty();
    // Fast path: nodes already persisted in config.yaml. Re-fetching every
    // start/TUN toggle was ~8s of WAN I/O on Merlin (4 subs, one 404). Explicit
    // refresh: `SOCKROCKET_REFRESH_SUBS=1` (update-subs / daily cron) or an
    // empty nodes list (first install).
    let refresh_subs = env::var_os("SOCKROCKET_REFRESH_SUBS").is_some();
    let config = if file_node_count > 0 && !refresh_subs {
        tracing::info!("Using {file_node_count} persisted node(s); skipping subscription fetch");
        let mut config = parsed;
        normalize_node_names(&mut config.nodes);
        config
    } else if has_subscriptions {
        let (config, changed, refreshed) = resolve_config(parsed).await?;
        // Always write the URL list and the nodes together, even when the
        // count is unchanged: a refresh can replace credentials in place.
        if changed {
            match sockrocket_core::config::storage::commit_subscription_refresh(
                Path::new(path),
                refreshed,
            ) {
                Ok(current) => current,
                Err(error) => {
                    tracing::warn!(
                        "Subscription save failed; preserving disk configuration: {error}"
                    );
                    config
                }
            }
        } else {
            config
        }
    } else {
        let mut config = parsed;
        normalize_node_names(&mut config.nodes);
        config
    };
    Ok(config)
}

/// Merge subscription-fetched nodes into the config and normalize names.
///
/// De-duplication matters here because `load_config` persists the merged list
/// back into config.yaml: without it, every start would append the full
/// subscription on top of the previously persisted copy, doubling the node
/// list each time until /jffs fills up. Duplicates (same server/port/
/// credentials/transport, whatever the display name) collapse to the first
/// occurrence, which is the older entry.
async fn resolve_config(
    mut config: AppConfig,
) -> Result<(AppConfig, bool, Vec<(String, Vec<sockrocket_core::Node>)>)> {
    tracing::info!(
        "Fetching {} subscription(s) from config",
        config.subscriptions.len()
    );

    let mut refreshed = Vec::new();
    for subscription in &config.subscriptions {
        match fetch_subscription(subscription).await {
            Ok(nodes) if nodes.is_empty() => {
                tracing::warn!(
                    "Subscription '{}' fetched OK but produced 0 nodes (check format / content)",
                    subscription.name
                );
            }
            Ok(nodes) => {
                tracing::info!(
                    "Fetched {} node(s) from subscription '{}'",
                    nodes.len(),
                    subscription.name
                );
                refreshed.push((subscription.url.clone(), nodes));
            }
            Err(e) => {
                // One bad URL must not prevent the daemon (or other subs) from starting.
                // Nodes already saved for that URL stay on disk.
                tracing::error!(
                    "failed to fetch subscription '{}': {e:#}",
                    subscription.name
                );
            }
        }
    }

    let changed = apply_subscription_refresh(&mut config, refreshed.clone());
    normalize_node_names(&mut config.nodes);
    Ok((config, changed, refreshed))
}

/// Validate a hot-reloaded config end-to-end (active index, outbound
/// construction) before it replaces the running one.
///
/// Intentionally does **not** re-fetch subscriptions. Merlin (and the GUI
/// config editor) write `active_node` / toggles often; each write used to
/// trigger a full subscription download + 178→89 dedup, saturating the
/// router's WAN/CPU and briefly swapping the outbound mid-traffic.
/// Subscription refresh stays explicit: `SOCKROCKET_REFRESH_SUBS=1` on start
/// (`update-subs` / daily cron) or the Web UI `update_subs` action.
async fn apply_reloaded_config(
    mut config: AppConfig,
) -> Result<(AppConfig, Option<usize>, SharedOutbound)> {
    normalize_node_names(&mut config.nodes);
    let index = resolve_active_index(&config)?;
    let outbound = build_outbound(&config, index, router_for_mode(&config).as_ref())?;
    Ok((config, index, outbound))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ipv6Reload {
    /// The new file cannot run in transparent mode. Leave the process on the
    /// previous config instead of exiting into a supervisor restart loop.
    Keep,
    Apply,
    /// DNS is applied in-process. The Merlin script rebuilds firewall rules
    /// without treating the process exit as a crash.
    Reapply,
}

/// Same tmpfs directory `iptables.sh` writes. A reboot removes it together
/// with the kernel rules it describes.
fn runtime_dir() -> std::path::PathBuf {
    env::var_os("SOCKROCKET_RUNTIME_DIR")
        .map(Into::into)
        .unwrap_or_else(|| "/tmp/sockrocket-run".into())
}

fn merlin_ipv6_dns_ready(_config_path: &str) -> bool {
    runtime_dir().join("ipv6_dns_active").is_file()
}

fn merlin_ipv6_answers(mode: sockrocket_core::config::dns::Ipv6Mode, config_path: &str) -> bool {
    mode == sockrocket_core::config::dns::Ipv6Mode::DualStack && merlin_ipv6_dns_ready(config_path)
}

/// IPv6 answers follow the installed interception on a transparent gateway.
/// A saved dual_stack mode still suppresses AAAA until that file exists.
fn set_transparent_dns_flags(
    dns: &mut sockrocket_core::dns::DnsConfig,
    merlin_transparent: bool,
    mode: sockrocket_core::config::dns::Ipv6Mode,
    config_path: &str,
) {
    let answers = merlin_transparent && merlin_ipv6_answers(mode, config_path);
    set_ipv6_answer_flags(dns, merlin_transparent, answers);
}

fn set_ipv6_answer_flags(
    dns: &mut sockrocket_core::dns::DnsConfig,
    merlin_transparent: bool,
    answers: bool,
) {
    dns.client_ipv4_only = merlin_transparent && !answers;
    dns.dual_stack_fakeip = answers;
}

fn ipv6_reload_decision(
    merlin_transparent: bool,
    mode_changed: bool,
    compatibility: bool,
    ipv6_only: bool,
) -> Ipv6Reload {
    if merlin_transparent && compatibility && ipv6_only {
        Ipv6Reload::Keep
    } else if merlin_transparent && mode_changed {
        Ipv6Reload::Reapply
    } else {
        Ipv6Reload::Apply
    }
}

fn sync_traffic_node(ledger: &TrafficLedger, config: &AppConfig, active_index: Option<usize>) {
    let direct = config.proxy_mode == ProxyMode::Direct;
    publish_traffic_catalog(&config.nodes, &config.subscriptions, active_index, direct);
    if direct {
        ledger.set_current_node(None, None);
        return;
    }
    match active_index.and_then(|index| config.nodes.get(index)) {
        Some(node) => {
            let target = set_catalog_active(
                &node_public_id(&sockrocket_core::node_fingerprint(node)),
                &node.name,
            );
            ledger.set_current_node(Some(target.id), Some(target.name));
            ledger.set_current_subscription(target.subscription_id, target.subscription_name);
        }
        None => ledger.set_current_node(None, None),
    }
}

fn reapply_merlin_policy(config_path: &str) {
    let Some(dir) = Path::new(config_path).parent() else {
        return;
    };
    let script = dir.join("scripts/sockrocket.sh");
    if !script.is_file() {
        tracing::error!(
            "ipv6_mode changed and DNS is updated, but {} is missing so the firewall was left unchanged",
            script.display()
        );
        return;
    }
    match std::process::Command::new("sh")
        .arg(&script)
        .arg("reapply-policy")
        .status()
    {
        Ok(status) if status.success() => {
            tracing::info!("ipv6_mode applied to the Merlin firewall without stopping the proxy");
        }
        Ok(status) => {
            tracing::error!("ipv6_mode firewall reapply failed ({status})");
        }
        Err(error) => {
            tracing::error!("ipv6_mode firewall reapply failed: {error}");
        }
    }
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
    sockrocket_core::config::group::validate_targets(&config.groups, &config.rules)
        .map_err(anyhow::Error::msg)?;
    if config.proxy_mode == ProxyMode::Direct {
        return Ok(SharedOutbound::direct());
    }
    let base_outbound = match active_index {
        Some(index) => create_outbound(&config.nodes[index])?,
        None => SharedOutbound::direct(),
    };

    Ok(wrap_with_rules(base_outbound, router, config))
}

/// Router for the configured [`ProxyMode`].
///
/// - **Rule**: device rules/finals, global rules, then China-direct, with the
///   in-binary GeoIP DB. Empty `rules` still yields China-direct.
/// - **Global** / **Direct**: no router (Global dials the active node; Direct
///   uses a direct outbound in [`build_outbound`]).
fn router_for_mode(config: &AppConfig) -> Option<Arc<Router>> {
    match config.proxy_mode {
        ProxyMode::Rule => Some(Arc::new(
            Router::new(sockrocket_core::rule_mode_ruleset(&config.rules))
                .with_geoip(Arc::new(sockrocket_core::china_geoip_db())),
        )),
        ProxyMode::Global | ProxyMode::Direct => None,
    }
}

/// Wrap a node outbound with rule-based routing when a router exists.
fn wrap_with_rules(
    base_outbound: SharedOutbound,
    router: Option<&Arc<Router>>,
    config: &AppConfig,
) -> SharedOutbound {
    match router {
        Some(router) => SharedOutbound(Arc::new(
            RoutingOutbound::new(router.clone(), base_outbound)
                .with_groups(&config.groups, &config.nodes),
        )),
        None => base_outbound,
    }
}

/// Factory used by the health monitor to rebuild the full (routing-aware)
/// outbound for a failover target node.
fn make_outbound_factory(router: Option<Arc<Router>>, config: &AppConfig) -> OutboundFactory {
    let config = config.clone();
    Arc::new(move |node| {
        let base = create_outbound(node).ok()?;
        Some(wrap_with_rules(base, router.as_ref(), &config))
    })
}

/// Resolve proxy server hostnames to IP addresses (v4 and v6) for TUN route bypass.
async fn collect_proxy_ips(config: &AppConfig, active_index: Option<usize>) -> Vec<IpAddr> {
    let mut ips = Vec::new();
    for group in &config.groups {
        for i in sockrocket_core::config::group::resolve_members(group, &config.nodes) {
            ips.extend(resolve_to_ips(&config.nodes[i].server));
        }
    }
    if let Some(idx) = active_index
        && let Some(node) = config.nodes.get(idx)
    {
        let resolved = resolve_to_ips(&node.server);
        if resolved.is_empty() {
            tracing::warn!("Could not resolve configured proxy server");
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
        ProxyProtocol::AnyTls { .. } => "anytls",
        ProxyProtocol::Wireguard { .. } => "wireguard",
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
    fn ipv6_reload_keeps_the_process_when_compatibility_cannot_start() {
        assert_eq!(
            ipv6_reload_decision(true, true, true, true),
            Ipv6Reload::Keep
        );
        assert_eq!(
            ipv6_reload_decision(true, false, true, true),
            Ipv6Reload::Keep
        );
    }

    #[test]
    fn ipv6_reload_reapplies_the_firewall_without_exiting() {
        assert_eq!(
            ipv6_reload_decision(true, true, false, true),
            Ipv6Reload::Reapply
        );
        assert_eq!(
            ipv6_reload_decision(false, true, true, true),
            Ipv6Reload::Apply
        );
        assert_eq!(
            ipv6_reload_decision(true, false, false, false),
            Ipv6Reload::Apply
        );
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
    fn persist_merged_nodes_replaces_nodes_and_subscription_urls() {
        use sockrocket_core::Subscription;
        let dir =
            std::env::temp_dir().join(format!("sockrocket-persist-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        let original = "mode: \"tun\"\nlisten_addr: \"0.0.0.0\"\nnodes: []\nsubscriptions: []\ncustom_router_key: keepme\n";
        std::fs::write(&path, original).unwrap();

        let config = AppConfig {
            nodes: vec![sample_node("persisted")],
            subscriptions: vec![Subscription {
                name: "Example".into(),
                url: "https://example.com/sub/demo".into(),
                format: "auto".into(),
                last_updated: Some(1),
                refresh_interval_hours: 24,
            }],
            ..AppConfig::default()
        };
        sockrocket_core::config::storage::write_nodes_and_subscriptions(&path, &config).unwrap();

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
        let subs = map
            .get(serde_yaml::Value::String("subscriptions".into()))
            .unwrap()
            .as_sequence()
            .unwrap();
        assert_eq!(
            subs[0].get("url").and_then(|v| v.as_str()),
            Some("https://example.com/sub/demo")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

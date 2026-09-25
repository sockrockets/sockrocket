use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::input::{InputEvent, InputState};
use gpui_component::{Icon, Sizable as _, Size as ComponentSize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::log_buffer::SharedLogBuffer;
use crate::theme::*;

// Keyboard shortcut actions
actions!(
    sockrocket,
    [
        Quit,
        ToggleProxy,
        SwitchToHome,
        SwitchToNodes,
        SwitchToConfig,
        SwitchToGroups,
        SwitchToConnections,
        SwitchToRules,
        SwitchToLogs,
        SwitchToSettings,
        TestAllLatency,
        CycleProxyMode,
        OpenCommandPalette,
        CloseCommandPalette,
        PaletteUp,
        PaletteDown,
    ]
);
use sockrocket_core::config::model::{
    AppConfig, GroupType, HealthCheckConfig, Node, ProxyGroupConfig, ProxyProtocol, RoutingRule,
    Subscription, TransportConfig, TransportType,
};
use sockrocket_core::proxy::ProxyStats;
use sockrocket_core::{
    ConfigWatchEvent, ConfigWatchHandle, ConfigWatcher, DEFAULT_SHARE_PORT, HealthCheckSetup,
    HealthEvent, Outbound, OutboundFactory, ProbeResult, ProxyMode, ProxyService, Router,
    RoutingOutbound, ShareServer, SharedOutbound, SharedShareState, SubscriptionFormat,
    SwappableOutbound, TunProxy, clear_system_proxy as clear_os_proxy, config_content_hash,
    create_outbound, dedup_nodes, fallback_pick, fetch_subscription, generate_token,
    get_system_proxy as get_os_proxy, local_lan_ip, new_shared_state, node_fingerprint,
    normalize_node_names, parse_proxy_uri, resolve_members, resolve_to_ips, rule_mode_ruleset,
    set_system_proxy as set_os_proxy, setup_tun_routes, start_share_server, system_proxy_supported,
    uniquify_node_names, url_test_pick,
};

/// Main application state
pub struct AppState {
    pub(crate) active_view: ActiveView,

    // Proxy state
    pub(crate) proxy_running: bool,
    pub(crate) proxy_status: String,
    pub(crate) proxy_validation_status: String,
    pub(crate) proxy_session_id: u64,

    // Proxy routing mode
    pub(crate) proxy_mode: ProxyMode,

    // Node management
    pub(crate) nodes: Vec<Node>,
    pub(crate) selected_node: Option<usize>,
    pub(crate) active_proxy_node: Option<usize>,

    // Input states
    pub(crate) import_url_input: Entity<InputState>,
    pub(crate) listen_addr_input: Entity<InputState>,
    pub(crate) socks_port_input: Entity<InputState>,
    pub(crate) http_port_input: Entity<InputState>,

    // Import status
    pub(crate) import_status: String,
    pub(crate) settings_status: String,
    pub(crate) rules_status: String,

    // Settings (applied values)
    pub(crate) listen_addr: String,
    pub(crate) socks_port: u16,
    pub(crate) http_port: u16,
    pub(crate) system_proxy_enabled: bool,
    pub(crate) system_proxy_status: String,
    pub(crate) system_proxy_managed_by_app: bool,

    // Proxy stats
    pub(crate) proxy_stats: Option<ProxyStats>,
    /// Snapshot for computing bandwidth speed: (bytes_up, bytes_down, instant)
    pub(crate) prev_bandwidth_snapshot: Option<(u64, u64, std::time::Instant)>,
    /// Computed upload speed in bytes/sec
    pub(crate) upload_speed_bps: f64,
    /// Computed download speed in bytes/sec
    pub(crate) download_speed_bps: f64,
    /// Ring buffer of recent upload speeds (bytes/sec), sampled ~1/s; last 60 samples
    pub(crate) upload_history: std::collections::VecDeque<f64>,
    /// Ring buffer of recent download speeds (bytes/sec), sampled ~1/s; last 60 samples
    pub(crate) download_history: std::collections::VecDeque<f64>,

    // Tokio runtime handle
    pub(crate) tokio_handle: tokio::runtime::Handle,

    // Proxy shutdown
    pub(crate) proxy_stop_tx: Option<tokio::sync::oneshot::Sender<()>>,
    /// Cancels in-flight proxy reachability validation immediately on disconnect.
    pub(crate) proxy_validation_cancel_tx: Option<tokio::sync::oneshot::Sender<()>>,
    /// Swappable layer in front of the active outbound: health-check failover
    /// and config hot-reload swap the inner connector without rebinding ports.
    pub(crate) proxy_swappable: Option<std::sync::Arc<SwappableOutbound>>,
    /// Outbound wrapper handed to the running TUN proxy.
    ///
    /// TUN is meant to be a *mode*: once it is on, switching nodes should keep
    /// it on. Wrapping the outbound in a swappable lets a node change call
    /// `set()` so new connections use the new node immediately â instead of
    /// tearing the TUN device down and rebuilding it (which dropped the default
    /// route and briefly blackholed traffic on every switch).
    pub(crate) tun_swappable: Option<std::sync::Arc<SwappableOutbound>>,

    // Node health check (settings from config; runtime status from monitor)
    pub(crate) health_check: HealthCheckConfig,
    pub(crate) health_status: String,

    // Config file hot-reload (watches our own gui-state.yaml; self-saves are
    // suppressed via content hashing)
    pub(crate) config_watch: Option<ConfigWatchHandle>,

    // TUN mode
    pub(crate) tun_enabled: bool,
    /// True between start_tun() spawning the task and tun_enabled being set.
    /// Guards against a second start_tun() call racing during the 15 s startup.
    pub(crate) tun_starting: bool,
    pub(crate) tun_status: String,
    pub(crate) tun_stop_tx: Option<tokio::sync::oneshot::Sender<()>>,

    // Rules management
    pub(crate) rules: Vec<RoutingRule>,
    pub(crate) rule_pattern_input: Entity<InputState>,
    // Selected values for type/target button groups
    pub(crate) rule_type_sel: String,
    pub(crate) rule_target_sel: String,

    // Node search filter
    pub(crate) node_filter: String,
    pub(crate) node_filter_input: Entity<InputState>,
    // Protocol chip filter â None means "All", Some("ss") etc. restricts to one protocol
    pub(crate) protocol_filter: Option<&'static str>,
    // Tag filter â None means show all nodes, Some(tag) restricts to nodes with that tag
    pub(crate) tag_filter: Option<String>,
    // Input for adding a tag to the selected node
    pub(crate) node_tag_input: Entity<InputState>,

    // Batch-selected node indices (for delete/share operations)
    pub(crate) selected_node_indices: std::collections::HashSet<usize>,

    // Nodes currently being latency-tested
    pub(crate) latency_testing: std::collections::HashSet<usize>,
    // Nodes whose most recent latency test completed with a failure
    pub(crate) latency_failed: std::collections::HashSet<usize>,
    // Failure category of each failed probe â drives the "Unreachable" reason
    // (timeout / server unreachable / TLS handshake failed / protocol error) shown in rows and panels.
    pub(crate) latency_fail_reason:
        std::collections::HashMap<usize, sockrocket_core::ProbeFailureKind>,
    // Bumped whenever the node list or any latency value changes, so the
    // Nodes page can cache its filtered+sorted index list across repaints
    // (latency batch tests otherwise re-run the whole filter/sort per frame).
    pub(crate) nodes_generation: u64,
    // Semaphore to cap concurrent latency tests and avoid network saturation
    pub(crate) latency_semaphore: std::sync::Arc<tokio::sync::Semaphore>,
    // Debounce flag: a persist task is already scheduled (avoid 50+ writes during batch test)
    pub(crate) pending_persist: bool,
    // Batch test tracking: count of batch-initiated tests still in-flight
    pub(crate) pending_latency_batch: usize,
    // When true, automatically switch to the fastest node after a batch test completes
    pub(crate) auto_select_best: bool,
    // When set, the batch-completion auto-pick is restricted to these node
    // indices (group "Test Now"); None means consider all nodes.
    pub(crate) auto_select_scope: Option<Vec<usize>>,

    // Saved subscriptions (URL + metadata) for auto-refresh
    pub(crate) subscriptions: Vec<Subscription>,
    // Indices of subscriptions currently being refreshed (shows spinner)
    pub(crate) refreshing_subscriptions: std::collections::HashSet<usize>,

    // Manual node URI input
    pub(crate) node_uri_input: Entity<InputState>,

    // Inline node rename
    pub(crate) node_rename_input: Entity<InputState>,
    pub(crate) editing_node_index: Option<usize>,

    // Rule editing
    pub(crate) editing_rule_index: Option<usize>,

    // Export & LAN share panel (Nodes page)
    pub(crate) show_share_panel: bool,
    pub(crate) export_format: SubscriptionFormat,
    pub(crate) export_status: String,
    pub(crate) export_saved_path: Option<PathBuf>,
    // One-shot status for node list actions (dedup, ...)
    pub(crate) nodes_action_status: String,
    // Node index whose share-URI QR code is expanded in the detail panel
    pub(crate) qr_expanded_node: Option<usize>,

    // LAN subscription share server
    pub(crate) lan_share_on: bool,
    pub(crate) lan_share_token: String,
    pub(crate) lan_share_port_input: Entity<InputState>,
    pub(crate) lan_share_format: SubscriptionFormat,
    pub(crate) lan_share_status: String,
    pub(crate) lan_ip: Option<std::net::IpAddr>,
    pub(crate) share_state: SharedShareState,
    pub(crate) share_server: Option<ShareServer>,

    // Config view active tab
    pub(crate) config_tab: ConfigTab,

    // Settings view active category
    #[allow(dead_code)] // wired for settings sidebar; Phase-1 UI is single-page
    pub(crate) settings_category: SettingsCategory,

    // Log viewer
    pub(crate) log_buffer: SharedLogBuffer,
    pub(crate) log_level_filter: tracing::Level,
    pub(crate) log_filter: String,
    pub(crate) log_filter_input: Entity<InputState>,

    // Command palette (Ctrl+K)
    pub(crate) palette_open: bool,
    pub(crate) palette_input: Entity<InputState>,
    pub(crate) palette_index: usize,
    /// Keeps input-event subscriptions alive.
    pub(crate) _subscriptions: Vec<gpui::Subscription>,

    // Proxy groups (user-defined, persisted in gui-state.yaml; members are
    // node fingerprints â see views/groups.rs)
    pub(crate) groups: Vec<ProxyGroupConfig>,
    // Groups page UI state: creation row (name input + type chips), inline
    // rename, per-group "Add Nodes" expander, delete confirm arming.
    pub(crate) group_name_input: Entity<InputState>,
    pub(crate) group_type_sel: GroupType,
    pub(crate) group_rename_input: Entity<InputState>,
    pub(crate) editing_group_index: Option<usize>,
    pub(crate) group_add_open: Option<usize>,
    pub(crate) group_delete_armed: Option<usize>,
    /// Group whose "Test Now" latency batch is in-flight; when the batch
    /// drains, the pick is computed via url_test_pick / fallback_pick
    /// (see finish_group_test) instead of the global min-latency rule.
    pub(crate) group_test_pending: Option<usize>,

    // Rule tester (Rules page)
    pub(crate) rule_tester_input: Entity<InputState>,

    // Uptime tracking for the dashboard stat card
    pub(crate) connected_since: Option<std::time::Instant>,

    // Connections page protocol filter
    pub(crate) conn_filter: ConnFilter,
}

/// Connections page protocol filter chips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnFilter {
    All,
    Socks5,
    Http,
}

impl ConnFilter {
    pub(crate) fn label(self) -> &'static str {
        match self {
            ConnFilter::All => "All",
            ConnFilter::Socks5 => "SOCKS5",
            ConnFilter::Http => "HTTP",
        }
    }

    pub(crate) fn matches(self, proto: &str) -> bool {
        match self {
            ConnFilter::All => true,
            ConnFilter::Socks5 => proto.eq_ignore_ascii_case("socks5"),
            ConnFilter::Http => proto.eq_ignore_ascii_case("http"),
        }
    }
}

/// One entry in the command palette result list.
#[derive(Debug, Clone)]
pub(crate) struct PaletteItem {
    pub kind: &'static str,
    pub label: String,
    pub hint: String,
    pub target: PaletteTarget,
}

#[derive(Debug, Clone)]
pub(crate) enum PaletteTarget {
    View(ActiveView),
    ToggleProxy,
    ToggleSystemProxy,
    ToggleTun,
    TestAll,
    TestAllBest,
    UseNode(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ActiveView {
    Home,
    Nodes,
    Groups,
    Connections,
    Config,
    Rules,
    Settings,
    Logs,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConfigTab {
    ImportSubscription,
    AddByUri,
}

#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // SystemProxy/About reserved for settings sections
pub enum SettingsCategory {
    ProxyConfig,
    SystemProxy,
    About,
}

pub(crate) fn current_system_proxy_state() -> (bool, String) {
    if !system_proxy_supported() {
        return (false, "Unsupported on this platform".to_string());
    }

    match get_os_proxy() {
        Ok(proxy) if proxy.enabled => (
            true,
            format!(
                "Enabled: {}:{} (bypass: {})",
                proxy.host, proxy.port, proxy.bypass
            ),
        ),
        Ok(_) => (false, "Disabled".to_string()),
        Err(err) => (false, format!("Error: {:#}", err)),
    }
}

pub(crate) fn rule_mode_summary(rules: &[RoutingRule]) -> String {
    let enabled = rules.iter().filter(|r| r.enabled).count();
    let total = rules.len();
    if total == 0 {
        "Built-in China direct rules".to_string()
    } else if enabled < total {
        format!("{} rule(s) active, {} disabled", enabled, total - enabled)
    } else {
        format!("{} custom rule(s)", total)
    }
}

/// Wrap a node outbound according to the proxy mode (rule routing / direct).
pub(crate) fn apply_proxy_mode(
    outbound: SharedOutbound,
    mode: &ProxyMode,
    rules: &[RoutingRule],
) -> SharedOutbound {
    match mode {
        ProxyMode::Global => outbound,
        ProxyMode::Rule => {
            let ruleset = rule_mode_ruleset(rules);
            let router = std::sync::Arc::new(
                Router::new(ruleset)
                    .with_geoip(std::sync::Arc::new(sockrocket_core::china_geoip_db())),
            );
            let routing = RoutingOutbound::new(router, outbound);
            SharedOutbound(std::sync::Arc::new(routing))
        }
        ProxyMode::Direct => SharedOutbound::direct(),
    }
}

impl AppState {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        tokio_handle: tokio::runtime::Handle,
        log_buffer: SharedLogBuffer,
    ) -> Self {
        let mut persisted = load_gui_state().unwrap_or_else(|| {
            let default_config = AppConfig::default();
            // Create default config on first launch
            let _ = save_gui_state(&default_config, None);
            default_config
        });
        let locale = sockrocket_gui::i18n::parse_locale(&persisted.locale);
        sockrocket_gui::i18n::set_locale(locale);
        // Startup cleanup of the persisted node list: URL-decode names, drop
        // cross-subscription fingerprint duplicates, and disambiguate display
        // names (names key group membership / selection restore, so they must
        // be unique). dedup shifts indices â remap active_node by fingerprint.
        let active_fp = persisted
            .active_node
            .and_then(|i| persisted.nodes.get(i))
            .map(node_fingerprint);
        let removed = dedup_nodes(&mut persisted.nodes);
        if removed > 0 {
            persisted.active_node = active_fp.and_then(|fp| {
                persisted
                    .nodes
                    .iter()
                    .position(|n| node_fingerprint(n) == fp)
            });
        }
        let renamed = uniquify_node_names(&mut persisted.nodes);
        if normalize_node_names(&mut persisted.nodes) || removed > 0 || renamed {
            let _ = save_gui_state(&persisted, None);
        }

        // Startup self-heal (a): a previous session that crashed or was killed
        // may have left the OS system proxy pointing at our (now dead) local
        // listeners, which breaks all browser traffic. Clear it.
        recover_stale_system_proxy(persisted.socks_port, persisted.http_port);
        // Startup self-heal (b): same crash scenario for TUN routes. The exit
        // cleanup (main.rs cleanup_on_exit) restores routes on a clean exit,
        // so leftovers imply a crash. sockrocket-core exposes no stale-route
        // *detection* API, so we conservatively restore only when the previous
        // session had TUN enabled; emergency_restore_routes() is a no-op when
        // there is nothing to restore (main.rs already calls it
        // unconditionally on every exit).
        // TODO(core): replace this heuristic with a real check ("default route
        // points at the TUN gateway while no TUN session is active") once
        // sockrocket-core exposes one.
        if persisted.tun_was_enabled {
            sockrocket_core::emergency_restore_routes();
            tracing::info!(
                "startup: restored routes possibly left behind by a crashed TUN session"
            );
        }
        let selected_node = persisted
            .active_node
            .filter(|index| *index < persisted.nodes.len());
        let import_url_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("https://example.com/subscribe?token=...")
        });
        let listen_addr_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("127.0.0.1")
                .default_value(persisted.listen_addr.clone())
        });
        let socks_port_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("1080")
                .default_value(persisted.socks_port.to_string())
        });
        let http_port_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("1087")
                .default_value(persisted.http_port.to_string())
        });
        let (system_proxy_enabled, system_proxy_status) = current_system_proxy_state();
        let rule_pattern_input = cx.new(|cx| InputState::new(window, cx).placeholder("google.com"));
        let node_filter_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search nodes..."));
        let node_tag_input = cx.new(|cx| InputState::new(window, cx).placeholder("Add tag..."));
        let node_uri_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("vless://... or vmess://... or ss://... or trojan://...")
        });
        let node_rename_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("New node name"));
        let log_filter_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter logs..."));
        let palette_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search pages, actions, nodes..."));
        let rule_tester_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("e.g. openai.com"));
        let lan_share_port_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(DEFAULT_SHARE_PORT.to_string())
                .default_value(DEFAULT_SHARE_PORT.to_string())
        });

        // Re-render the palette on every keystroke; Enter runs the selected match.
        let palette_sub = cx.subscribe(&palette_input, |this, _, event, cx| match event {
            InputEvent::Change => {
                this.palette_index = 0;
                cx.notify();
            }
            InputEvent::PressEnter { .. } => this.run_palette_first(cx),
            _ => {}
        });
        // Rule tester re-evaluates live as the user types.
        let tester_sub = cx.subscribe(&rule_tester_input, |_, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });

        // Groups page inputs: creation row (name + type) and inline rename.
        let group_name_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("New group name"));
        let group_rename_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("New group name"));

        let mut state = Self {
            active_view: ActiveView::Home,
            proxy_running: false,
            proxy_status: "Disconnected".to_string(),
            proxy_validation_status: "Not validated".to_string(),
            proxy_session_id: 0,
            proxy_mode: persisted.proxy_mode.clone(),
            nodes: persisted.nodes.clone(),
            selected_node,
            active_proxy_node: None,
            import_url_input,
            listen_addr_input,
            socks_port_input,
            http_port_input,
            import_status: if persisted.nodes.is_empty() {
                String::new()
            } else {
                format!("â Loaded {} saved nodes", persisted.nodes.len())
            },
            settings_status: String::new(),
            rules_status: String::new(),
            listen_addr: persisted.listen_addr,
            socks_port: persisted.socks_port,
            http_port: persisted.http_port,
            system_proxy_enabled,
            system_proxy_status,
            system_proxy_managed_by_app: false,
            proxy_stats: None,
            prev_bandwidth_snapshot: None,
            upload_speed_bps: 0.0,
            download_speed_bps: 0.0,
            upload_history: std::collections::VecDeque::new(),
            download_history: std::collections::VecDeque::new(),
            tokio_handle,
            proxy_stop_tx: None,
            proxy_validation_cancel_tx: None,
            proxy_swappable: None,
            tun_swappable: None,
            health_check: persisted.health_check.clone(),
            health_status: String::new(),
            config_watch: None,
            tun_enabled: false,
            tun_starting: false,
            tun_status: "Disabled".to_string(),
            tun_stop_tx: None,
            rules: persisted.rules.clone(),
            rule_pattern_input,
            rule_type_sel: "domain-suffix".to_string(),
            rule_target_sel: "proxy".to_string(),
            node_filter: String::new(),
            node_filter_input,
            protocol_filter: None,
            tag_filter: None,
            node_tag_input,
            latency_testing: std::collections::HashSet::new(),
            latency_failed: std::collections::HashSet::new(),
            latency_fail_reason: std::collections::HashMap::new(),
            nodes_generation: 0,
            latency_semaphore: std::sync::Arc::new(tokio::sync::Semaphore::new(5)),
            pending_persist: false,
            pending_latency_batch: 0,
            auto_select_best: false,
            auto_select_scope: None,
            selected_node_indices: std::collections::HashSet::new(),
            subscriptions: persisted.subscriptions.clone(),
            refreshing_subscriptions: std::collections::HashSet::new(),
            node_uri_input,
            node_rename_input,
            editing_node_index: None,
            editing_rule_index: None,
            show_share_panel: false,
            export_format: SubscriptionFormat::V2ray,
            export_status: String::new(),
            export_saved_path: None,
            nodes_action_status: String::new(),
            qr_expanded_node: None,
            lan_share_on: false,
            lan_share_token: generate_token(),
            lan_share_port_input,
            lan_share_format: SubscriptionFormat::Clash,
            lan_share_status: String::new(),
            lan_ip: None,
            share_state: new_shared_state(persisted.nodes.clone(), SubscriptionFormat::Clash),
            share_server: None,
            config_tab: ConfigTab::ImportSubscription,
            settings_category: SettingsCategory::ProxyConfig,
            log_buffer,
            log_level_filter: tracing::Level::INFO,
            log_filter: String::new(),
            log_filter_input,
            palette_open: false,
            palette_input,
            palette_index: 0,
            _subscriptions: vec![palette_sub, tester_sub],
            groups: persisted.groups.clone(),
            group_name_input,
            group_type_sel: GroupType::UrlTest,
            group_rename_input,
            editing_group_index: None,
            group_add_open: None,
            group_delete_armed: None,
            group_test_pending: None,
            rule_tester_input,
            connected_since: None,
            conn_filter: ConnFilter::All,
        };
        state.start_auto_refresh(cx);
        state.start_config_watch(cx);
        state.start_ui_heartbeat(cx);
        // Launched elevated via UAC with a pending TUN request? Start proxy and
        // then TUN unconditionally â the user already consented at the prompt.
        let tun_admin_pending = std::env::args().any(|a| a == "--tun-admin-pending");
        if tun_admin_pending {
            state.tun_status = "ð Elevated â starting proxy + TUN...".to_string();
            state.auto_connect_on_startup(cx, true);
        } else if persisted.auto_connect && persisted.active_node.is_some() {
            state.auto_connect_on_startup(cx, persisted.tun_was_enabled);
        }
        state
    }

    pub(crate) fn set_view(&mut self, view: ActiveView, cx: &mut Context<Self>) {
        self.active_view = view;
        cx.notify();
    }

    // === UI heartbeat ===
    // 1s tick while the app is alive. Bandwidth is sampled on every tick (so
    // the status bar rates stay current no matter which page is open), but a
    // repaint is only requested on pages with live data â a 1 fps full rebuild
    // of static pages (Nodes/Rules/Settings/...) is pure waste.
    pub(crate) fn start_ui_heartbeat(&mut self, cx: &mut Context<Self>) {
        let handle = self.tokio_handle.clone();
        cx.spawn(async move |weak, cx| {
            loop {
                handle
                    .spawn(async {
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    })
                    .await
                    .ok();
                let alive = weak
                    .update(cx, |this: &mut AppState, cx| {
                        this.sample_bandwidth();
                        if matches!(
                            this.active_view,
                            ActiveView::Home | ActiveView::Connections | ActiveView::Logs
                        ) || (this.active_view == ActiveView::Nodes
                            && !this.latency_testing.is_empty())
                        {
                            // The Nodes page repaints at 1 Hz while latency
                            // tests are in-flight so the "testing" indicator
                            // still pulses (cheaply) without a per-frame
                            // animation repaint storm.
                            cx.notify();
                        }
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        })
        .detach();
    }

    /// Sample proxy bandwidth counters into the speed readouts and the
    /// sparkline history ring buffers. Called once per UI heartbeat tick.
    /// (Previously computed inside render_home, which froze the status bar
    /// rates whenever another page was open.)
    pub(crate) fn sample_bandwidth(&mut self) {
        const HISTORY_LEN: usize = 60;
        let bytes_up = self
            .proxy_stats
            .as_ref()
            .map(|s| s.bytes_sent())
            .unwrap_or(0);
        let bytes_down = self
            .proxy_stats
            .as_ref()
            .map(|s| s.bytes_received())
            .unwrap_or(0);

        let now = std::time::Instant::now();
        if self.proxy_running {
            if let Some((prev_up, prev_down, prev_time)) = self.prev_bandwidth_snapshot {
                let dt = now.duration_since(prev_time).as_secs_f64();
                if dt >= 0.5 {
                    self.upload_speed_bps = (bytes_up.saturating_sub(prev_up)) as f64 / dt;
                    self.download_speed_bps = (bytes_down.saturating_sub(prev_down)) as f64 / dt;
                    self.prev_bandwidth_snapshot = Some((bytes_up, bytes_down, now));
                    // Push to history ring buffers
                    self.upload_history.push_back(self.upload_speed_bps);
                    self.download_history.push_back(self.download_speed_bps);
                    if self.upload_history.len() > HISTORY_LEN {
                        self.upload_history.pop_front();
                    }
                    if self.download_history.len() > HISTORY_LEN {
                        self.download_history.pop_front();
                    }
                }
            } else {
                self.prev_bandwidth_snapshot = Some((bytes_up, bytes_down, now));
            }
        } else {
            self.prev_bandwidth_snapshot = None;
            self.upload_speed_bps = 0.0;
            self.download_speed_bps = 0.0;
            // Drain history when proxy stops
            if !self.upload_history.is_empty() {
                self.upload_history.clear();
                self.download_history.clear();
            }
        }
    }

    // === Command palette (Ctrl+K) ===

    pub(crate) fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = true;
        self.palette_index = 0;
        self.palette_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    pub(crate) fn close_palette(&mut self, cx: &mut Context<Self>) {
        if self.palette_open {
            self.palette_open = false;
            cx.notify();
        }
    }

    /// Build the filtered palette result list from the current query.
    pub(crate) fn palette_items(&self, cx: &App) -> Vec<PaletteItem> {
        let mut items: Vec<PaletteItem> = Vec::new();
        self.for_each_palette_item(cx, |kind, label, hint, target| {
            items.push(PaletteItem {
                kind,
                label,
                hint,
                target,
            });
            true
        });
        items
    }

    /// Count of filtered palette entries, capped at `cap`. `palette_move`
    /// only needs the length to clamp the selection, so count with an early
    /// exit instead of materializing a Vec of PaletteItem (2-3 owned Strings
    /// per node) on every arrow-key press.
    pub(crate) fn palette_item_count(&self, cx: &App, cap: usize) -> usize {
        let mut count = 0usize;
        self.for_each_palette_item(cx, |_, _, _, _| {
            count += 1;
            count < cap
        });
        count
    }

    /// Single source for the palette item list: visits each entry whose label
    /// matches the current query, until `emit` returns false.
    fn for_each_palette_item(
        &self,
        cx: &App,
        mut emit: impl FnMut(&'static str, String, String, PaletteTarget) -> bool,
    ) {
        let query = self.palette_input.read(cx).value().trim().to_lowercase();
        // Returns false to stop the scan (the consumer has seen enough).
        let mut push =
            |kind: &'static str, label: String, hint: String, target: PaletteTarget| -> bool {
                if !query.is_empty() && !label.to_lowercase().contains(&query) {
                    return true; // filtered out â keep scanning
                }
                emit(kind, label, hint, target)
            };

        const PAGES: [(&str, ActiveView, &str); 7] = [
            ("Dashboard", ActiveView::Home, "Ctrl+1"),
            ("Nodes", ActiveView::Nodes, "Ctrl+2"),
            ("Groups", ActiveView::Groups, "Ctrl+7"),
            ("Connections", ActiveView::Connections, "Ctrl+3"),
            ("Rules", ActiveView::Rules, "Ctrl+4"),
            ("Logs", ActiveView::Logs, "Ctrl+5"),
            ("Settings", ActiveView::Settings, "Ctrl+6"),
        ];
        for (name, view, key) in PAGES {
            if !push(
                "page",
                format!("Go to {}", name),
                key.to_string(),
                PaletteTarget::View(view),
            ) {
                return;
            }
        }

        if !push(
            "action",
            if self.proxy_running {
                "Disconnect proxy".to_string()
            } else {
                "Connect proxy".to_string()
            },
            "Ctrl+Shift+C".to_string(),
            PaletteTarget::ToggleProxy,
        ) {
            return;
        }
        if !push(
            "action",
            "Toggle System Proxy".to_string(),
            String::new(),
            PaletteTarget::ToggleSystemProxy,
        ) {
            return;
        }
        if !push(
            "action",
            "Toggle TUN Mode".to_string(),
            String::new(),
            PaletteTarget::ToggleTun,
        ) {
            return;
        }
        if !push(
            "action",
            "Test all nodes".to_string(),
            "Ctrl+T".to_string(),
            PaletteTarget::TestAll,
        ) {
            return;
        }
        if !push(
            "action",
            "Test all & connect fastest".to_string(),
            String::new(),
            PaletteTarget::TestAllBest,
        ) {
            return;
        }

        for (i, node) in self.nodes.iter().enumerate() {
            let latency = node
                .latency_ms
                .map(|ms| format!("{}ms", ms))
                .unwrap_or_default();
            if !push(
                "node",
                format!("Use node {}", node.name),
                latency,
                PaletteTarget::UseNode(i),
            ) {
                return;
            }
        }
    }

    pub(crate) fn run_palette_first(&mut self, cx: &mut Context<Self>) {
        if !self.palette_open {
            return;
        }
        let items = self.palette_items(cx);
        let Some(item) = items
            .get(self.palette_index)
            .or_else(|| items.first())
            .cloned()
        else {
            return;
        };
        self.run_palette_item(item, cx);
    }

    /// Move the palette selection by `delta` (-1 up / +1 down), clamped to the
    /// currently filtered result list.
    pub(crate) fn palette_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        if !self.palette_open {
            return;
        }
        let len = self.palette_item_count(cx, 12);
        if len == 0 {
            self.palette_index = 0;
            cx.notify();
            return;
        }
        let next = self.palette_index as isize + delta;
        self.palette_index = next.clamp(0, len as isize - 1) as usize;
        cx.notify();
    }

    pub(crate) fn run_palette_item(&mut self, item: PaletteItem, cx: &mut Context<Self>) {
        self.palette_open = false;
        match item.target {
            PaletteTarget::View(view) => self.set_view(view, cx),
            PaletteTarget::ToggleProxy => self.toggle_proxy(cx),
            PaletteTarget::ToggleSystemProxy => {
                if self.system_proxy_enabled {
                    self.disable_system_proxy(cx);
                } else {
                    self.enable_system_proxy(cx);
                }
            }
            PaletteTarget::ToggleTun => {
                if self.tun_enabled {
                    self.stop_tun(cx);
                } else {
                    self.start_tun(cx);
                }
            }
            PaletteTarget::TestAll => {
                self.set_view(ActiveView::Nodes, cx);
                self.test_all_latency(cx);
            }
            PaletteTarget::TestAllBest => {
                self.set_view(ActiveView::Nodes, cx);
                self.test_all_and_select_best(cx);
            }
            PaletteTarget::UseNode(index) => self.use_node_by_index(index, cx),
        }
        cx.notify();
    }

    /// Select a node and make it the active outbound (reconnects if running).
    pub(crate) fn use_node_by_index(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.nodes.len() {
            return;
        }
        self.selected_node = Some(index);
        self.schedule_persist(cx);
        // Keep group "current" pointers consistent with the actual pick.
        let fp = node_fingerprint(&self.nodes[index]);
        for g in &mut self.groups {
            if g.members.contains(&fp) {
                g.current = Some(fp.clone());
            }
        }
        if self.proxy_running {
            self.restart_proxy_with_current_state(cx);
        }
        cx.notify();
    }

    // === Proxy groups (Groups page actions) ===
    //
    // Group members are node *fingerprints* (sockrocket_core::ProxyGroupConfig);
    // every mutation ends in schedule_persist() so gui-state.yaml stays in
    // sync. Fingerprints that no longer resolve to a node are skipped by
    // resolve_members() â they stay in config so a later subscription
    // refresh can resurrect them.

    /// Click a member chip: make that node the active outbound and record it
    /// as the group's current pick.
    pub(crate) fn group_use_member(
        &mut self,
        group_idx: usize,
        member_fp: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(node_idx) = self
            .nodes
            .iter()
            .position(|n| node_fingerprint(n) == member_fp)
        else {
            return;
        };
        if let Some(g) = self.groups.get_mut(group_idx) {
            g.current = Some(member_fp.to_string());
        }
        self.use_node_by_index(node_idx, cx);
    }

    /// Create a group from the Groups page creation row (name input + type
    /// chips). Empty names are ignored.
    pub(crate) fn group_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.group_name_input.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        self.groups.push(ProxyGroupConfig {
            name,
            gtype: self.group_type_sel,
            members: Vec::new(),
            current: None,
        });
        self.group_name_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.schedule_persist(cx);
        cx.notify();
    }

    /// One-click template: a url-test group spanning every current node
    /// (replaces the old auto-generated AUTO-SELECT group).
    pub(crate) fn group_template_auto_select(&mut self, cx: &mut Context<Self>) {
        if self.nodes.is_empty() {
            return;
        }
        let members = self.nodes.iter().map(node_fingerprint).collect();
        self.groups.push(ProxyGroupConfig {
            name: "AUTO-SELECT".to_string(),
            gtype: GroupType::UrlTest,
            members,
            current: None,
        });
        self.schedule_persist(cx);
        cx.notify();
    }

    /// Delete a group (the Groups page gates this behind a two-step confirm).
    pub(crate) fn group_delete(&mut self, group_idx: usize, cx: &mut Context<Self>) {
        if group_idx >= self.groups.len() {
            return;
        }
        self.groups.remove(group_idx);
        // Index-keyed UI state shifts after the removal: drop state pointing
        // at the deleted group, reindex state pointing past it.
        let shift = |slot: &mut Option<usize>| match *slot {
            Some(i) if i == group_idx => *slot = None,
            Some(i) if i > group_idx => *slot = Some(i - 1),
            _ => {}
        };
        shift(&mut self.editing_group_index);
        shift(&mut self.group_add_open);
        shift(&mut self.group_delete_armed);
        // An in-flight "Test Now" batch for a later group keeps its probes;
        // only the pending group-aware pick is reindexed/cancelled here.
        shift(&mut self.group_test_pending);
        self.schedule_persist(cx);
        cx.notify();
    }

    /// Confirm an inline rename from the group_rename_input.
    pub(crate) fn group_rename_confirm(&mut self, group_idx: usize, cx: &mut Context<Self>) {
        let new_name = self.group_rename_input.read(cx).value().trim().to_string();
        if !new_name.is_empty()
            && let Some(g) = self.groups.get_mut(group_idx)
        {
            g.name = new_name;
            self.schedule_persist(cx);
        }
        self.editing_group_index = None;
        cx.notify();
    }

    /// Add a node (by index) to a group; no-op if already a member.
    pub(crate) fn group_add_member(
        &mut self,
        group_idx: usize,
        node_idx: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(node) = self.nodes.get(node_idx) else {
            return;
        };
        let fp = node_fingerprint(node);
        if let Some(g) = self.groups.get_mut(group_idx)
            && !g.members.contains(&fp)
        {
            g.members.push(fp);
            self.schedule_persist(cx);
        }
        cx.notify();
    }

    /// Remove a member (by fingerprint) from a group; clears the group's
    /// current pick when it pointed at the removed member.
    pub(crate) fn group_remove_member(
        &mut self,
        group_idx: usize,
        member_fp: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(g) = self.groups.get_mut(group_idx) {
            g.members.retain(|m| m != member_fp);
            if g.current.as_deref() == Some(member_fp) {
                g.current = None;
            }
            self.schedule_persist(cx);
        }
        cx.notify();
    }

    /// Kick a latency test of this group's members only; when the batch
    /// drains, finish_group_test() computes the pick (url-test = lowest
    /// latency, fallback = first reachable in order) and switches to it.
    pub(crate) fn group_test_now(&mut self, group_idx: usize, cx: &mut Context<Self>) {
        let Some(g) = self.groups.get(group_idx) else {
            return;
        };
        let gtype = g.gtype;
        let member_indices = resolve_members(g, &self.nodes);
        if member_indices.is_empty() {
            return;
        }
        // Pre-pick from last known latencies so the group has a sensible
        // current pick even before the new results arrive.
        let probes: Vec<ProbeResult> = member_indices
            .iter()
            .map(|&i| (i, self.nodes[i].latency_ms))
            .collect();
        let pre_pick = match gtype {
            GroupType::Fallback => fallback_pick(&self.nodes, &probes),
            _ => url_test_pick(&self.nodes, &probes),
        };
        if let Some(fp) = pre_pick
            && let Some(idx) = self.nodes.iter().position(|n| node_fingerprint(n) == fp)
        {
            if let Some(g) = self.groups.get_mut(group_idx) {
                g.current = Some(fp);
            }
            self.use_node_by_index(idx, cx);
        }
        // The completion path is group-aware (group_test_pending), so the
        // generic min-latency auto-select must stay off for this batch.
        self.pending_latency_batch = member_indices.len();
        self.auto_select_best = false;
        self.auto_select_scope = None;
        self.group_test_pending = Some(group_idx);
        for i in member_indices {
            self.test_node_latency(i, cx);
        }
    }

    /// Batch drain for a group "Test Now": build probe results from the fresh
    /// latencies (failed probes left latency_ms = None), pick via the group's
    /// semantics, store the fingerprint as group.current and switch to it.
    fn finish_group_test(&mut self, group_idx: usize, cx: &mut Context<Self>) {
        let Some(g) = self.groups.get(group_idx) else {
            return;
        };
        let member_indices = resolve_members(g, &self.nodes);
        let probes: Vec<ProbeResult> = member_indices
            .iter()
            .map(|&i| (i, self.nodes[i].latency_ms))
            .collect();
        let pick = match g.gtype {
            GroupType::Fallback => fallback_pick(&self.nodes, &probes),
            _ => url_test_pick(&self.nodes, &probes),
        };
        let Some(fp) = pick else {
            return; // every member failed: keep the previous pick
        };
        let Some(idx) = self.nodes.iter().position(|n| node_fingerprint(n) == fp) else {
            return;
        };
        if let Some(g) = self.groups.get_mut(group_idx) {
            g.current = Some(fp);
        }
        self.use_node_by_index(idx, cx);
    }

    // === Proxy Control ===

    pub(crate) fn toggle_proxy(&mut self, cx: &mut Context<Self>) {
        if self.proxy_running {
            self.stop_proxy(cx);
        } else {
            self.start_proxy(cx);
        }
    }

    pub(crate) fn start_proxy(&mut self, cx: &mut Context<Self>) {
        if self.proxy_running {
            return;
        }

        if self.nodes.is_empty() {
            self.proxy_status = "No nodes available. Please add a node first.".to_string();
            cx.notify();
            return;
        }

        if self.selected_node.is_none() {
            self.proxy_status = "Please select a node first".to_string();
            cx.notify();
            return;
        }

        let selected_index = self.selected_node;
        let node = selected_index.and_then(|i| self.nodes.get(i).cloned());
        let node_name = node
            .as_ref()
            .map(|n| n.name.clone())
            .unwrap_or_else(|| "Direct".to_string());

        let outbound = match &node {
            Some(n) => match create_outbound(n) {
                Ok(o) => {
                    tracing::info!("Proxy outbound: {} (node: {})", o.0.name(), n.name);
                    o
                }
                Err(e) => {
                    self.proxy_status = format!("â Outbound error: {:#}", e);
                    cx.notify();
                    return;
                }
            },
            None => {
                tracing::info!("Proxy outbound: direct (no node selected)");
                SharedOutbound::direct()
            }
        };

        // Wrap outbound based on proxy mode
        let final_outbound = match self.proxy_mode {
            ProxyMode::Global => {
                tracing::info!("Proxy mode: Global");
                outbound
            }
            ProxyMode::Rule => {
                tracing::info!("Proxy mode: Rule ({})", rule_mode_summary(&self.rules));
                apply_proxy_mode(outbound, &ProxyMode::Rule, &self.rules)
            }
            ProxyMode::Direct => {
                tracing::info!("Proxy mode: Direct (no proxy)");
                SharedOutbound::direct()
            }
        };

        // Swappable layer: the health monitor (auto-switch) and config
        // hot-reload replace the inner outbound without rebinding listeners.
        let swappable = std::sync::Arc::new(SwappableOutbound::new(final_outbound));
        let service = ProxyService::with_outbound(SharedOutbound(swappable.clone()));
        let stats = service.stats().clone();

        // Health check setup: probes the active node and fails over to the
        // fastest reachable node when it dies. Entirely opt-in.
        let health_setup = if self.health_check.enabled {
            let rules = self.rules.clone();
            let mode = self.proxy_mode.clone();
            let factory: OutboundFactory = std::sync::Arc::new(move |node| {
                let base = create_outbound(node).ok()?;
                Some(apply_proxy_mode(base, &mode, &rules))
            });
            Some(HealthCheckSetup {
                config: self.health_check.clone(),
                nodes: self.nodes.clone(),
                active_node: selected_index,
                swappable: swappable.clone(),
                outbound_factory: factory,
            })
        } else {
            None
        };
        // Bridge for the monitor's event stream back into the UI task.
        let (health_events_tx, health_events_rx) =
            tokio::sync::oneshot::channel::<tokio::sync::watch::Receiver<Option<HealthEvent>>>();

        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (validation_cancel_tx, validation_cancel_rx) = tokio::sync::oneshot::channel::<()>();
        self.proxy_session_id += 1;
        let session_id = self.proxy_session_id;
        self.proxy_stop_tx = Some(stop_tx);
        self.proxy_validation_cancel_tx = Some(validation_cancel_tx);
        self.proxy_stats = Some(stats);
        self.prev_bandwidth_snapshot = None;
        self.upload_speed_bps = 0.0;
        self.download_speed_bps = 0.0;
        self.proxy_running = true;
        self.connected_since = Some(std::time::Instant::now());
        self.proxy_status = "Connecting...".to_string();
        self.proxy_validation_status = "Waiting for local proxy startup".to_string();
        self.active_proxy_node = None;
        self.proxy_swappable = Some(swappable);
        self.health_status = if self.health_check.enabled {
            "Monitoring node health...".to_string()
        } else {
            String::new()
        };
        // Persist auto_connect=true so a restart reconnects to this node.
        self.schedule_persist(cx);
        cx.notify();

        let listen_addr = self.listen_addr.clone();
        let socks_port = self.socks_port;
        let http_port = self.http_port;
        let handle = self.tokio_handle.clone();
        let direct_mode = self.proxy_mode == ProxyMode::Direct;

        cx.spawn(async move |weak, cx| {
            let service_listen_addr = listen_addr.clone();
            let join = handle.spawn(async move {
                let mut service = service;
                if let Err(err) = service
                    .start(&service_listen_addr, socks_port, http_port)
                    .await
                {
                    let _ = ready_tx.send(Err(err.to_string()));
                    return Err(err);
                }
                if let Some(setup) = health_setup {
                    service.enable_health_check(setup);
                    if let Some(events) = service.health_events() {
                        let _ = health_events_tx.send(events);
                    }
                }
                let _ = ready_tx.send(Ok(()));
                // Wait for stop signal
                let _ = stop_rx.await;
                service.stop().await;
                Ok::<_, anyhow::Error>(())
            });

            let ready_ok = match ready_rx.await {
                Ok(Ok(())) => {
                    weak.update(cx, |this: &mut AppState, cx| {
                        if this.proxy_session_id != session_id {
                            return;
                        }
                        this.active_proxy_node = selected_index;
                        // The local listeners are bound, but the node itself is
                        // not verified yet. Do NOT claim "Connected" and do NOT
                        // enable the system proxy here â both happen only after
                        // the reachability probe below succeeds, otherwise the
                        // UI would report success (and hijack browser traffic)
                        // while the node cannot actually reach anything.
                        this.proxy_status = format!("Verifying â {}", node_name);
                        this.proxy_validation_status =
                            "Verifying proxy reachability...".to_string();
                        cx.notify();
                    })
                    .ok();
                    true
                }
                Ok(Err(err)) => {
                    weak.update(cx, |this: &mut AppState, cx| {
                        if this.proxy_session_id != session_id {
                            return;
                        }
                        this.proxy_running = false;
                        this.proxy_stop_tx = None;
                        this.proxy_validation_cancel_tx = None;
                        this.proxy_stats = None;
                        this.active_proxy_node = None;
                        this.proxy_swappable = None;
                        this.proxy_status = format!("â Start failed: {:#}", err);
                        this.proxy_validation_status = "Not validated".to_string();
                        cx.notify();
                    })
                    .ok();
                    false
                }
                Err(_) => false,
            };

            let mut validation_task = if ready_ok {
                let addr_for_check = listen_addr.clone();
                Some(handle.spawn(async move {
                    tokio::select! {
                        result = verify_local_http_proxy(
                            &addr_for_check,
                            http_port,
                            std::time::Duration::from_secs(8),
                            direct_mode,
                        ) => result,
                        _ = validation_cancel_rx => {
                            Err(anyhow::anyhow!("cancelled"))
                        }
                    }
                }))
            } else {
                None
            };

            // Wait for proxy to stop, but do not let validation block disconnect.
            let mut join = join;
            let result = loop {
                if let Some(task) = validation_task.as_mut() {
                    tokio::select! {
                        proxy_check = task => {
                            let proxy_check = proxy_check
                                .map_err(|e| anyhow::anyhow!("task join error: {}", e))
                                .and_then(|r| r);
                            weak.update(cx, |this: &mut AppState, cx| {
                                if this.proxy_session_id != session_id || !this.proxy_running {
                                    return;
                                }
                                match proxy_check {
                                    Ok(summary) => {
                                        // Node verified â only now report
                                        // success and hand system traffic to
                                        // the proxy.
                                        this.proxy_status = if direct_mode {
                                            "Connected â Direct (no proxy)".to_string()
                                        } else {
                                            format!("Connected â {}", node_name)
                                        };
                                        this.proxy_validation_status =
                                            format!("â {}", summary);
                                        match set_os_proxy(&this.listen_addr, this.http_port) {
                                            Ok(()) => {
                                                this.system_proxy_managed_by_app = true;
                                                this.refresh_system_proxy_status(None, cx);
                                            }
                                            Err(err) => {
                                                this.system_proxy_managed_by_app = false;
                                                this.refresh_system_proxy_status(
                                                    Some(err.to_string()),
                                                    cx,
                                                );
                                            }
                                        }
                                    }
                                    Err(err) => {
                                        // Node failed verification. Keep the
                                        // local proxy running so the user can
                                        // retry, but do NOT claim success and
                                        // do NOT point the system proxy at an
                                        // unreachable node (that would break
                                        // all browser traffic).
                                        this.proxy_status =
                                            format!("â  Unreachable â {}", node_name);
                                        this.proxy_validation_status =
                                            format!("â  Reachability check failed â {:#}", err);
                                    }
                                }
                                cx.notify();
                            })
                            .ok();
                            validation_task = None;
                        }
                        result = &mut join => {
                            if let Some(task) = validation_task.take() {
                                task.abort();
                            }
                            break result;
                        }
                    }
                } else {
                    break join.await;
                }
            };

            weak.update(cx, |this: &mut AppState, cx| {
                if this.proxy_session_id != session_id {
                    return;
                }
                // System proxy was already cleared in stop_proxy().  Only
                // handle the case where the proxy service terminated on its
                // own (crash/error) without stop_proxy() being called.
                if this.system_proxy_managed_by_app {
                    match clear_os_proxy() {
                        Ok(()) => {
                            this.system_proxy_managed_by_app = false;
                            this.refresh_system_proxy_status(None, cx);
                        }
                        Err(err) => {
                            this.refresh_system_proxy_status(Some(err.to_string()), cx);
                        }
                    }
                }
                // These are already false/None when stop_proxy() was called;
                // set them here too to handle the self-termination path.
                this.proxy_running = false;
                this.proxy_stop_tx = None;
                this.proxy_validation_cancel_tx = None;
                this.proxy_stats = None;
                this.active_proxy_node = None;
                this.proxy_swappable = None;
                match result {
                    Ok(Ok(())) => {
                        this.proxy_status = "Disconnected".to_string();
                        this.proxy_validation_status = "Not validated".to_string();
                    }
                    Ok(Err(e)) => this.proxy_status = format!("â {:#}", e),
                    Err(e) => this.proxy_status = format!("â Task error: {}", e),
                }
                // Persist auto_connect=false after a self-termination too.
                this.schedule_persist(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();

        // Health event consumer: forwards monitor events into UI state until
        // the service task drops the event sender (on stop).
        cx.spawn(async move |weak, cx| {
            let Ok(mut events) = health_events_rx.await else {
                return; // health check disabled or service failed to start
            };
            while events.changed().await.is_ok() {
                let Some(event) = events.borrow().clone() else {
                    continue;
                };
                let alive = weak
                    .update(cx, |this: &mut AppState, cx| {
                        if this.proxy_session_id != session_id || !this.proxy_running {
                            return;
                        }
                        this.on_health_event(event, cx);
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn stop_proxy(&mut self, cx: &mut Context<Self>) {
        // Stop TUN first if active
        if self.tun_enabled {
            self.stop_tun(cx);
        }
        if self.system_proxy_managed_by_app {
            match clear_os_proxy() {
                Ok(()) => {
                    self.system_proxy_managed_by_app = false;
                    self.refresh_system_proxy_status(None, cx);
                }
                Err(err) => {
                    self.refresh_system_proxy_status(Some(err.to_string()), cx);
                }
            }
        }
        if let Some(tx) = self.proxy_stop_tx.take() {
            let _ = tx.send(());
        }
        // Cancel validation immediately so it doesn't continue probing after disconnect.
        if let Some(tx) = self.proxy_validation_cancel_tx.take() {
            let _ = tx.send(());
        }
        // Immediately reflect disconnected state in the UI so the user sees
        // the change on the same frame as the click, rather than waiting for
        // the background Tokio task to finish service.stop() and call back.
        // The background task's cleanup callback will still run and update
        // proxy_status to "Disconnected" (or an error string), completing the
        // transition.  These double-sets are harmless; session_id guards
        // prevent any stale callback from clobbering a new session.
        self.proxy_running = false;
        self.proxy_stats = None;
        self.active_proxy_node = None;
        self.proxy_swappable = None;
        self.health_status = String::new();
        self.proxy_validation_status = "Not validated".to_string();
        self.proxy_status = "Disconnecting...".to_string();
        // Persist auto_connect=false / tun_was_enabled=false.
        self.schedule_persist(cx);
        cx.notify();
    }

    /// Apply a health monitor event to UI state.
    pub(crate) fn on_health_event(&mut self, event: HealthEvent, cx: &mut Context<Self>) {
        match event {
            HealthEvent::ProbeOk { latency_ms } => {
                self.health_status = format!("â Node healthy ({} ms)", latency_ms);
            }
            HealthEvent::ProbeFailed {
                consecutive_failures,
                error,
            } => {
                self.health_status = format!(
                    "â  Health probe failed Ã{} â {}",
                    consecutive_failures, error
                );
            }
            HealthEvent::AutoSwitched {
                node_index,
                node_name,
                latency_ms,
            } => {
                self.active_proxy_node = Some(node_index);
                self.selected_node = Some(node_index);
                if let Some(node) = self.nodes.get_mut(node_index) {
                    node.latency_ms = Some(latency_ms);
                    self.nodes_generation = self.nodes_generation.wrapping_add(1);
                }
                self.proxy_status = format!("Connected â {} (auto-switched)", node_name);
                self.health_status =
                    format!("â Auto-switched to {} ({} ms)", node_name, latency_ms);
                // Persist the new active node so a restart keeps the healthy one.
                self.schedule_persist(cx);
            }
            HealthEvent::SwitchFailed { reason } => {
                self.health_status = format!("â Auto-switch failed: {}", reason);
            }
        }
        cx.notify();
    }

    pub(crate) fn restart_proxy_with_current_state(&mut self, cx: &mut Context<Self>) {
        if !self.proxy_running {
            return;
        }

        let restore_tun = self.tun_enabled;
        let handle = self.tokio_handle.clone();
        // stop_proxy() sets proxy_running = false immediately; we only need a
        // brief pause so the OS releases the port before the new bind attempt.
        self.stop_proxy(cx);

        cx.spawn(async move |weak, cx| {
            handle
                .spawn(async {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                })
                .await
                .ok();

            let proxy_started = weak
                .update(cx, |this: &mut AppState, cx| {
                    this.start_proxy(cx);
                    true
                })
                .unwrap_or(false);

            if !restore_tun || !proxy_started {
                return;
            }

            // Wait for the proxy to finish starting before re-enabling TUN.
            //
            // The old TUN must be *fully* stopped before a new one starts:
            // stop_tun() only signals the background task, and `tun_enabled`
            // stays true until route restoration plus wintun teardown finish.
            // The old budget (20 x 150ms = 3s) was too tight â a slow teardown
            // made this loop give up, leaving TUN running with the *previous*
            // node's outbound and bypass routes. The newly selected node's IP
            // then was not exempt, its traffic looped into TUN, and everything
            // reported "unreachable". Allow 20s.
            for _ in 0..133 {
                handle
                    .spawn(async {
                        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                    })
                    .await
                    .ok();

                let tun_started = weak
                    .update(cx, |this: &mut AppState, cx| {
                        if this.tun_enabled || !this.proxy_running {
                            false
                        } else {
                            // Auto-reconnect TUN restore: no window handle is
                            // available in this context. Only reachable when
                            // already privileged (unprivileged starts would have
                            // relaunched elevated first), so the UAC path in
                            // start_tun that needs `window` is never taken.
                            this.start_tun(cx);
                            true
                        }
                    })
                    .unwrap_or(false);

                if tun_started {
                    break;
                }
            }
        })
        .detach();
    }

    // === Import ===

    pub(crate) fn import_subscription(&mut self, cx: &mut Context<Self>) {
        let url = self.import_url_input.read(cx).value().to_string();
        if url.trim().is_empty() {
            self.import_status = "â  Please enter a subscription URL".to_string();
            cx.notify();
            return;
        }

        self.import_status = "â³ Importing...".to_string();
        cx.notify();

        let handle = self.tokio_handle.clone();
        let sub = Subscription {
            name: "Imported".to_string(),
            url: url.clone(),
            format: "auto".to_string(),
            last_updated: None,
            refresh_interval_hours: 24,
        };

        cx.spawn(async move |weak, cx| {
            let result = handle
                .spawn(async move { fetch_subscription(&sub).await })
                .await;

            weak.update(cx, |this: &mut AppState, cx| {
                match result {
                    Ok(Ok(mut new_nodes)) => {
                        normalize_node_names(&mut new_nodes);
                        // Tag nodes with source subscription URL for later refresh tracking
                        for n in &mut new_nodes {
                            n.extra.insert("sub_url".to_string(), url.clone());
                        }
                        let first_new_fp = new_nodes.first().map(node_fingerprint);
                        let count = new_nodes.len();
                        this.nodes.extend(new_nodes);
                        // Drop fingerprint-identical copies (across subscriptions too)
                        // and make display names unique.
                        dedup_nodes(&mut this.nodes);
                        uniquify_node_names(&mut this.nodes);
                        if count > 0 && this.selected_node.is_none() {
                            this.selected_node = first_new_fp.and_then(|fp| {
                                this.nodes.iter().position(|n| node_fingerprint(n) == fp)
                            });
                        }
                        this.import_status =
                            format!("â Imported {} nodes (total: {})", count, this.nodes.len());
                        // Save subscription for auto-refresh (upsert by URL)
                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();
                        if let Some(existing) = this.subscriptions.iter_mut().find(|s| s.url == url)
                        {
                            existing.last_updated = Some(now);
                        } else {
                            this.subscriptions.push(Subscription {
                                name: format!("Sub {}", this.subscriptions.len() + 1),
                                url: url.clone(),
                                format: "auto".to_string(),
                                last_updated: Some(now),
                                refresh_interval_hours: 24,
                            });
                        }
                        this.schedule_persist(cx);
                    }
                    Ok(Err(e)) => {
                        this.import_status = format!("â Error: {}", e);
                    }
                    Err(e) => {
                        this.import_status = format!("â Error: {}", e);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn clear_nodes(&mut self, cx: &mut Context<Self>) {
        if self.proxy_running {
            self.stop_proxy(cx);
        }
        self.nodes.clear();
        self.selected_node = None;
        self.active_proxy_node = None;
        self.latency_testing.clear();
        self.latency_failed.clear();
        self.latency_fail_reason.clear();
        self.pending_latency_batch = 0;
        self.auto_select_best = false;
        self.import_status = "Nodes cleared".to_string();
        self.proxy_validation_status = "Not validated".to_string();
        self.schedule_persist(cx);
        cx.notify();
    }

    // === Export / Dedup / LAN Share ===

    /// Nodes currently in scope for export & sharing: the batch selection if
    /// any, otherwise the full list.
    pub(crate) fn export_scope_nodes(&self) -> Vec<Node> {
        if self.selected_node_indices.is_empty() {
            self.nodes.clone()
        } else {
            let mut indices: Vec<usize> = self
                .selected_node_indices
                .iter()
                .copied()
                .filter(|&i| i < self.nodes.len())
                .collect();
            indices.sort_unstable();
            indices.into_iter().map(|i| self.nodes[i].clone()).collect()
        }
    }

    pub(crate) fn export_scope_label(&self) -> String {
        if self.selected_node_indices.is_empty() {
            format!("all {} node(s)", self.nodes.len())
        } else {
            format!("{} selected node(s)", self.selected_node_indices.len())
        }
    }

    pub(crate) fn copy_export_to_clipboard(&mut self, cx: &mut Context<Self>) {
        // No checkbox selection means "export all" (see export_scope_nodes).
        let nodes = self.export_scope_nodes();
        if nodes.is_empty() {
            self.export_status = "â  No nodes to export".to_string();
            cx.notify();
            return;
        }
        let text = self.export_format.render(&nodes);
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.export_saved_path = None;
        self.export_status = format!(
            "â Copied {} node(s) as {} subscription",
            nodes.len(),
            self.export_format.as_str()
        );
        cx.notify();
    }

    /// Save the subscription next to gui-state.yaml (no native file dialog in
    /// GPUI) and show the full path so the user can grab it.
    pub(crate) fn save_export_to_file(&mut self, cx: &mut Context<Self>) {
        // No checkbox selection means "export all" (see export_scope_nodes).
        let nodes = self.export_scope_nodes();
        if nodes.is_empty() {
            self.export_status = "â  No nodes to export".to_string();
            cx.notify();
            return;
        }
        let text = self.export_format.render(&nodes);
        let dir = gui_state_path()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(std::env::temp_dir);
        if let Err(err) = std::fs::create_dir_all(&dir) {
            self.export_status = format!("â Save failed: {}", err);
            cx.notify();
            return;
        }
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let path = dir.join(format!(
            "sockrocket-subscription-{}.{}",
            epoch,
            self.export_format.file_extension()
        ));
        match std::fs::write(&path, text) {
            Ok(()) => {
                self.export_saved_path = Some(path.clone());
                self.export_status =
                    format!("â Saved {} node(s) â {}", nodes.len(), path.display());
            }
            Err(err) => {
                self.export_saved_path = None;
                self.export_status = format!("â Save failed: {}", err);
            }
        }
        cx.notify();
    }

    /// Remove nodes with identical credentials (protocol + address + port +
    /// password/UUID), keeping the first occurrence. Persisted immediately.
    pub(crate) fn dedup_nodes_action(&mut self, cx: &mut Context<Self>) {
        // Indices shift after removal â preserve selection by name.
        let selected_name = self
            .selected_node
            .and_then(|i| self.nodes.get(i))
            .map(|n| n.name.clone());
        let active_name = self
            .active_proxy_node
            .and_then(|i| self.nodes.get(i))
            .map(|n| n.name.clone());

        let removed = dedup_nodes(&mut self.nodes);

        self.selected_node_indices.clear();
        self.latency_testing.clear();
        self.latency_failed.clear();
        self.latency_fail_reason.clear();
        self.selected_node =
            selected_name.and_then(|name| self.nodes.iter().position(|n| n.name == name));
        self.active_proxy_node =
            active_name.and_then(|name| self.nodes.iter().position(|n| n.name == name));

        self.nodes_action_status = if removed > 0 {
            format!("â Removed {} duplicate(s)", removed)
        } else {
            "No duplicates found".to_string()
        };
        self.schedule_persist(cx);
        cx.notify();
    }

    /// Push the current node scope + format into the share server's state.
    pub(crate) fn sync_share_state(&self) {
        if let Ok(mut state) = self.share_state.write() {
            state.nodes = self.export_scope_nodes();
            state.format = self.lan_share_format;
        }
    }

    pub(crate) fn toggle_lan_share(&mut self, cx: &mut Context<Self>) {
        if self.lan_share_on {
            if let Some(server) = self.share_server.take() {
                let handle = self.tokio_handle.clone();
                handle.spawn(async move {
                    server.stop().await;
                });
            }
            self.lan_share_on = false;
            self.lan_share_status = "Stopped".to_string();
            cx.notify();
            return;
        }

        let port = self
            .lan_share_port_input
            .read(cx)
            .value()
            .trim()
            .parse::<u16>()
            .unwrap_or(DEFAULT_SHARE_PORT);
        self.sync_share_state();

        let token = self.lan_share_token.clone();
        let state = self.share_state.clone();
        let handle = self.tokio_handle.clone();
        self.lan_share_status = "â³ Starting...".to_string();
        cx.notify();

        cx.spawn(async move |weak, cx| {
            let result = handle
                .spawn(async move { start_share_server("0.0.0.0", port, token, state).await })
                .await;
            weak.update(cx, |this: &mut AppState, cx| {
                match result {
                    Ok(Ok(server)) => {
                        let bound = server.port();
                        this.lan_ip = local_lan_ip();
                        this.share_server = Some(server);
                        this.lan_share_on = true;
                        this.lan_share_status = if this.lan_ip.is_some() {
                            format!("â Sharing on LAN port {}", bound)
                        } else {
                            format!(
                                "â Sharing on port {} (no LAN IP detected, URL uses 127.0.0.1)",
                                bound
                            )
                        };
                    }
                    Ok(Err(err)) => {
                        this.lan_share_status = format!("â Failed to start: {:#}", err);
                    }
                    Err(err) => {
                        this.lan_share_status = format!("â Failed to start: {}", err);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn set_lan_share_format(
        &mut self,
        format: SubscriptionFormat,
        cx: &mut Context<Self>,
    ) {
        self.lan_share_format = format;
        if let Ok(mut state) = self.share_state.write() {
            state.format = format;
        }
        cx.notify();
    }

    /// Full subscription URL for LAN clients (shown + QR-encoded when on).
    pub(crate) fn lan_share_url(&self) -> Option<String> {
        if !self.lan_share_on {
            return None;
        }
        let server = self.share_server.as_ref()?;
        let host = self
            .lan_ip
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "127.0.0.1".to_string());
        Some(format!(
            "http://{}:{}/sub?token={}&format={}",
            host,
            server.port(),
            self.lan_share_token,
            self.lan_share_format.as_str()
        ))
    }

    // === Latency Test ===

    pub(crate) fn test_node_latency(&mut self, index: usize, cx: &mut Context<Self>) {
        let node = match self.nodes.get(index) {
            Some(n) => n.clone(),
            None => return,
        };

        // Mark as testing; keep the previous latency value visible while the test
        // runs (cleared only if the new test fails).  Clear the failed flag so the
        // spinner renders instead of "â" while in-flight.
        self.latency_testing.insert(index);
        self.latency_failed.remove(&index);
        self.latency_fail_reason.remove(&index);
        cx.notify();

        let handle = self.tokio_handle.clone();
        let semaphore = self.latency_semaphore.clone();

        cx.spawn(async move |weak, cx| {
            let result = handle
                .spawn(async move {
                    // Acquire semaphore to cap concurrent tests (avoids saturating
                    // the local network when testing many nodes simultaneously).
                    let _permit = semaphore.acquire_owned().await.ok();

                    // Unwarmed: this outbound lives only for the latency
                    // probe; a warming pool would fire a full round of
                    // pre-connect handshakes per tested node.
                    let outbound = sockrocket_core::create_outbound_unwarmed(&node)
                        // Outbound construction failures are local config
                        // problems, not network reachability.
                        .map_err(|e| (format!("{:#}", e), sockrocket_core::ProbeFailureKind::Other))?;
                    // Run TCP ping and full HTTP probe concurrently.
                    // HTTP gives protocol-realistic latency; TCP is the fallback
                    // if the proxy probe target is temporarily unreachable.
                    // Timeout reduced to 5s (was 10s) to keep batch tests responsive.
                    let server = node.server.clone();
                    let port = node.port;
                    let (tcp_result, http_result) = tokio::join!(
                        sockrocket_core::tcp_latency_test(&server, port, 5),
                        sockrocket_core::http_latency_test(&outbound, 5),
                    );
                    // The HTTP probe runs *through* the proxy, so it is the only
                    // result that proves the node actually works. A TCP ping that
                    // succeeds while the proxy probe fails means the port is
                    // reachable but the node cannot carry traffic â reporting the
                    // TCP latency there would show a latency number (and imply
                    // "available") for a node that is really unusable. So the
                    // proxy probe result is authoritative; TCP is diagnostic only.
                    match http_result {
                        Ok(ms) => Ok(ms),
                        Err(http_err) => {
                            // Classify the authoritative proxy-probe failure so
                            // the UI can show why the node is unreachable.
                            let kind = sockrocket_core::classify_probe_error(&http_err);
                            let msg = match tcp_result {
                                Ok(_) => format!(
                                    "proxy probe failed: {:#} (tcp port reachable but node unusable)",
                                    http_err
                                ),
                                Err(tcp_err) => format!(
                                    "proxy probe failed: {:#}; tcp fallback failed: {:#}",
                                    http_err, tcp_err
                                ),
                            };
                            Err((msg, kind))
                        }
                    }
                })
                .await;

            weak.update(cx, |this: &mut AppState, cx| {
                this.latency_testing.remove(&index);
                match result {
                    Ok(Ok(ms)) => {
                        this.latency_failed.remove(&index);
                        this.latency_fail_reason.remove(&index);
                        if let Some(n) = this.nodes.get_mut(index) {
                            n.latency_ms = Some(ms);
                        }
                    }
                    Ok(Err((_, kind))) => {
                        this.latency_failed.insert(index);
                        this.latency_fail_reason.insert(index, kind);
                        if let Some(n) = this.nodes.get_mut(index) {
                            n.latency_ms = None;
                        }
                    }
                    Err(_) => {
                        this.latency_failed.insert(index);
                        this.latency_fail_reason
                            .insert(index, sockrocket_core::ProbeFailureKind::Other);
                        if let Some(n) = this.nodes.get_mut(index) {
                            n.latency_ms = None;
                        }
                    }
                }
                // A latency value changed â re-sort the Nodes page once.
                this.nodes_generation = this.nodes_generation.wrapping_add(1);
                // Batch test tracking: decrement counter and auto-select best when done
                if this.pending_latency_batch > 0 {
                    this.pending_latency_batch -= 1;
                    if this.pending_latency_batch == 0 {
                        if let Some(gi) = this.group_test_pending.take() {
                            // Group "Test Now" batch: pick via the group's own
                            // semantics (url-test / fallback) and switch.
                            this.finish_group_test(gi, cx);
                        } else if this.auto_select_best {
                            // Find the fastest node, restricted to the batch
                            // scope when set.
                            let scope = this.auto_select_scope.take();
                            let best = this
                                .nodes
                                .iter()
                                .enumerate()
                                .filter(|(i, _)| {
                                    scope.as_ref().is_none_or(|s| s.contains(i))
                                })
                                .filter_map(|(i, n)| n.latency_ms.map(|ms| (i, ms)))
                                .min_by_key(|&(_, ms)| ms)
                                .map(|(i, _)| i);
                            if let Some(best_idx) = best {
                                this.activate_node(best_idx, cx);
                                // Keep group "current" pointers consistent
                                // with the auto-selected pick.
                                let fp = node_fingerprint(&this.nodes[best_idx]);
                                for g in &mut this.groups {
                                    if g.members.contains(&fp) {
                                        g.current = Some(fp.clone());
                                    }
                                }
                            }
                        }
                        // Always reset the flags when a batch drains â leaving
                        // auto_select_best set would leak into the next batch.
                        this.auto_select_best = false;
                        this.auto_select_scope = None;
                    }
                }
                this.schedule_persist(cx); // debounced: coalesces N node completions â 1 write
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn test_all_latency(&mut self, cx: &mut Context<Self>) {
        let count = self.nodes.len();
        self.pending_latency_batch = 0; // plain test-all doesn't trigger auto-select
        self.auto_select_best = false;
        self.auto_select_scope = None;
        self.group_test_pending = None;
        for i in 0..count {
            self.test_node_latency(i, cx);
        }
    }

    /// Test all nodes and automatically switch to the fastest one when done.
    pub(crate) fn test_all_and_select_best(&mut self, cx: &mut Context<Self>) {
        let count = self.nodes.len();
        if count == 0 {
            return;
        }
        self.pending_latency_batch = count;
        self.auto_select_best = true;
        self.auto_select_scope = None;
        self.group_test_pending = None;
        for i in 0..count {
            self.test_node_latency(i, cx);
        }
    }

    // === Settings ===

    pub(crate) fn apply_settings(&mut self, cx: &mut Context<Self>) {
        let addr = self.listen_addr_input.read(cx).value().trim().to_string();
        let socks = self.socks_port_input.read(cx).value().trim().to_string();
        let http = self.http_port_input.read(cx).value().trim().to_string();

        if addr.is_empty() {
            self.settings_status = "â Listen address cannot be empty".to_string();
            cx.notify();
            return;
        }

        let socks_port = match socks.parse::<u16>() {
            Ok(port) => port,
            Err(_) => {
                self.settings_status = format!("â Invalid SOCKS5 port: {}", socks);
                cx.notify();
                return;
            }
        };
        let http_port = match http.parse::<u16>() {
            Ok(port) => port,
            Err(_) => {
                self.settings_status = format!("â Invalid HTTP port: {}", http);
                cx.notify();
                return;
            }
        };

        let changed = self.listen_addr != addr
            || self.socks_port != socks_port
            || self.http_port != http_port;

        self.listen_addr = addr;
        self.socks_port = socks_port;
        self.http_port = http_port;
        self.schedule_persist(cx);
        self.refresh_system_proxy_status(None, cx);

        self.settings_status = if changed {
            if self.proxy_running {
                "â Settings applied. Restarting proxy with new ports.".to_string()
            } else {
                "â Settings applied".to_string()
            }
        } else {
            "â Settings already up to date".to_string()
        };

        if changed && self.proxy_running {
            self.restart_proxy_with_current_state(cx);
        }
        cx.notify();
    }

    pub(crate) fn enable_system_proxy(&mut self, cx: &mut Context<Self>) {
        match set_os_proxy(&self.listen_addr, self.http_port) {
            Ok(()) => {
                // We set it, so stop_proxy()/"Disconnect" must clear it â
                // otherwise the system proxy keeps pointing at a dead port.
                self.system_proxy_managed_by_app = true;
                self.refresh_system_proxy_status(None, cx);
            }
            Err(err) => self.refresh_system_proxy_status(Some(format!("{:#}", err)), cx),
        }
    }

    pub(crate) fn disable_system_proxy(&mut self, cx: &mut Context<Self>) {
        match clear_os_proxy() {
            Ok(()) => {
                self.system_proxy_managed_by_app = false;
                self.refresh_system_proxy_status(None, cx);
            }
            Err(err) => self.refresh_system_proxy_status(Some(format!("{:#}", err)), cx),
        }
    }

    pub(crate) fn refresh_system_proxy_status(
        &mut self,
        fallback_error: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let (enabled, status) = if let Some(msg) = fallback_error {
            (false, format!("Error: {msg}"))
        } else {
            current_system_proxy_state()
        };

        self.system_proxy_enabled = enabled;
        self.system_proxy_status = status;
        cx.notify();
    }

    // === TUN Mode ===

    /// TUN as a whole-app mode switch (the "TUN mode" master toggle).
    ///
    /// TUN is a **mode**, not a per-node action: when it is on, all traffic is
    /// routed through the virtual adapter regardless of which node is selected,
    /// and it stays on across node switches (the outbound is hot-swapped via
    /// `tun_swappable`). It also persists (`tun_was_enabled`) so the mode is
    /// restored on the next launch.
    ///
    /// Turning the mode on brings up whatever it depends on: if the local proxy
    /// is not running yet, this starts it and enables TUN once it is listening,
    /// rather than telling the user to "start proxy first".
    pub(crate) fn set_tun_mode(&mut self, on: bool, cx: &mut Context<Self>) {
        if on {
            if self.tun_enabled || self.tun_starting {
                return;
            }
            if self.proxy_running {
                self.start_tun(cx);
            } else {
                // auto_connect_on_startup starts the proxy and, because
                // restore_tun is true, enables TUN once the proxy is listening.
                self.auto_connect_on_startup(cx, true);
            }
        } else if self.tun_enabled {
            self.stop_tun(cx);
        }
    }

    pub(crate) fn toggle_tun(&mut self, cx: &mut Context<Self>) {
        self.set_tun_mode(!self.tun_enabled && !self.tun_starting, cx);
    }

    pub(crate) fn start_tun(&mut self, cx: &mut Context<Self>) {
        // Guard against re-entry: block if already running OR if the startup
        // task is still in-flight (tun_enabled is false during the ~30 s setup).
        if self.tun_enabled || self.tun_starting {
            return;
        }
        if !self.proxy_running {
            self.tun_status = "â  Start proxy first".to_string();
            cx.notify();
            return;
        }

        // TUN needs Administrator on Windows. When launched unprivileged,
        // relaunch the app through the UAC prompt (`runas`) carrying
        // `--tun-admin-pending`; the elevated instance auto-starts TUN on
        // arrival.
        #[cfg(target_os = "windows")]
        if !sockrocket_core::tun_privileged() {
            tracing::info!("[tun-uac] not privileged â relaunching elevated off the GPUI thread");
            self.tun_status = "ð Waiting for UAC approval...".to_string();
            cx.notify();

            // ShellExecuteW("runas") must NOT run on the GPUI main thread while
            // we are inside this click callback. The callback holds a mutable
            // borrow of `App`; the UAC secure-desktop switch delivers activation
            // /paint messages that re-enter the window procedure, which tries to
            // borrow `App` again â "RefCell already borrowed" â a panic
            // propagating across the `extern "C"` window-proc boundary â abort
            // ("panic in a function that cannot unwind").
            //
            // Running it on a plain OS thread lets the GPUI main thread return
            // from this listener, release the borrow, and go back to its message
            // loop *before* any UAC-induced message arrives â so the re-entrant
            // messages are handled normally instead of fatally.
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let outcome = sockrocket_core::relaunch_elevated(&["--tun-admin-pending"])
                    .map_err(|e| e.to_string());
                let _ = tx.send(outcome);
            });

            // Poll for the UAC result on the foreground executor (main thread,
            // borrow already released) and quit gracefully once approved.
            cx.spawn(async move |weak, cx| {
                for _ in 0..1200 {
                    match rx.try_recv() {
                        Ok(Ok(())) => {
                            tracing::info!("[tun-uac] approved â quitting for elevated instance");
                            weak.update(cx, |this, cx| {
                                this.tun_status =
                                    "ð Approved â restarting as Administrator...".to_string();
                                cx.notify();
                            })
                            .ok();
                            let _ = cx.update(|app: &mut App| app.quit());
                            return;
                        }
                        Ok(Err(msg)) => {
                            tracing::error!("[tun-uac] elevation failed: {msg}");
                            weak.update(cx, |this, cx| {
                                this.tun_status = format!("â Elevation failed: {msg}");
                                cx.notify();
                            })
                            .ok();
                            return;
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => {}
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
                    }
                    Timer::after(std::time::Duration::from_millis(50)).await;
                }
            })
            .detach();
            return;
        }

        // Get proxy server host for loop avoidance. Resolution is deferred to
        // the blocking task below â resolve_to_ips does a blocking DNS lookup
        // and must not run on the UI thread.
        let proxy_server_host: Option<String> = self
            .selected_node
            .and_then(|i| self.nodes.get(i))
            .map(|n| n.server.clone());

        // Build outbound from current state
        let node = self.selected_node.and_then(|i| self.nodes.get(i).cloned());
        let outbound = match &node {
            Some(n) => match create_outbound(n) {
                Ok(o) => o,
                Err(e) => {
                    self.tun_status = format!("Error: {}", e);
                    cx.notify();
                    return;
                }
            },
            None => SharedOutbound::direct(),
        };

        // Wrap with routing mode
        let final_outbound = match self.proxy_mode {
            ProxyMode::Global => outbound,
            ProxyMode::Rule => {
                let ruleset = rule_mode_ruleset(&self.rules);
                let router = std::sync::Arc::new(
                    Router::new(ruleset)
                        .with_geoip(std::sync::Arc::new(sockrocket_core::china_geoip_db())),
                );
                let routing = RoutingOutbound::new(router, outbound);
                SharedOutbound(std::sync::Arc::new(routing))
            }
            ProxyMode::Direct => SharedOutbound::direct(),
        };

        // Wrap in a swappable layer so a later node change can redirect TUN's
        // outbound with `set()` â keeping TUN (and the default route) up â
        // instead of tearing the device down and rebuilding it.
        let tun_swappable = std::sync::Arc::new(SwappableOutbound::new(final_outbound));
        self.tun_swappable = Some(tun_swappable.clone());
        let tun_outbound = SharedOutbound(tun_swappable.clone() as std::sync::Arc<dyn Outbound>);

        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        self.tun_stop_tx = Some(stop_tx);
        self.tun_starting = true;
        self.tun_status = "Preparing TUN driver...".to_string();
        cx.notify();

        let handle = self.tokio_handle.clone();
        cx.spawn(async move |weak, cx| {
            // Step 1: ensure wintun.dll (Windows only, no-op elsewhere)
            if !sockrocket_core::wintun_dll_available() {
                weak.update(cx, |this: &mut AppState, cx| {
                    this.tun_status = "Extracting WinTun driver...".to_string();
                    cx.notify();
                })
                .ok();

                let dll_result = handle.spawn(sockrocket_core::ensure_wintun_dll()).await;
                match dll_result {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => {
                        weak.update(cx, |this: &mut AppState, cx| {
                            this.tun_starting = false;
                            this.tun_stop_tx = None;
                            this.tun_status = format!("â WinTun install failed: {:#}", e);
                            cx.notify();
                        })
                        .ok();
                        return;
                    }
                    Err(e) => {
                        weak.update(cx, |this: &mut AppState, cx| {
                            this.tun_starting = false;
                            this.tun_stop_tx = None;
                            this.tun_status = format!("â WinTun task error: {}", e);
                            cx.notify();
                        })
                        .ok();
                        return;
                    }
                }
            }

            weak.update(cx, |this: &mut AppState, cx| {
                this.tun_status = "Starting TUN...".to_string();
                cx.notify();
            })
            .ok();

            // Step 2: Create TUN device
            let result = handle
                .spawn(async move {
                    // Wrap TUN creation with a timeout to prevent indefinite
                    // blocking. First-time WinTun adapter creation plus route
                    // setup can take a while, so allow a generous window.
                    // NOTE: on timeout the blocking task keeps running in the
                    // background; it cleans up after itself when it finishes.
                    let tun_result = tokio::time::timeout(
                        std::time::Duration::from_secs(30),
                        tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
                            // Resolve the proxy server's IP *before* starting
                            // TUN. setup_tun_routes() points the default route
                            // at the TUN device, and the proxy server IP is
                            // what keeps our own outbound traffic from being
                            // sucked into TUN and deadlocking. Without a
                            // loop-avoidance route the entire system loses
                            // network access, so refuse to start rather than
                            // install a default route we cannot survive.
                            let proxy_server_ips = proxy_server_host
                                .as_deref()
                                .map(resolve_to_ips)
                                .unwrap_or_default();
                            if proxy_server_ips.is_empty() {
                                return Err(anyhow::anyhow!(
                                    "TUN not started: cannot resolve the proxy node's IP address ({}). \
                                     Without a bypass route the proxy's own traffic would be swallowed \
                                     by the TUN loop and kill the whole network, so the start was \
                                     aborted and the current network is unaffected. Pick a resolvable \
                                     node and try again.",
                                    proxy_server_host.as_deref().unwrap_or("no node selected")
                                ));
                            }
                            let tun_proxy = TunProxy::start(tun_outbound, None, None)?;
                            let route_info = tun_proxy.route_info();
                            let route_guard = setup_tun_routes(&route_info, &proxy_server_ips)?;
                            Ok((tun_proxy, route_guard))
                        }),
                    )
                    .await;

                    match tun_result {
                        Ok(Ok(inner)) => inner,
                        Ok(Err(e)) => Err(anyhow::anyhow!("TUN setup task failed: {}", e)),
                        Err(_) => Err(anyhow::anyhow!(
                            "TUN startup timed out after 30s â the driver may still be \
                             initializing in the background; wait a moment and try again."
                        )),
                    }
                })
                .await;

            match result {
                Ok(Ok((tun_proxy, route_guard))) => {
                    weak.update(cx, |this: &mut AppState, cx| {
                        this.tun_starting = false;
                        this.tun_enabled = true;
                        this.tun_status = "â TUN active â all traffic proxied".to_string();
                        // Persist tun_was_enabled=true for the next launch.
                        this.schedule_persist(cx);
                        cx.notify();
                    })
                    .ok();

                    // Hold TUN alive until stop signal
                    let _ = stop_rx.await;

                    // Clean up: restore routes then stop TUN
                    route_guard.restore();
                    tun_proxy.stop().await;

                    weak.update(cx, |this: &mut AppState, cx| {
                        this.tun_enabled = false;
                        this.tun_status = "Disabled".to_string();
                        // Drop the swappable handle: no TUN is running, so
                        // nothing should be redirected through it anymore.
                        this.tun_swappable = None;
                        // Persist tun_was_enabled=false.
                        this.schedule_persist(cx);
                        cx.notify();
                    })
                    .ok();
                }
                Ok(Err(e)) => {
                    weak.update(cx, |this: &mut AppState, cx| {
                        this.tun_starting = false;
                        this.tun_stop_tx = None;
                        this.tun_status = format!("â {:#}", e);
                        cx.notify();
                    })
                    .ok();
                }
                Err(e) => {
                    weak.update(cx, |this: &mut AppState, cx| {
                        this.tun_starting = false;
                        this.tun_stop_tx = None;
                        this.tun_status = format!("â {:#}", e);
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    pub(crate) fn stop_tun(&mut self, cx: &mut Context<Self>) {
        if let Some(tx) = self.tun_stop_tx.take() {
            let _ = tx.send(());
        }
        self.tun_status = "Stopping TUN...".to_string();
        cx.notify();
    }

    // === Node Management ===

    pub(crate) fn delete_node(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.nodes.len() {
            return;
        }
        // Stop proxy if deleting the active node
        if self.proxy_running && self.active_proxy_node == Some(index) {
            self.stop_proxy(cx);
        }
        self.nodes.remove(index);
        // Adjust selected/active indices
        if let Some(sel) = self.selected_node {
            if sel == index {
                self.selected_node = None;
            } else if sel > index {
                self.selected_node = Some(sel - 1);
            }
        }
        if let Some(act) = self.active_proxy_node {
            if act == index {
                self.active_proxy_node = None;
            } else if act > index {
                self.active_proxy_node = Some(act - 1);
            }
        }
        // Remap index-keyed sets: drop the removed index, shift entries above
        // it down by one â otherwise latency spinners/fail marks and batch
        // checkboxes end up attached to the wrong rows.
        for set in [
            &mut self.latency_testing,
            &mut self.latency_failed,
            &mut self.selected_node_indices,
        ] {
            set.remove(&index);
            let remapped = set
                .iter()
                .map(|&i| if i > index { i - 1 } else { i })
                .collect();
            *set = remapped;
        }
        // Same remap for the failure-reason map (keyed by node index too).
        self.latency_fail_reason.remove(&index);
        self.latency_fail_reason = std::mem::take(&mut self.latency_fail_reason)
            .into_iter()
            .map(|(i, k)| (if i > index { i - 1 } else { i }, k))
            .collect();
        self.schedule_persist(cx);
        cx.notify();
    }

    pub(crate) fn delete_all_nodes(&mut self, cx: &mut Context<Self>) {
        if self.nodes.is_empty() {
            return;
        }
        if self.proxy_running {
            self.stop_proxy(cx);
        }
        self.nodes.clear();
        self.selected_node = None;
        self.active_proxy_node = None;
        self.schedule_persist(cx);
        cx.notify();
    }

    pub(crate) fn add_node_from_uri(&mut self, cx: &mut Context<Self>) {
        let uri = self.node_uri_input.read(cx).value().trim().to_string();
        if uri.is_empty() {
            self.import_status = "â  Please paste a proxy share link".to_string();
            cx.notify();
            return;
        }

        // Support pasting multiple URIs separated by newlines
        let uris: Vec<&str> = uri
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();
        let mut added = 0u32;
        let mut errors = Vec::new();
        for u in &uris {
            match parse_proxy_uri(u) {
                Ok(mut node) => {
                    let normalized =
                        sockrocket_core::config::model::decode_display_name(&node.name);
                    node.name = normalized;
                    self.nodes.push(node);
                    added += 1;
                }
                Err(e) => {
                    let short = if u.len() > 30 { &u[..30] } else { u };
                    errors.push(format!("{}: {:#}", short, e));
                }
            }
        }

        self.import_status = if errors.is_empty() {
            format!("â Added {} node(s) from URI", added)
        } else if added > 0 {
            format!(
                "â Added {} node(s), {} failed: {}",
                added,
                errors.len(),
                errors[0]
            )
        } else {
            format!("â Failed: {}", errors[0])
        };

        if added > 0 {
            self.schedule_persist(cx);
        }
        cx.notify();
    }

    pub(crate) fn sort_nodes_by_latency(&mut self, cx: &mut Context<Self>) {
        // Remember selected/active node names before sorting
        let selected_name = self
            .selected_node
            .and_then(|i| self.nodes.get(i))
            .map(|n| n.name.clone());
        let active_name = self
            .active_proxy_node
            .and_then(|i| self.nodes.get(i))
            .map(|n| n.name.clone());

        // Index-keyed probe state (failed marks + reasons, testing spinners)
        // must follow its node to the new position â capture by fingerprint
        // before the sort, rebuild after. (Failed nodes sort to the bottom
        // because their latency is None.)
        let fail_state: Vec<(String, sockrocket_core::ProbeFailureKind)> = self
            .latency_failed
            .iter()
            .filter_map(|&i| {
                self.nodes.get(i).map(|n| {
                    (
                        node_fingerprint(n),
                        self.latency_fail_reason
                            .get(&i)
                            .copied()
                            .unwrap_or(sockrocket_core::ProbeFailureKind::Other),
                    )
                })
            })
            .collect();
        let testing_fps: Vec<String> = self
            .latency_testing
            .iter()
            .filter_map(|&i| self.nodes.get(i))
            .map(node_fingerprint)
            .collect();

        self.nodes
            .sort_by(|a, b| match (a.latency_ms, b.latency_ms) {
                (Some(a_ms), Some(b_ms)) => a_ms.cmp(&b_ms),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            });

        self.latency_failed.clear();
        self.latency_fail_reason.clear();
        self.latency_testing.clear();
        for (i, n) in self.nodes.iter().enumerate() {
            let fp = node_fingerprint(n);
            if let Some((_, kind)) = fail_state.iter().find(|(f, _)| *f == fp) {
                self.latency_failed.insert(i);
                self.latency_fail_reason.insert(i, *kind);
            }
            if testing_fps.contains(&fp) {
                self.latency_testing.insert(i);
            }
        }

        // Restore selected/active indices by name
        if let Some(name) = selected_name {
            self.selected_node = self.nodes.iter().position(|n| n.name == name);
        }
        if let Some(name) = active_name {
            self.active_proxy_node = self.nodes.iter().position(|n| n.name == name);
        }
        // Node order changed: the Nodes page's cached filtered index list
        // points at stale positions until this bumps.
        self.nodes_generation = self.nodes_generation.wrapping_add(1);
        self.schedule_persist(cx);
        cx.notify();
    }

    // === Node Selection ===

    /// Build the outbound for the currently selected node + proxy mode.
    ///
    /// Mirrors the construction in `start_proxy` so node switches can produce
    /// the same outbound without going through a full restart.
    fn build_current_outbound(&self) -> Option<SharedOutbound> {
        let node = self.selected_node.and_then(|i| self.nodes.get(i))?;
        let outbound = match create_outbound(node) {
            Ok(o) => o,
            Err(e) => {
                tracing::warn!("cannot build outbound for {}: {:#}", node.name, e);
                return None;
            }
        };
        Some(match self.proxy_mode {
            ProxyMode::Global => outbound,
            ProxyMode::Rule => apply_proxy_mode(outbound, &ProxyMode::Rule, &self.rules),
            ProxyMode::Direct => SharedOutbound::direct(),
        })
    }

    /// Redirect the running proxy (and TUN, if on) to a new outbound in place.
    ///
    /// Both the local SOCKS/HTTP servers and the TUN proxy hold a swappable
    /// wrapper, so replacing the inner connector takes effect on the next
    /// connection **without** rebinding listeners or tearing down the TUN
    /// device. That is what makes TUN behave as a persistent mode: switching
    /// nodes no longer drops the default route.
    ///
    /// Returns `true` if the live outbound was updated in place.
    fn apply_outbound_hot_swap(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(final_outbound) = self.build_current_outbound() else {
            return false;
        };
        let mut swapped = false;
        if let Some(swappable) = &self.proxy_swappable {
            swappable.set(final_outbound.clone());
            swapped = true;
        }
        if self.tun_enabled {
            if let Some(swappable) = &self.tun_swappable {
                swappable.set(final_outbound.clone());
                swapped = true;
            }
            // CRITICAL: the outbound is now the new node, but TUN's
            // loop-avoidance routes still exempt the *old* node's IP. Without
            // updating them, the new node's traffic is routed into TUN and
            // loops, so the node reports unreachable even though it is fine.
            let node_ips = self
                .selected_node
                .and_then(|i| self.nodes.get(i))
                .map(|n| resolve_to_ips(&n.server))
                .unwrap_or_default();
            if let Err(e) = sockrocket_core::update_tun_bypass_ips(&node_ips) {
                tracing::warn!("failed to update TUN bypass routes on node switch: {:#}", e);
            }
        }
        if swapped {
            self.active_proxy_node = self.selected_node;
            tracing::info!(
                "hot-swapped outbound to {:?} (tun_enabled={})",
                self.selected_node.map(|i| self.nodes[i].name.clone()),
                self.tun_enabled
            );
            cx.notify();
        }
        swapped
    }

    pub(crate) fn select_node(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected_node = Some(index);
        self.schedule_persist(cx);
        cx.notify();
    }

    /// Double-click on a node row: select it and, when the proxy is running,
    /// restart it on the new node (listeners rebound after a short gap).
    pub(crate) fn switch_to_node(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.nodes.len() {
            return;
        }
        self.selected_node = Some(index);
        self.schedule_persist(cx);
        if self.proxy_running {
            // Prefer swapping the live outbound: it keeps the local listeners
            // bound and, crucially, keeps TUN up â a full restart would drop
            // the default route on every node change.
            if !self.apply_outbound_hot_swap(cx) {
                self.restart_proxy_with_current_state(cx);
            }
        } else {
            // Proxy not running: double-click means "use this node now" â
            // start the proxy on it.
            self.start_proxy(cx);
        }
        cx.notify();
    }

    pub(crate) fn set_proxy_mode(&mut self, mode: ProxyMode, cx: &mut Context<Self>) {
        if self.proxy_mode == mode {
            return;
        }
        self.proxy_mode = mode;
        self.schedule_persist(cx);

        // If proxy is running, restart with new mode
        if self.proxy_running {
            self.restart_proxy_with_current_state(cx);
        }
        cx.notify();
    }

    pub(crate) fn cycle_proxy_mode(&mut self, cx: &mut Context<Self>) {
        let next = match self.proxy_mode {
            ProxyMode::Global => ProxyMode::Rule,
            ProxyMode::Rule => ProxyMode::Direct,
            ProxyMode::Direct => ProxyMode::Global,
        };
        self.set_proxy_mode(next, cx);
    }

    pub(crate) fn activate_node(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected_node = Some(index);

        // Already connected to this node â nothing to do.
        if self.proxy_running && self.active_proxy_node == Some(index) {
            cx.notify();
            return;
        }

        if self.proxy_running {
            // Same as switch_to_node: swap in place so TUN stays up.
            if !self.apply_outbound_hot_swap(cx) {
                self.restart_proxy_with_current_state(cx);
            }
            return;
        }

        self.start_proxy(cx);
    }

    pub(crate) fn persist_gui_state(&self) {
        let config = self.snapshot_config();

        // Keep the LAN share server (if running) serving the latest list.
        if let Ok(mut state) = self.share_state.write() {
            state.nodes = self.export_scope_nodes();
        }

        if let Err(err) = save_gui_state(&config, self.config_watch.as_ref()) {
            tracing::error!("failed to persist GUI state: {}", err);
        }
    }

    /// Assemble the on-disk config from current UI state.
    pub(crate) fn snapshot_config(&self) -> AppConfig {
        AppConfig {
            listen_addr: self.listen_addr.clone(),
            socks_port: self.socks_port,
            http_port: self.http_port,
            proxy_mode: self.proxy_mode.clone(),
            nodes: self.nodes.clone(),
            active_node: self.selected_node,
            subscriptions: self.subscriptions.clone(),
            rules: self.rules.clone(),
            auto_connect: self.proxy_running,
            tun_was_enabled: self.tun_enabled,
            health_check: self.health_check.clone(),
            // User-defined proxy groups (Groups page); members are node
            // fingerprints, so the list round-trips through restarts and
            // subscription refreshes.
            groups: self.groups.clone(),
            locale: sockrocket_gui::i18n::current_locale().id().to_string(),
        }
    }

    /// Debounced persist: coalesces multiple rapid calls (e.g. batch latency tests)
    /// into a single disk write 500ms after the last call.
    pub(crate) fn schedule_persist(&mut self, cx: &mut Context<Self>) {
        // Persisted state changed â invalidate the Nodes page filter/sort cache.
        // (Cheap over-invalidation: also fires for rules/subscription edits.)
        self.nodes_generation = self.nodes_generation.wrapping_add(1);
        if self.pending_persist {
            return; // write already queued
        }
        self.pending_persist = true;
        let handle = self.tokio_handle.clone();
        cx.spawn(async move |weak, cx| {
            handle
                .spawn(async {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                })
                .await
                .ok();
            weak.update(cx, |this, _cx| {
                this.pending_persist = false;
                this.persist_gui_state();
            })
            .ok();
        })
        .detach();
    }

    // === Subscription Auto-Refresh ===

    /// Refresh a single subscription: replaces its nodes (by sub_url tag) with fresh ones.
    pub(crate) fn refresh_subscription(&mut self, sub_index: usize, cx: &mut Context<Self>) {
        let Some(sub) = self.subscriptions.get(sub_index).cloned() else {
            return;
        };
        if self.refreshing_subscriptions.contains(&sub_index) {
            return; // already refreshing this one
        }
        self.refreshing_subscriptions.insert(sub_index);
        cx.notify();

        let handle = self.tokio_handle.clone();
        let url = sub.url.clone();

        tracing::info!("Auto-refreshing subscription: {}", sub.name);
        cx.spawn(async move |weak, cx| {
            // Retry up to 3 times with exponential backoff
            let mut last_err = String::new();
            for attempt in 0..3u32 {
                if attempt > 0 {
                    handle
                        .spawn(async move {
                            tokio::time::sleep(std::time::Duration::from_secs(2u64.pow(attempt)))
                                .await;
                        })
                        .await
                        .ok();
                }
                let sub_clone = sub.clone();
                match handle
                    .spawn(async move { fetch_subscription(&sub_clone).await })
                    .await
                {
                    Ok(Ok(mut new_nodes)) => {
                        weak.update(cx, |this: &mut AppState, cx| {
                            normalize_node_names(&mut new_nodes);
                            // Tag new nodes with subscription URL
                            for n in &mut new_nodes {
                                n.extra.insert("sub_url".to_string(), url.clone());
                            }
                            // Save selection/active names before removing old nodes
                            let selected_name = this
                                .selected_node
                                .and_then(|i| this.nodes.get(i))
                                .map(|n| n.name.clone());
                            let active_name = this
                                .active_proxy_node
                                .and_then(|i| this.nodes.get(i))
                                .map(|n| n.name.clone());

                            // Remove stale nodes from this subscription
                            this.nodes.retain(|n| n.extra.get("sub_url") != Some(&url));

                            let count = new_nodes.len();
                            this.nodes.extend(new_nodes);
                            // Cross-subscription cleanup: drop fingerprint-identical
                            // copies, then make display names unique (names key group
                            // membership and the selection restore just below).
                            dedup_nodes(&mut this.nodes);
                            uniquify_node_names(&mut this.nodes);

                            // Restore selection/active by name
                            if let Some(name) = selected_name {
                                this.selected_node = this.nodes.iter().position(|n| n.name == name);
                            }
                            if let Some(name) = active_name {
                                this.active_proxy_node =
                                    this.nodes.iter().position(|n| n.name == name);
                            }

                            // Update last_updated timestamp
                            if let Some(s) = this.subscriptions.get_mut(sub_index) {
                                s.last_updated = Some(
                                    SystemTime::now()
                                        .duration_since(UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_secs(),
                                );
                            }
                            this.refreshing_subscriptions.remove(&sub_index);
                            this.import_status = format!(
                                "â Refreshed '{}': {} nodes",
                                this.subscriptions
                                    .get(sub_index)
                                    .map(|s| s.name.as_str())
                                    .unwrap_or("subscription"),
                                count
                            );
                            this.schedule_persist(cx);
                            cx.notify();
                        })
                        .ok();
                        return;
                    }
                    Ok(Err(e)) => last_err = format!("{:#}", e),
                    Err(e) => last_err = format!("task: {:#}", e),
                }
            }
            tracing::warn!(
                "Auto-refresh of subscription {} failed after 3 attempts: {}",
                sub_index,
                last_err
            );
            // Show failure in the import status bar so the user sees it in the Config tab.
            weak.update(cx, |this: &mut AppState, cx| {
                let name = this
                    .subscriptions
                    .get(sub_index)
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| "subscription".to_string());
                this.refreshing_subscriptions.remove(&sub_index);
                this.import_status = format!("â  Refresh failed for '{}': {}", name, last_err);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Refresh all subscriptions that are past their refresh_interval_hours.
    pub(crate) fn check_and_refresh_stale(&mut self, cx: &mut Context<Self>) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let stale: Vec<usize> = self
            .subscriptions
            .iter()
            .enumerate()
            .filter(|(_, sub)| {
                sub.refresh_interval_hours > 0 && {
                    let threshold = sub.refresh_interval_hours * 3600;
                    match sub.last_updated {
                        None => true,
                        Some(ts) => now.saturating_sub(ts) >= threshold,
                    }
                }
            })
            .map(|(i, _)| i)
            .collect();

        for i in stale {
            self.refresh_subscription(i, cx);
        }
    }

    /// Start the periodic auto-refresh background task.
    /// Called once at startup; checks for stale subscriptions every 30 minutes.
    pub(crate) fn auto_connect_on_startup(&mut self, cx: &mut Context<Self>, restore_tun: bool) {
        let handle = self.tokio_handle.clone();
        cx.spawn(async move |weak, cx| {
            // Short delay to let the UI fully initialize before connecting.
            handle
                .spawn(async {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                })
                .await
                .ok();

            let proxy_started = weak
                .update(cx, |this: &mut AppState, cx| {
                    tracing::info!("Auto-reconnecting to last active node...");
                    this.start_proxy(cx);
                    true
                })
                .unwrap_or(false);

            if !restore_tun || !proxy_started {
                return;
            }

            // Wait for proxy to finish starting before re-enabling TUN.
            // See the note in restart_proxy_with_current_state: the old TUN
            // needs to finish tearing down (routes restored + wintun adapter
            // closed) before a new one can be built, otherwise the new node's
            // IP is missing from the bypass routes. Allow 20s.
            for _ in 0..133 {
                handle
                    .spawn(async {
                        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                    })
                    .await
                    .ok();

                let tun_started = weak
                    .update(cx, |this: &mut AppState, cx| {
                        if this.tun_enabled || !this.proxy_running {
                            false
                        } else {
                            tracing::info!("Restoring TUN mode after auto-reconnect...");
                            // No window handle available here; only reachable
                            // when privileged, so start_tun's UAC path (the
                            // only `window` user) is never taken.
                            this.start_tun(cx);
                            true
                        }
                    })
                    .unwrap_or(false);

                if tun_started {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn start_auto_refresh(&mut self, cx: &mut Context<Self>) {
        let handle = self.tokio_handle.clone();
        cx.spawn(async move |weak, cx| {
            // Brief delay to let the UI initialize before the first refresh.
            // Must run inside the tokio runtime (via handle.spawn) because
            // gpui's own executor does not provide a tokio reactor.
            handle
                .spawn(async {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                })
                .await
                .ok();
            // Initial stale check on startup
            if let Ok(()) = weak.update(cx, |this, cx| {
                this.check_and_refresh_stale(cx);
            }) {}
            // Then check every 30 minutes
            loop {
                handle
                    .spawn(async {
                        tokio::time::sleep(std::time::Duration::from_secs(1800)).await;
                    })
                    .await
                    .ok();
                match weak.update(cx, |this, cx| {
                    this.check_and_refresh_stale(cx);
                }) {
                    Ok(_) => {}
                    Err(_) => break, // entity dropped, stop the loop
                }
            }
        })
        .detach();
    }

    // === Config File Hot-Reload ===

    /// Watch gui-state.yaml for external edits and apply them smoothly.
    /// Our own saves are suppressed via content hashing (see save_gui_state).
    pub(crate) fn start_config_watch(&mut self, cx: &mut Context<Self>) {
        let Some(path) = gui_state_path() else {
            return;
        };
        if !path.exists() {
            return;
        }
        let current = self.snapshot_config();
        // Must run inside the tokio runtime context: ConfigWatcher::spawn uses
        // tokio::spawn internally, and gpui's executor provides no reactor.
        let mut watcher = {
            let _guard = self.tokio_handle.enter();
            ConfigWatcher::spawn(&path, &current, std::time::Duration::from_millis(2500))
        };
        self.config_watch = Some(watcher.handle());
        cx.spawn(async move |weak, cx| {
            loop {
                let Some(event) = watcher.next().await else {
                    break; // watcher shut down
                };
                if weak
                    .update(cx, |this: &mut AppState, cx| {
                        this.apply_config_watch_event(event, cx);
                    })
                    .is_err()
                {
                    break; // entity dropped
                }
            }
        })
        .detach();
    }

    pub(crate) fn apply_config_watch_event(
        &mut self,
        event: ConfigWatchEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            ConfigWatchEvent::ParseError(error) => {
                // Bad edit on disk: keep running with the current config.
                self.settings_status =
                    format!("â  Config file error (kept current settings): {}", error);
                cx.notify();
            }
            ConfigWatchEvent::Reloaded(reloaded) => {
                let new_config = reloaded.config;
                let restart_fields = reloaded.requires_restart.clone();

                // The node list is replaced wholesale: index-keyed state
                // (latency spinners/fail marks, batch checkboxes) would point
                // at the wrong rows â clear it, and restore the selection by
                // name (falling back to the file's active_node).
                let prev_selected_name = self
                    .selected_node
                    .and_then(|i| self.nodes.get(i))
                    .map(|n| n.name.clone());
                self.latency_testing.clear();
                self.latency_failed.clear();
                self.latency_fail_reason.clear();
                self.selected_node_indices.clear();
                self.pending_latency_batch = 0;
                self.auto_select_best = false;
                self.auto_select_scope = None;
                self.group_test_pending = None;

                // Runtime-changeable parts take effect immediately.
                self.nodes = new_config.nodes.clone();
                // The node list changed wholesale â invalidate the Nodes page
                // filter/sort cache (keyed on this counter).
                self.nodes_generation = self.nodes_generation.wrapping_add(1);
                self.subscriptions = new_config.subscriptions.clone();
                self.rules = new_config.rules.clone();
                self.proxy_mode = new_config.proxy_mode.clone();
                self.health_check = new_config.health_check.clone();
                // External group edits apply immediately; clamp index-keyed
                // Groups page UI state against the new list.
                self.groups = new_config.groups.clone();
                if self
                    .editing_group_index
                    .is_some_and(|i| i >= self.groups.len())
                {
                    self.editing_group_index = None;
                }
                if self.group_add_open.is_some_and(|i| i >= self.groups.len()) {
                    self.group_add_open = None;
                }
                if self
                    .group_delete_armed
                    .is_some_and(|i| i >= self.groups.len())
                {
                    self.group_delete_armed = None;
                }
                self.selected_node = new_config
                    .active_node
                    .filter(|index| *index < new_config.nodes.len())
                    .or_else(|| {
                        prev_selected_name
                            .and_then(|name| self.nodes.iter().position(|n| n.name == name))
                    });

                if restart_fields.is_empty() {
                    self.settings_status = "â Config reloaded from disk".to_string();
                } else {
                    // Listen address/ports cannot change while listeners are
                    // bound â adopt the values for the next start and say so.
                    self.listen_addr = new_config.listen_addr.clone();
                    self.socks_port = new_config.socks_port;
                    self.http_port = new_config.http_port;
                    self.settings_status = format!(
                        "â  {} changed in config file â restart proxy to apply",
                        restart_fields.join(", ")
                    );
                }

                if self.proxy_running && restart_fields.is_empty() {
                    if self.health_check.enabled {
                        // The health monitor holds a node/rules snapshot;
                        // restart the proxy so it rebuilds from fresh state.
                        self.restart_proxy_with_current_state(cx);
                    } else {
                        // Hot-swap the outbound without touching listeners.
                        let new_outbound = self
                            .selected_node
                            .and_then(|index| self.nodes.get(index))
                            .and_then(|node| create_outbound(node).ok())
                            .map(|base| apply_proxy_mode(base, &self.proxy_mode, &self.rules));
                        if let (Some(swappable), Some(outbound)) =
                            (self.proxy_swappable.clone(), new_outbound)
                        {
                            swappable.set(outbound);
                            self.active_proxy_node = self.selected_node;
                        }
                    }
                }
                cx.notify();
            }
        }
    }

    // === Health Check Settings ===

    pub(crate) fn toggle_health_check(&mut self, cx: &mut Context<Self>) {
        self.health_check.enabled = !self.health_check.enabled;
        self.health_config_changed(cx);
    }

    pub(crate) fn toggle_health_auto_switch(&mut self, cx: &mut Context<Self>) {
        self.health_check.auto_switch = !self.health_check.auto_switch;
        self.health_config_changed(cx);
    }

    pub(crate) fn cycle_health_interval(&mut self, cx: &mut Context<Self>) {
        const VALUES: [u64; 4] = [30, 60, 120, 300];
        let current = self.health_check.interval_secs;
        let next = VALUES
            .iter()
            .copied()
            .find(|v| *v > current)
            .unwrap_or(VALUES[0]);
        self.health_check.interval_secs = next;
        self.health_config_changed(cx);
    }

    pub(crate) fn cycle_health_threshold(&mut self, cx: &mut Context<Self>) {
        const VALUES: [u32; 4] = [1, 2, 3, 5];
        let current = self.health_check.failure_threshold;
        let next = VALUES
            .iter()
            .copied()
            .find(|v| *v > current)
            .unwrap_or(VALUES[0]);
        self.health_check.failure_threshold = next;
        self.health_config_changed(cx);
    }

    pub(crate) fn health_config_changed(&mut self, cx: &mut Context<Self>) {
        self.schedule_persist(cx);
        // The monitor takes its config at spawn time â restart the proxy so
        // the running monitor picks up the new settings.
        if self.proxy_running {
            self.restart_proxy_with_current_state(cx);
        }
        cx.notify();
    }
}

// === Render ===

impl Render for AppState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("app-root")
            .key_context("AppState")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG_APP))
            .font(ui_font())
            .text_color(rgb(TEXT_PRIMARY))
            .on_action(cx.listener(|this, _: &ToggleProxy, _, cx| {
                this.toggle_proxy(cx);
            }))
            .on_action(cx.listener(|this, _: &SwitchToHome, _, cx| {
                this.set_view(ActiveView::Home, cx);
            }))
            .on_action(cx.listener(|this, _: &SwitchToNodes, _, cx| {
                this.set_view(ActiveView::Nodes, cx);
            }))
            .on_action(cx.listener(|this, _: &SwitchToConfig, _, cx| {
                this.set_view(ActiveView::Config, cx);
            }))
            .on_action(cx.listener(|this, _: &SwitchToGroups, _, cx| {
                this.set_view(ActiveView::Groups, cx);
            }))
            .on_action(cx.listener(|this, _: &SwitchToConnections, _, cx| {
                this.set_view(ActiveView::Connections, cx);
            }))
            .on_action(cx.listener(|this, _: &SwitchToRules, _, cx| {
                this.set_view(ActiveView::Rules, cx);
            }))
            .on_action(cx.listener(|this, _: &SwitchToLogs, _, cx| {
                this.set_view(ActiveView::Logs, cx);
            }))
            .on_action(cx.listener(|this, _: &SwitchToSettings, _, cx| {
                this.set_view(ActiveView::Settings, cx);
            }))
            .on_action(cx.listener(|this, _: &TestAllLatency, _, cx| {
                this.test_all_latency(cx);
            }))
            .on_action(cx.listener(|this, _: &CycleProxyMode, _, cx| {
                this.cycle_proxy_mode(cx);
            }))
            .on_action(cx.listener(|this, _: &OpenCommandPalette, window, cx| {
                this.open_palette(window, cx);
            }))
            .on_action(cx.listener(|this, _: &CloseCommandPalette, _, cx| {
                this.close_palette(cx);
            }))
            .on_action(cx.listener(|this, _: &PaletteUp, _, cx| {
                this.palette_move(-1, cx);
            }))
            .on_action(cx.listener(|this, _: &PaletteDown, _, cx| {
                this.palette_move(1, cx);
            }))
            .child(self.render_titlebar(window, cx))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_row()
                    .overflow_hidden()
                    .child(self.render_sidebar(cx))
                    .child(self.render_content(cx)),
            )
            .child(self.render_statusbar(cx))
            // Ctrl+K command palette overlay (top-most layer)
            .child(self.render_palette(cx))
    }
}

// === Content Router ===

impl AppState {
    pub(crate) fn render_content(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_view.clone();
        // v2: content-area p-4; dashboard centered max-w-4xl, settings max-w-2xl,
        // every other page fills the width. Nodes/Connections/Logs fill the
        // viewport height so their tables scroll internally.
        // Only centered pages constrain their width (v2: dashboard max-w-4xl,
        // settings max-w-2xl); other pages fill the width. An f32::MAX max_w
        // here breaks flex shrink, so it is applied conditionally instead.
        let centered = matches!(active, ActiveView::Home | ActiveView::Settings);
        // The Nodes table is virtualized (uniform_list) and scrolls
        // internally, which requires a definite height. Children of a
        // page-level scroll container get unbounded height and the list
        // collapses to zero rows â so the Nodes page opts out of page
        // scrolling. (Other table pages size to content + page-scroll.)
        let virtualized_page = matches!(active, ActiveView::Nodes);
        let max_w = match active {
            ActiveView::Home => px(896.0),
            ActiveView::Settings => px(672.0),
            _ => px(0.0),
        };
        div()
            .id("main-content")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_x_hidden()
            .when(!virtualized_page, |d| d.overflow_y_scroll())
            .when(virtualized_page, |d| d.overflow_y_hidden())
            .p_4()
            .child(
                div()
                    .w_full()
                    .when(centered, |d| d.max_w(max_w).mx_auto())
                    // Full-height pages scroll their inner lists instead of
                    // scrolling the whole page.
                    .when(
                        matches!(
                            active,
                            ActiveView::Home
                                | ActiveView::Nodes
                                | ActiveView::Connections
                                | ActiveView::Logs
                        ),
                        |d| d.h_full(),
                    )
                    .child(match active {
                        ActiveView::Home => self.render_home(cx).into_any_element(),
                        ActiveView::Nodes => self.render_nodes(cx).into_any_element(),
                        ActiveView::Groups => self.render_groups(cx).into_any_element(),
                        ActiveView::Connections => self.render_connections(cx).into_any_element(),
                        ActiveView::Config => self.render_config(cx).into_any_element(),
                        ActiveView::Rules => self.render_rules(cx).into_any_element(),
                        ActiveView::Settings => self.render_settings(cx).into_any_element(),
                        ActiveView::Logs => self.render_logs(cx).into_any_element(),
                    }),
            )
    }
}

// === Shared view helpers (used by multiple pages) ===

impl AppState {
    /// Page header title: PAGE_TITLE, semibold, primary text.
    pub(crate) fn page_header(&self, title: &str) -> Div {
        div().pb_1().mb_1().child(
            div()
                .text_size(px(PAGE_TITLE))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(TEXT_PRIMARY))
                .child(title.to_string()),
        )
    }

    /// v3 page header: 28px tinted icon square + title + micro muted subtitle.
    pub(crate) fn page_header_v3(
        &self,
        icon_path: &str,
        icon_color: u32,
        title: &str,
        subtitle: &str,
    ) -> Div {
        div()
            .pb_1()
            .mb_1()
            .flex()
            .flex_row()
            .items_center()
            .gap_2p5()
            .child(
                div()
                    .w(px(32.0))
                    .h(px(32.0))
                    .rounded(px(8.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgba(with_alpha(icon_color, 0x1a)))
                    .border_1()
                    .border_color(rgba(with_alpha(icon_color, 0x33)))
                    .child(
                        Icon::empty()
                            .path(SharedString::from(icon_path.to_string()))
                            .with_size(ComponentSize::Size(px(16.0)))
                            .text_color(rgb(icon_color)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .child(
                        div()
                            .text_size(px(PAGE_TITLE))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(TEXT_PRIMARY))
                            .child(title.to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(MICRO))
                            .font_family(MONO_FONT)
                            .text_color(rgb(TEXT_MUTED))
                            .child(subtitle.to_string()),
                    ),
            )
    }
}

// === Subscription management (Config page actions) ===

impl AppState {
    pub(crate) fn delete_subscription(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.subscriptions.len() {
            return;
        }
        let url = self.subscriptions[index].url.clone();
        // Node indices shift after removal â remember selection by name.
        let selected_name = self
            .selected_node
            .and_then(|i| self.nodes.get(i))
            .map(|n| n.name.clone());
        let active_name = self
            .active_proxy_node
            .and_then(|i| self.nodes.get(i))
            .map(|n| n.name.clone());

        self.subscriptions.remove(index);
        // Remove nodes tagged with this subscription URL
        self.nodes.retain(|n| n.extra.get("sub_url") != Some(&url));

        // Index-keyed state is invalid after the reshuffle.
        self.latency_testing.clear();
        self.latency_failed.clear();
        self.latency_fail_reason.clear();
        self.selected_node_indices.clear();
        self.pending_latency_batch = 0;
        self.auto_select_best = false;
        self.auto_select_scope = None;
        self.group_test_pending = None;
        // Restore indices by name
        self.selected_node =
            selected_name.and_then(|name| self.nodes.iter().position(|n| n.name == name));
        self.active_proxy_node =
            active_name.and_then(|name| self.nodes.iter().position(|n| n.name == name));
        self.schedule_persist(cx);
        cx.notify();
    }

    /// Cycle through common refresh intervals: 24h â 12h â 6h â off (0) â 24h
    pub(crate) fn cycle_refresh_interval(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(sub) = self.subscriptions.get_mut(index) {
            sub.refresh_interval_hours = match sub.refresh_interval_hours {
                0 => 1,
                1 => 2,
                2 => 6,
                6 => 12,
                12 => 24,
                _ => 0, // 24 or any custom â Off
            };
        }
        self.schedule_persist(cx);
        cx.notify();
    }

    pub(crate) fn format_last_updated(ts: Option<u64>) -> String {
        let Some(ts) = ts else {
            return "Never".to_string();
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let age = now.saturating_sub(ts);
        if age < 60 {
            "Just now".to_string()
        } else if age < 3600 {
            format!("{}m ago", age / 60)
        } else if age < 86400 {
            format!("{}h ago", age / 3600)
        } else {
            format!("{}d ago", age / 86400)
        }
    }
}

// === Rule actions (Rules page business logic) ===

impl AppState {
    pub(crate) fn add_rule(&mut self, cx: &mut Context<Self>) {
        let rule_type = self.rule_type_sel.clone();
        let pattern = self.rule_pattern_input.read(cx).value().to_string();
        let target = self.rule_target_sel.clone();

        if pattern.trim().is_empty() {
            self.rules_status = "â  Pattern is required".to_string();
            cx.notify();
            return;
        }

        let new_rule = RoutingRule {
            rule_type: rule_type.trim().to_string(),
            pattern: pattern.trim().to_string(),
            target: target.trim().to_string(),
            enabled: true,
        };

        let action_msg = if let Some(edit_idx) = self.editing_rule_index.take() {
            if edit_idx < self.rules.len() {
                self.rules[edit_idx] = new_rule;
                "â Rule updated"
            } else {
                self.rules.push(new_rule);
                "â Rule added"
            }
        } else {
            self.rules.push(new_rule);
            "â Rule added"
        };

        self.rules_status = if self.proxy_running && self.proxy_mode == ProxyMode::Rule {
            format!("{}. Restarting Rule mode to apply changes.", action_msg)
        } else {
            action_msg.to_string()
        };
        self.schedule_persist(cx);
        if self.proxy_running && self.proxy_mode == ProxyMode::Rule {
            self.restart_proxy_with_current_state(cx);
        }
        cx.notify();
    }

    pub(crate) fn start_edit_rule(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.rules.len() {
            return;
        }
        let rule = self.rules[index].clone();
        // Set button-group selectors
        self.rule_type_sel = rule.rule_type.clone();
        self.rule_target_sel = rule.target.clone();
        self.rule_pattern_input.update(cx, |state, cx| {
            state.set_value(rule.pattern, window, cx);
        });
        self.editing_rule_index = Some(index);
        self.rules_status = format!("Editing rule #{}", index + 1);
        cx.notify();
    }

    pub(crate) fn cancel_edit_rule(&mut self, cx: &mut Context<Self>) {
        self.editing_rule_index = None;
        self.rules_status = String::new();
        cx.notify();
    }

    pub(crate) fn delete_rule(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.rules.len() {
            self.rules.remove(index);
            self.rules_status = if self.proxy_running && self.proxy_mode == ProxyMode::Rule {
                "â Rule removed. Restarting Rule mode to apply changes.".to_string()
            } else {
                "â Rule removed".to_string()
            };
            self.schedule_persist(cx);
            if self.proxy_running && self.proxy_mode == ProxyMode::Rule {
                self.restart_proxy_with_current_state(cx);
            }
            cx.notify();
        }
    }

    pub(crate) fn move_rule_up(&mut self, index: usize, cx: &mut Context<Self>) {
        if index > 0 && index < self.rules.len() {
            self.rules.swap(index, index - 1);
            self.rules_status = if self.proxy_running && self.proxy_mode == ProxyMode::Rule {
                "â Rule order updated. Restarting Rule mode to apply changes.".to_string()
            } else {
                "â Rule moved".to_string()
            };
            self.schedule_persist(cx);
            if self.proxy_running && self.proxy_mode == ProxyMode::Rule {
                self.restart_proxy_with_current_state(cx);
            }
            cx.notify();
        }
    }

    pub(crate) fn move_rule_down(&mut self, index: usize, cx: &mut Context<Self>) {
        if index + 1 < self.rules.len() {
            self.rules.swap(index, index + 1);
            self.rules_status = if self.proxy_running && self.proxy_mode == ProxyMode::Rule {
                "â Rule order updated. Restarting Rule mode to apply changes.".to_string()
            } else {
                "â Rule moved".to_string()
            };
            self.schedule_persist(cx);
            if self.proxy_running && self.proxy_mode == ProxyMode::Rule {
                self.restart_proxy_with_current_state(cx);
            }
            cx.notify();
        }
    }

    pub(crate) fn load_china_rules(&mut self, cx: &mut Context<Self>) {
        // Add the built-in China direct ruleset as explicit rules
        let china_rules = vec![
            RoutingRule {
                rule_type: "geoip".into(),
                pattern: "CN".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "cn".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "baidu.com".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "qq.com".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "taobao.com".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "aliyun.com".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "jd.com".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "163.com".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "bilibili.com".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "domain-suffix".into(),
                pattern: "zhihu.com".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "ip-cidr".into(),
                pattern: "10.0.0.0/8".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "ip-cidr".into(),
                pattern: "172.16.0.0/12".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "ip-cidr".into(),
                pattern: "192.168.0.0/16".into(),
                target: "direct".into(),
                enabled: true,
            },
            RoutingRule {
                rule_type: "match".into(),
                pattern: "*".into(),
                target: "proxy".into(),
                enabled: true,
            },
        ];
        self.rules = china_rules;
        self.rules_status = if self.proxy_running && self.proxy_mode == ProxyMode::Rule {
            "â China Direct preset loaded. Restarting Rule mode to apply changes.".to_string()
        } else {
            "â China Direct preset loaded".to_string()
        };
        self.schedule_persist(cx);
        if self.proxy_running && self.proxy_mode == ProxyMode::Rule {
            self.restart_proxy_with_current_state(cx);
        }
        cx.notify();
    }

    pub(crate) fn clear_rules(&mut self, cx: &mut Context<Self>) {
        self.rules.clear();
        self.rules_status = if self.proxy_running && self.proxy_mode == ProxyMode::Rule {
            "â Custom rules cleared. Falling back to built-in China Direct rules.".to_string()
        } else {
            "â Custom rules cleared".to_string()
        };
        self.schedule_persist(cx);
        if self.proxy_running && self.proxy_mode == ProxyMode::Rule {
            self.restart_proxy_with_current_state(cx);
        }
        cx.notify();
    }
}

// === Helpers ===

pub(crate) fn format_speed(bps: f64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * KB;
    const GB: f64 = 1024.0 * MB;
    if bps >= GB {
        format!("{:.1} GB/s", bps / GB)
    } else if bps >= MB {
        format!("{:.1} MB/s", bps / MB)
    } else if bps >= KB {
        format!("{:.1} KB/s", bps / KB)
    } else {
        format!("{:.0} B/s", bps)
    }
}

/// Human-readable byte totals (KB/MB/GB), mono column friendly.
pub(crate) fn format_bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let f = n as f64;
    if f >= GB {
        format!("{:.1} GB", f / GB)
    } else if f >= MB {
        format!("{:.1} MB", f / MB)
    } else if f >= KB {
        format!("{:.1} KB", f / KB)
    } else {
        format!("{} B", n)
    }
}

pub(crate) fn ui_font() -> Font {
    // v2 design typeface: Inter (embedded, see main.rs), with CJK-capable
    // system fallbacks so Chinese text still renders.
    let mut font = font("Inter");
    // Windows-only fallbacks (gpui logs an error for every name that does not
    // resolve, so keep this list to fonts that actually ship with Windows).
    font.fallbacks = Some(FontFallbacks::from_fonts(vec![
        "Microsoft YaHei UI".to_string(),
        "Microsoft YaHei".to_string(),
        "DengXian".to_string(),
        "Segoe UI".to_string(),
    ]));
    font
}

pub(crate) fn localized_font() -> Font {
    ui_font()
}

pub(crate) async fn verify_local_http_proxy(
    listen_addr: &str,
    http_port: u16,
    timeout: std::time::Duration,
    direct_mode: bool,
) -> anyhow::Result<String> {
    // Brief startup delay: let the proxy finish route table setup and any
    // per-protocol handshaking before we start probing.  800ms is enough for
    // the local listener to be ready; further latency is absorbed by the probe
    // timeout itself.
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;

    // Quick liveness check: verify the local port is accepting connections
    // before sending any external CONNECT requests.  This avoids wasting the
    // full probe timeout when the proxy failed to bind.
    let local_ok = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::net::TcpStream::connect((listen_addr, http_port)),
    )
    .await;
    if !matches!(local_ok, Ok(Ok(_))) {
        anyhow::bail!(
            "local proxy not accepting connections on {}:{}",
            listen_addr,
            http_port
        );
    }

    // Direct mode has no remote node: the local relay forwarding straight to
    // the internet IS the whole outbound. Probing gstatic/cloudflare/dns.google
    // is meaningless here â on a censored network they are unreachable directly,
    // which previously made Direct mode always fail validation and left the
    // system proxy unset ("cannot access anything"). The liveness check above
    // is all the confidence Direct mode needs.
    if direct_mode {
        return Ok("Direct relay responding".to_string());
    }

    // Per-probe timeout: use a shorter first-attempt timeout so we can
    // iterate to the retry faster when the node is temporarily slow.
    let first_timeout = std::cmp::min(timeout, std::time::Duration::from_secs(5));

    // Three well-known CONNECT-friendly endpoints.  They are probed in
    // parallel; a single success is sufficient to declare the proxy healthy.
    let mut errors = Vec::new();

    for attempt in 1..=2u32 {
        let probe_timeout = if attempt == 1 { first_timeout } else { timeout };
        let (r1, r2, r3) = tokio::join!(
            verify_local_http_proxy_once(listen_addr, http_port, probe_timeout, "www.gstatic.com",),
            verify_local_http_proxy_once(
                listen_addr,
                http_port,
                probe_timeout,
                "cp.cloudflare.com",
            ),
            verify_local_http_proxy_once(listen_addr, http_port, probe_timeout, "dns.google",),
        );
        match (r1, r2, r3) {
            // Any single success counts: proxy is reachable.
            (Ok(s), _, _) | (_, Ok(s), _) | (_, _, Ok(s)) => {
                return if attempt == 1 {
                    Ok(format!("Connected â {}", s))
                } else {
                    Ok(format!("Connected â {} (after retry)", s))
                };
            }
            (Err(e1), Err(e2), Err(e3)) => {
                errors.push(format!(
                    "attempt {}: gstatic: {}; cloudflare: {}; dns.google: {}",
                    attempt, e1, e2, e3
                ));
                if attempt < 2 {
                    // Wait briefly before retrying â lets transient proxy
                    // warm-up issues resolve without a long global timeout.
                    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                }
            }
        }
    }

    anyhow::bail!(
        "internet reachability check failed after retries: {}",
        errors.join(" | ")
    );
}

pub(crate) async fn verify_local_http_proxy_once(
    listen_addr: &str,
    http_port: u16,
    timeout: std::time::Duration,
    target_host: &str,
) -> anyhow::Result<String> {
    // Use a short timeout for the local TCP connect (proxy should be on localhost)
    let connect_timeout = std::cmp::min(timeout, std::time::Duration::from_secs(2));
    let mut stream = tokio::time::timeout(
        connect_timeout,
        tokio::net::TcpStream::connect((listen_addr, http_port)),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "local proxy not responding on {}:{}",
            listen_addr,
            http_port
        )
    })??;

    let connect_request = format!(
        "CONNECT {0}:443 HTTP/1.1\r\nHost: {0}:443\r\nProxy-Connection: keep-alive\r\n\r\n",
        target_host
    );
    stream.write_all(connect_request.as_bytes()).await?;

    let mut buf = vec![0u8; 4096];
    let read = tokio::time::timeout(timeout, stream.read(&mut buf))
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "proxy did not respond within {}s â node may be unreachable or slow",
                timeout.as_secs()
            )
        })??;
    if read == 0 {
        anyhow::bail!("proxy closed connection without a response");
    }

    let response = String::from_utf8_lossy(&buf[..read]);
    let status_line = response
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    if !status_line.starts_with("HTTP/1.") {
        anyhow::bail!("unexpected proxy response: {}", status_line);
    }
    // Accept "200" anywhere after the HTTP version â some proxies omit
    // the trailing space (e.g. "HTTP/1.1 200" without a reason phrase).
    let code = status_line.split(' ').nth(1).unwrap_or_default().trim();
    if code != "200" {
        anyhow::bail!("tunnel establishment failed: {}", status_line);
    }

    // CONNECT 200 means the local proxy successfully connected to the upstream
    // node and established a tunnel.  That is sufficient proof the proxy is
    // routing traffic.  A full TLS handshake through the tunnel is skipped
    // because many VPN nodes perform TLS interception or have stricter TLS
    // policies for outbound verification probes, causing false negatives.
    Ok(format!("tunnel established via {}", target_host))
}

/// One-shot migrate of `gui-state.yaml` from a legacy Windows path
/// (`HOME/.config/sockrocket/`) into the platform-correct location when the
/// new path is missing.
fn migrate_legacy_gui_state() {
    let Some(new_path) = gui_state_path() else {
        return;
    };
    if new_path.exists() {
        return;
    }
    if let Some(parent) = new_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    if cfg!(target_os = "windows")
        && let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
    {
        candidates.push(
            home.join(".config")
                .join("sockrocket")
                .join("gui-state.yaml"),
        );
    }

    for old_path in candidates {
        if old_path == new_path || !old_path.exists() {
            continue;
        }
        match std::fs::copy(&old_path, &new_path) {
            Ok(_) => {
                tracing::info!(
                    "migrated gui-state.yaml from {} to {}",
                    old_path.display(),
                    new_path.display()
                );
                return;
            }
            Err(e) => tracing::warn!("failed to migrate gui-state.yaml: {e}"),
        }
    }
}

pub(crate) fn load_gui_state() -> Option<AppConfig> {
    migrate_legacy_gui_state();
    let path = gui_state_path()?;
    let content = std::fs::read_to_string(path).ok()?;
    serde_yaml::from_str(&content).ok()
}

/// Startup self-heal: if the OS system proxy still points at this app's local
/// ports, it is a leftover from a previous session (crash/kill skipped the
/// exit cleanup) â our proxy is never running this early, so clear it before
/// the dead listener breaks all browser traffic.
fn recover_stale_system_proxy(socks_port: u16, http_port: u16) {
    if !system_proxy_supported() {
        return;
    }
    let Ok(proxy) = get_os_proxy() else {
        return;
    };
    let points_at_us = proxy.enabled
        && (proxy.host == "127.0.0.1" || proxy.host.eq_ignore_ascii_case("localhost"))
        && (proxy.port == socks_port || proxy.port == http_port);
    if !points_at_us {
        return;
    }
    match clear_os_proxy() {
        Ok(()) => tracing::warn!(
            "startup: cleared stale system proxy 127.0.0.1:{} left by a previous session",
            proxy.port
        ),
        Err(err) => tracing::error!("startup: failed to clear stale system proxy: {:#}", err),
    }
}

pub(crate) fn save_gui_state(
    config: &AppConfig,
    watch: Option<&ConfigWatchHandle>,
) -> anyhow::Result<()> {
    let path = gui_state_path().ok_or_else(|| anyhow::anyhow!("no GUI config directory found"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_yaml::to_string(config)?;
    // Suppress the hot-reload event for our own write. Noted *before*
    // writing so the poller can never observe the new mtime first.
    if let Some(handle) = watch {
        handle.note_saved(config, config_content_hash(&content));
    }
    std::fs::write(path, content)?;
    Ok(())
}

/// Directory holding `gui-state.yaml` and other persistent app data.
///
/// Resolution is **platform-first**:
/// - Windows -> %APPDATA% (fallback HOME if APPDATA is missing)
/// - macOS   -> ~/Library/Application Support
/// - Linux   -> $XDG_CONFIG_HOME or ~/.config
pub(crate) fn gui_state_path() -> Option<PathBuf> {
    let base: PathBuf = if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))?
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
        })?
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?
    };
    Some(base.join("sockrocket").join("gui-state.yaml"))
}

pub(crate) fn protocol_name(protocol: &ProxyProtocol) -> &'static str {
    match protocol {
        ProxyProtocol::Shadowsocks { .. } => "Shadowsocks",
        ProxyProtocol::VMess { .. } => "VMess",
        ProxyProtocol::VLess { .. } => "VLESS",
        ProxyProtocol::Tuic { .. } => "TUIC v5",
        ProxyProtocol::Trojan { .. } => "Trojan",
        ProxyProtocol::Hysteria2 { .. } => "Hysteria2",
    }
}

pub(crate) fn protocol_short_name(protocol: &ProxyProtocol) -> &'static str {
    match protocol {
        ProxyProtocol::Shadowsocks { .. } => "ss",
        ProxyProtocol::VMess { .. } => "vmess",
        ProxyProtocol::VLess { .. } => "vless",
        ProxyProtocol::Tuic { .. } => "tuic",
        ProxyProtocol::Trojan { .. } => "trojan",
        ProxyProtocol::Hysteria2 { .. } => "hy2",
    }
}

pub(crate) fn transport_summary(transport: Option<&TransportConfig>) -> String {
    match transport {
        None => "TCP".to_string(),
        Some(transport) => {
            let mode = match transport.transport_type {
                TransportType::Tcp => "TCP",
                TransportType::Tls => "TLS",
                TransportType::WebSocket => "WebSocket",
                TransportType::Quic => "QUIC",
                TransportType::Reality => "Reality",
            };

            let mut parts = vec![mode.to_string()];
            if let Some(tls) = &transport.tls
                && let Some(sni) = &tls.sni
            {
                parts.push(format!("SNI {sni}"));
            }
            if let Some(ws) = &transport.ws
                && let Some(path) = &ws.path
                && !path.is_empty()
            {
                parts.push(format!("Path {}", path));
            }
            if let Some(reality) = &transport.reality
                && let Some(sni) = &reality.sni
            {
                parts.push(format!("Server {}", sni));
            }
            parts.join(" Â· ")
        }
    }
}

pub(crate) fn security_summary(node: &Node) -> String {
    if let Some(transport) = &node.transport {
        if let Some(tls) = &transport.tls {
            return if tls.skip_cert_verify {
                "TLS (skip cert verify)".to_string()
            } else {
                "TLS".to_string()
            };
        }
        if transport.reality.is_some() {
            return "Reality".to_string();
        }
    }

    match node.protocol {
        ProxyProtocol::Tuic { .. } => "QUIC + TLS".to_string(),
        _ => "Default".to_string(),
    }
}

pub(crate) fn auth_summary(protocol: &ProxyProtocol) -> String {
    match protocol {
        ProxyProtocol::Shadowsocks { cipher, .. } => format!("Cipher {cipher}"),
        ProxyProtocol::VMess {
            cipher, alter_id, ..
        } => {
            format!("Cipher {cipher}, alterId {alter_id}")
        }
        ProxyProtocol::VLess { flow, .. } => flow
            .as_ref()
            .map(|flow| format!("Flow {flow}"))
            .unwrap_or_else(|| "UUID auth".to_string()),
        ProxyProtocol::Tuic {
            congestion_control, ..
        } => format!("Congestion {congestion_control}"),
        ProxyProtocol::Trojan { .. } => "Password auth".to_string(),
        ProxyProtocol::Hysteria2 { .. } => "Password auth".to_string(),
    }
}

pub(crate) fn protocol_note(protocol: &ProxyProtocol) -> &'static str {
    match protocol {
        ProxyProtocol::Tuic { .. } => {
            "Activating a TUIC node points the system proxy at the local HTTP proxy port so desktop traffic is captured."
        }
        ProxyProtocol::VMess { .. } | ProxyProtocol::VLess { .. } => {
            "Transport and TLS details for VMess / VLESS are shown under Transport."
        }
        _ => "Select a node to activate it; once active the real Active state is shown.",
    }
}

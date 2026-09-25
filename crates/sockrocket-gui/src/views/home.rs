// === Home View (Dashboard, v2) ===
// Split impl block of `AppState` (moved from app.rs; behavior unchanged).
//
// Layout per ui-prototype-v2 §page-dashboard:
//   hero glass-panel (40px status ring + Connected heading + Live badge +
//   node name/protocol + Connect/Disconnect), hairline footer row with
//   ↑ green / ↓ cyan speed readouts + 32px throughput sparkline
//   → quick-toggle row (System Proxy / TUN / Rules)
//   → 3 stat cards (Active Connections / Session Traffic / Latency)
//   → Recent Events card (tinted rows with colored left border).
// All connection display derives from the unified state fields
// (`proxy_running` / `proxy_status` / `proxy_validation_status`) through
// `connection_status_kind` — the view never assembles its own status text (§5.1).

use crate::app::*;
use crate::theme::*;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::Sizable as _;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{Icon, Size as ComponentSize};
use sockrocket_core::tun_supported;

impl AppState {
    pub(crate) fn render_home(&mut self, cx: &mut Context<Self>) -> Div {
        // Bandwidth sampling happens in the 1s UI heartbeat (see
        // AppState::sample_bandwidth) so the status bar stays live on every
        // page; here we only read the latest values.
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
        let upload_speed = self.upload_speed_bps;
        let download_speed = self.download_speed_bps;

        // --- Five-state machine derivation (§5.1, single source of truth) ---
        let kind = connection_status_kind(self.proxy_running, &self.proxy_status);
        // Connecting / Verifying (and the Disconnecting transitional state,
        // which maps to Connecting) show the button's loading state (§5.6).
        let busy = matches!(kind, StatusKind::Connecting | StatusKind::Verifying);

        // Current node: the active connection first, the pending selection
        // otherwise (mirrors what the Connect button would use).
        let display_node = self
            .active_proxy_node
            .or(self.selected_node)
            .and_then(|i| self.nodes.get(i));

        // Empty state (never connected / nothing to connect): guide to Nodes.
        if self.nodes.is_empty() {
            return div().child(empty_state(
                "No nodes yet — import a subscription or add a node to get started.",
                Some(
                    Button::new("home-goto-nodes")
                        .xsmall()
                        .primary()
                        .label("Open Nodes")
                        .tooltip("Go to the Nodes page (Ctrl+2)")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.set_view(ActiveView::Nodes, cx);
                        }))
                        .into_any_element(),
                ),
            ));
        }

        // --- Recent events / warnings (v2 tinted rows) ---
        let mut events: Vec<(AlertKind, SharedString)> = Vec::new();
        match kind {
            // Unreachable: Danger row carrying the real failure reason.
            StatusKind::Unreachable => {
                events.push((
                    AlertKind::Danger,
                    self.proxy_validation_status.clone().into(),
                ));
            }
            // Hard errors (start failure, crash, task error).
            StatusKind::Error => {
                events.push((AlertKind::Danger, self.proxy_status.clone().into()));
            }
            _ => {}
        }
        // Health-monitor failover is informational.
        if self.proxy_status.contains("(auto-switched)") {
            events.push((AlertKind::Info, self.proxy_status.clone().into()));
        }
        if self.health_status.starts_with('⚠') {
            events.push((AlertKind::Warning, self.health_status.clone().into()));
        }

        // --- The page's single primary action: Connect / Stop ---
        let can_connect =
            self.proxy_running || (!self.nodes.is_empty() && self.selected_node.is_some());

        // --- Stat-card inputs (live via the 1s UI heartbeat) ---
        let active_conns = self
            .proxy_stats
            .as_ref()
            .map(|s| s.active_connections())
            .unwrap_or(0);
        let session_bytes = self
            .proxy_stats
            .as_ref()
            .map(|s| s.bytes_sent().saturating_add(s.bytes_received()))
            .unwrap_or(0);
        let node_latency = display_node.and_then(|n| n.latency_ms);
        let up_hist: Vec<f64> = self.upload_history.iter().cloned().collect();
        let down_hist: Vec<f64> = self.download_history.iter().cloned().collect();

        // --- v2 hero state ---
        let connected = matches!(kind, StatusKind::Connected);
        let hero_status = if connected {
            "Connected"
        } else if busy {
            kind.label()
        } else {
            "Disconnected"
        };
        let (up_val, up_unit) = speed_parts(upload_speed);
        let (down_val, down_unit) = speed_parts(download_speed);

        // v2 connect button: green-tinted Connect / red-tinted Disconnect
        let connect_btn = {
            let btn = div()
                .id("connect-btn")
                .px_5()
                .py_2()
                .rounded(px(8.0))
                .border_1()
                .text_size(px(SMALL))
                .font_weight(FontWeight::MEDIUM)
                .when(!can_connect, |d| d.opacity(0.5).cursor_default())
                .when(can_connect, |d| {
                    d.cursor_pointer().on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_proxy(cx);
                    }))
                });
            if self.proxy_running {
                btn.bg(rgba(with_alpha(DANGER, 0x1a)))
                    .border_color(rgba(with_alpha(DANGER, 0x4d)))
                    .text_color(rgb(DANGER))
                    .child(if busy { "Stopping…" } else { "Disconnect" })
            } else {
                btn.bg(rgba(with_alpha(SUCCESS, 0x1a)))
                    .border_color(rgba(with_alpha(SUCCESS, 0x4d)))
                    .text_color(rgb(SUCCESS))
                    .child(if busy { "Connecting…" } else { "Connect" })
            }
        };

        div()
            .flex()
            .flex_col()
            .gap_3()
            .h_full()
            // === HERO (v2): icon ring + status + node + connect, speed footer ===
            .child(
                card()
                    .p(px(20.0))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_4()
                            // Icon ring (bolt; green when connected)
                            .child(
                                div()
                                    .w(px(40.0))
                                    .h(px(40.0))
                                    .rounded_full()
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .border_1()
                                    .bg(if connected {
                                        rgba(with_alpha(SUCCESS, 0x1a))
                                    } else {
                                        rgba(with_alpha(TEXT_MUTED, 0x1a))
                                    })
                                    .border_color(if connected {
                                        rgba(with_alpha(SUCCESS, 0x4d))
                                    } else {
                                        rgba(with_alpha(TEXT_MUTED, 0x4d))
                                    })
                                    .child(
                                        Icon::empty()
                                            .path("icons/bolt.svg")
                                            .with_size(ComponentSize::Size(px(20.0)))
                                            .text_color(rgb(if connected {
                                                SUCCESS
                                            } else {
                                                TEXT_MUTED
                                            })),
                                    ),
                            )
                            // Status + node column
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .text_size(px(HEADING))
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(rgb(if connected {
                                                        SUCCESS
                                                    } else if busy {
                                                        ACCENT
                                                    } else {
                                                        TEXT_MUTED
                                                    }))
                                                    .child(hero_status),
                                            )
                                            .child(if connected {
                                                mini_badge("Live", SUCCESS)
                                            } else {
                                                mini_badge("Idle", TEXT_MUTED)
                                            }),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap_2()
                                            .min_w_0()
                                            .child(
                                                div()
                                                    .text_size(px(BODY))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(rgb(if display_node.is_some() {
                                                        TEXT_PRIMARY
                                                    } else {
                                                        TEXT_MUTED
                                                    }))
                                                    .child(
                                                        display_node
                                                            .map(|n| n.name.clone())
                                                            .unwrap_or_else(|| {
                                                                "(none selected)".to_string()
                                                            }),
                                                    ),
                                            )
                                            .when_some(display_node, |d, node| {
                                                d.child(
                                                    div()
                                                        .text_size(px(MICRO))
                                                        .font_family(MONO_FONT)
                                                        .text_color(rgb(TEXT_MUTED))
                                                        .child(format!(
                                                            "{} · {} · {}",
                                                            protocol_short_name(&node.protocol),
                                                            node.port,
                                                            if node.transport.is_some() {
                                                                "TLS"
                                                            } else {
                                                                "PLAIN"
                                                            }
                                                        )),
                                                )
                                            }),
                                    ),
                            )
                            .child(connect_btn),
                    )
                    // Selection hint when Connect cannot fire yet
                    .when(!self.proxy_running && self.selected_node.is_none(), |d| {
                        d.child(
                            div()
                                .text_size(px(SMALL))
                                .text_color(rgb(TEXT_MUTED))
                                .child("Select a node on the Nodes page (Ctrl+2)"),
                        )
                    })
                    // Live traffic footer (v2: mt-4 pt-4 border-t, inline 32px sparkline)
                    .child(hairline().mt_4())
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_6()
                            .pt_4()
                            .child(speed_block("↑", &up_val, &up_unit, SUCCESS))
                            .child(speed_block("↓", &down_val, &down_unit, ACCENT))
                            .child(
                                div()
                                    .ml_4()
                                    .flex_1()
                                    .min_w_0()
                                    .max_w(px(280.0))
                                    .child(throughput_sparkline(&up_hist, &down_hist, 32.0)),
                            ),
                    ),
            )
            // === Quick toggles (v2 glass buttons: System Proxy / TUN / Rules) ===
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_3()
                    // System Proxy
                    .child({
                        let on = self.system_proxy_enabled;
                        quick_btn("qb-sysproxy")
                            .child(status_dot(if on { SUCCESS } else { TEXT_MUTED }))
                            .child(
                                div()
                                    .text_size(px(SMALL))
                                    .text_color(rgb(TEXT_PRIMARY))
                                    .child("System Proxy"),
                            )
                            .child(
                                div()
                                    .text_size(px(MICRO))
                                    .text_color(rgb(if on { SUCCESS } else { TEXT_MUTED }))
                                    .child(if on { "On" } else { "Off" }),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.system_proxy_enabled {
                                    this.disable_system_proxy(cx);
                                } else {
                                    this.enable_system_proxy(cx);
                                }
                            }))
                    })
                    // TUN
                    .child({
                        let tun_ok = tun_supported();
                        let on = self.tun_enabled;
                        let (state, state_color) = if !on && !self.tun_starting && !tun_ok {
                            ("N/A", TEXT_MUTED)
                        } else if self.tun_starting {
                            ("…", ACCENT)
                        } else if on {
                            ("On", SUCCESS)
                        } else {
                            ("Off", TEXT_MUTED)
                        };
                        let btn = quick_btn("qb-tun")
                            .child(status_dot(if on { SUCCESS } else { TEXT_MUTED }))
                            .child(
                                div()
                                    .text_size(px(SMALL))
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .child("TUN Mode"),
                            )
                            .child(
                                div()
                                    .text_size(px(MICRO))
                                    .text_color(rgb(state_color))
                                    .child(state),
                            );
                        if on {
                            btn.on_click(cx.listener(|this, _, _, cx| this.toggle_tun(cx)))
                        } else if !self.tun_starting && tun_ok {
                            btn.on_click(cx.listener(|this, _, _, cx| this.toggle_tun(cx)))
                        } else {
                            btn.cursor_default()
                        }
                    })
                    // Rules shortcut
                    .child(
                        quick_btn("qb-rules")
                            .child(
                                Icon::empty()
                                    .path("icons/nav-rules.svg")
                                    .with_size(ComponentSize::Size(px(14.0)))
                                    .text_color(rgb(TEXT_MUTED)),
                            )
                            .child(
                                div()
                                    .text_size(px(SMALL))
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .child("Rules"),
                            )
                            .child(
                                div()
                                    .text_size(px(MICRO))
                                    .font_family(MONO_FONT)
                                    .text_color(rgb(TEXT_MUTED))
                                    .child(format!("{} rules", self.rules.len())),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_view(ActiveView::Rules, cx);
                            })),
                    ),
            )
            // TUN status / failure reason — tun_status would otherwise never
            // be visible anywhere in the UI.
            .when(
                self.tun_enabled || self.tun_starting || self.tun_status != "Disabled",
                |d| {
                    let status = self.tun_status.clone();
                    let kind = if status.starts_with('❌') || status.starts_with('✗') {
                        AlertKind::Danger
                    } else if status.starts_with('⚠') {
                        AlertKind::Warning
                    } else if status.starts_with('✅') || status.starts_with('✓') {
                        AlertKind::Success
                    } else {
                        AlertKind::Info
                    };
                    d.child(alert_strip(kind, status))
                },
            )
            // === Stat cards (v2: 3-up) ===
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_3()
                    .child(
                        stat_card(
                            "Active Connections",
                            format!("{}", active_conns),
                            format!(
                                "{} total",
                                self.proxy_stats
                                    .as_ref()
                                    .map(|s| s.total_connections())
                                    .unwrap_or(0)
                            ),
                            TEXT_PRIMARY,
                        )
                        .flex_1()
                        .min_w_0(),
                    )
                    .child(
                        stat_card(
                            "Session Traffic",
                            format_bytes(session_bytes),
                            format!(
                                "↑ {} · ↓ {}",
                                format_bytes(bytes_up),
                                format_bytes(bytes_down)
                            ),
                            ACCENT,
                        )
                        .flex_1()
                        .min_w_0(),
                    )
                    .child(
                        stat_card(
                            "Latency",
                            node_latency
                                .map(|ms| format!("{}ms", ms))
                                .unwrap_or_else(|| "—".to_string()),
                            display_node
                                .map(|n| n.name.clone())
                                .unwrap_or_else(|| "no node".to_string()),
                            node_latency
                                .map(|ms| latency_color(ms as u64))
                                .unwrap_or(TEXT_MUTED),
                        )
                        .flex_1()
                        .min_w_0(),
                    ),
            )
            // === Recent Events (v2 tinted rows with colored left border) ===
            .child(
                card().child(section_label("Recent Events")).child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .children(events.iter().map(|(kind, msg)| {
                            let color = kind.color();
                            div()
                                .flex()
                                .flex_row()
                                .items_start()
                                .gap_3()
                                .py(px(6.0))
                                .px_3()
                                .rounded(px(8.0))
                                .bg(rgba(with_alpha(color, 0x0d)))
                                .border_l_2()
                                .border_color(rgb(color))
                                .child(
                                    div()
                                        .text_size(px(SMALL))
                                        .text_color(rgb(if matches!(kind, AlertKind::Success) {
                                            TEXT_PRIMARY
                                        } else {
                                            TEXT_SECONDARY
                                        }))
                                        .child(msg.clone()),
                                )
                        }))
                        .when(events.is_empty(), |d| {
                            d.child(
                                div()
                                    .py_2()
                                    .text_size(px(TINY))
                                    .text_color(rgb(TEXT_MUTED))
                                    .child("No recent events — all systems normal"),
                            )
                        }),
                ),
            )
    }
}

/// v2 quick-toggle glass button base (rounded-lg px-4 py-2.5, hairline border).
fn quick_btn(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .px_4()
        .py(px(10.0))
        .rounded(px(8.0))
        .bg(rgb(BG_PANEL))
        .border_1()
        .border_color(rgba(with_alpha(BORDER, 0x99)))
        .cursor_pointer()
        .hover(|s| s.border_color(rgba(with_alpha(ACCENT, 0x20))))
}

/// Split "1.2 MB/s" into ("1.2", "MB/s") for the v2 value+unit readout.
fn speed_parts(bps: f64) -> (String, String) {
    let s = format_speed(bps);
    match s.split_once(' ') {
        Some((v, u)) => (v.to_string(), u.to_string()),
        None => (s, String::new()),
    }
}

/// v2 hero speed block: arrow + HEADING mono value + MICRO unit.
fn speed_block(arrow: &str, value: &str, unit: &str, color: u32) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_1p5()
        .flex_shrink_0()
        .child(
            div()
                .text_size(px(SMALL))
                .text_color(rgb(TEXT_MUTED))
                .child(arrow.to_string()),
        )
        .child(
            div()
                .text_size(px(HEADING))
                .font_family(MONO_FONT)
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(color))
                .child(value.to_string()),
        )
        .child(
            div()
                .text_size(px(MICRO))
                .font_family(MONO_FONT)
                .text_color(rgb(TEXT_MUTED))
                .child(unit.to_string()),
        )
}

/// Map the unified connection state fields onto the §5.1 five-state machine.
/// This is the only place the Home view interprets `proxy_status`; the pill,
/// the button loading state, and the event rows all derive from its result.
fn connection_status_kind(proxy_running: bool, proxy_status: &str) -> StatusKind {
    if proxy_running {
        if proxy_status.starts_with("Connected") {
            StatusKind::Connected
        } else if proxy_status.starts_with("Verifying") {
            StatusKind::Verifying
        } else if proxy_status.starts_with('⚠') {
            StatusKind::Unreachable
        } else {
            // Listeners bound, service still starting ("Connecting...").
            StatusKind::Connecting
        }
    } else if proxy_status.starts_with('✗') {
        StatusKind::Error
    } else if proxy_status.ends_with("...") {
        // "Connecting..." / "Disconnecting..." transitional states.
        StatusKind::Connecting
    } else {
        StatusKind::Stopped
    }
}

/// 1px hairline divider between in-card sections (no nested cards, §3).
fn hairline() -> Div {
    div()
        .h(px(1.0))
        .w_full()
        .flex_shrink_0()
        .bg(rgba(with_alpha(BORDER, 0x99)))
}

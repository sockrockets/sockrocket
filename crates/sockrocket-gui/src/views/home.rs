// === Home View (macOS-leaning) ===
// One composition: connection hero + preference rows + optional alerts.
// Stats sit inline in the hero; no dashboard card grid.

use crate::app::*;
use crate::theme::*;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::Sizable as _;
use gpui_component::button::{Button, ButtonVariants as _};
use sockrocket_core::tun_supported;

impl AppState {
    pub(crate) fn render_home(&mut self, cx: &mut Context<Self>) -> Div {
        // Bandwidth sampling happens in the 1s UI heartbeat (see
        // AppState::sample_bandwidth) so the status bar stays live on every
        // page; here we only read the latest values.
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

        // --- Hero state (same labels as status bar / chrome) ---
        let connected = matches!(kind, StatusKind::Connected);
        let hero_status = kind.label();
        let (up_val, up_unit) = speed_parts(upload_speed);
        let (down_val, down_unit) = speed_parts(download_speed);

        // Primary CTA: filled system blue / destructive
        let connect_btn = {
            let btn = div()
                .id("connect-btn")
                .px_5()
                .py(px(7.0))
                .rounded(px(8.0))
                .text_size(px(BODY))
                .font_weight(FontWeight::SEMIBOLD)
                .when(!can_connect, |d| d.opacity(0.45).cursor_default())
                .when(can_connect, |d| {
                    d.cursor_pointer().on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_proxy(cx);
                    }))
                });
            if self.proxy_running {
                btn.bg(rgba(with_alpha(DANGER, 0x28)))
                    .text_color(rgb(DANGER))
                    .child(if busy { "Stopping…" } else { "Disconnect" })
            } else {
                btn.bg(rgb(ACCENT))
                    .text_color(rgb(0xffffff))
                    .child(if busy { "Connecting…" } else { "Connect" })
            }
        };

        let tun_ok = tun_supported();
        let sys_on = self.system_proxy_enabled;
        let tun_on = self.tun_enabled;

        div()
            .flex()
            .flex_col()
            .gap_4()
            .h_full()
            .child(self.page_header_v3(
                "icons/nav-dashboard.svg",
                ACCENT,
                sockrocket_gui::i18n::t("nav.dashboard"),
                sockrocket_gui::i18n::t("home.subtitle"),
            ))
            // === Hero: status + CTA + speeds/metrics (one surface) ===
            .child(
                card()
                    .p(px(24.0))
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_4()
                            .child(status_dot(if connected {
                                SUCCESS
                            } else if busy {
                                ACCENT
                            } else {
                                TEXT_MUTED
                            }))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_size(px(HEADING))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(TEXT_PRIMARY))
                                            .child(hero_status),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(SMALL))
                                            .text_color(rgb(TEXT_SECONDARY))
                                            .child(
                                                display_node
                                                    .map(|n| {
                                                        format!(
                                                            "{} · {} · {}",
                                                            n.name,
                                                            protocol_short_name(&n.protocol),
                                                            n.port
                                                        )
                                                    })
                                                    .unwrap_or_else(|| {
                                                        sockrocket_gui::i18n::t("home.select_node")
                                                            .into()
                                                    }),
                                            ),
                                    ),
                            )
                            .child(connect_btn),
                    )
                    .when(!self.proxy_running && self.selected_node.is_none(), |d| {
                        d.child(
                            div()
                                .text_size(px(SMALL))
                                .text_color(rgb(TEXT_MUTED))
                                .child(sockrocket_gui::i18n::t("home.select_node")),
                        )
                    })
                    .child(hairline())
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_end()
                            .gap(px(20.0))
                            .child(speed_block("↑", &up_val, &up_unit, TEXT_PRIMARY))
                            .child(speed_block("↓", &down_val, &down_unit, TEXT_PRIMARY))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .max_w(px(240.0))
                                    .child(throughput_sparkline(&up_hist, &down_hist, 28.0)),
                            )
                            .child(
                                inline_metric(
                                    "Connections",
                                    format!("{active_conns}"),
                                    TEXT_PRIMARY,
                                )
                                .ml_auto(),
                            )
                            .child(inline_metric(
                                "Traffic",
                                format_bytes(session_bytes),
                                TEXT_PRIMARY,
                            ))
                            .child(inline_metric(
                                "Latency",
                                node_latency
                                    .map(|ms| format!("{ms} ms"))
                                    .unwrap_or_else(|| "—".into()),
                                node_latency
                                    .map(|ms| latency_color(ms as u64))
                                    .unwrap_or(TEXT_MUTED),
                            )),
                    ),
            )
            // === Preferences group (Settings-style rows) ===
            .child(
                card()
                    .p(px(0.0))
                    .gap_0()
                    .child(pref_row(
                        "System Proxy",
                        "Route apps through Sockrocket",
                        toggle_switch("home-sysproxy", sys_on).on_click(cx.listener(
                            |this, _, _, cx| {
                                if this.system_proxy_enabled {
                                    this.disable_system_proxy(cx);
                                } else {
                                    this.enable_system_proxy(cx);
                                }
                            },
                        )),
                        true,
                    ))
                    .child(div().px_4().child(hairline()))
                    .child({
                        let tun_switch = toggle_switch("home-tun", tun_on);
                        let tun_switch = if tun_ok && !self.tun_starting {
                            tun_switch.on_click(cx.listener(|this, _, _, cx| this.toggle_tun(cx)))
                        } else {
                            tun_switch.opacity(0.45).cursor_default()
                        };
                        pref_row(
                            "TUN Mode",
                            if tun_ok {
                                "Capture all traffic (needs admin on Mac)"
                            } else {
                                "Not available on this platform"
                            },
                            tun_switch,
                            true,
                        )
                    })
                    .child(div().px_4().child(hairline()))
                    .child(
                        div()
                            .id("home-rules-row")
                            .w_full()
                            .px_4()
                            .py_3()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(BG_HOVER)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_view(ActiveView::Rules, cx);
                            }))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_size(px(BODY))
                                            .text_color(rgb(TEXT_PRIMARY))
                                            .child("Rules"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(SMALL))
                                            .text_color(rgb(TEXT_SECONDARY))
                                            .child(format!(
                                                "{} rules configured",
                                                self.rules.len()
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(SMALL))
                                    .text_color(rgb(ACCENT))
                                    .child("Open"),
                            ),
                    ),
            )
            .when(
                self.tun_enabled || self.tun_starting || self.tun_status != "Disabled",
                |d| {
                    let status = self.tun_status.clone();
                    let kind = if status.contains("failed")
                        || status.contains("Failed")
                        || status.contains("Error")
                        || status.starts_with('❌')
                        || status.starts_with('✗')
                    {
                        AlertKind::Danger
                    } else if status.starts_with('⚠') {
                        AlertKind::Warning
                    } else if status.contains("active") || status.contains("Active") {
                        AlertKind::Success
                    } else {
                        AlertKind::Info
                    };
                    d.child(alert_strip(kind, status))
                },
            )
            .when(!events.is_empty(), |d| {
                d.child(
                    card().child(section_label("Needs attention")).children(
                        events
                            .iter()
                            .map(|(kind, msg)| alert_strip(*kind, msg.clone())),
                    ),
                )
            })
    }
}

fn pref_row(title: &str, subtitle: &str, control: impl IntoElement, _last: bool) -> Div {
    div()
        .w_full()
        .px_4()
        .py_3()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_size(px(BODY))
                        .text_color(rgb(TEXT_PRIMARY))
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .text_size(px(SMALL))
                        .text_color(rgb(TEXT_SECONDARY))
                        .child(subtitle.to_string()),
                ),
        )
        .child(control)
}

/// Split "1.2 MB/s" into ("1.2", "MB/s") for the v2 value+unit readout.
fn speed_parts(bps: f64) -> (String, String) {
    let s = format_speed(bps);
    match s.split_once(' ') {
        Some((v, u)) => (v.to_string(), u.to_string()),
        None => (s, String::new()),
    }
}

/// Compact speed readout for the hero footer.
fn speed_block(arrow: &str, value: &str, unit: &str, color: u32) -> Div {
    div()
        .flex()
        .flex_row()
        .items_baseline()
        .gap_1()
        .flex_shrink_0()
        .child(
            div()
                .text_size(px(SMALL))
                .text_color(rgb(TEXT_SECONDARY))
                .child(arrow.to_string()),
        )
        .child(
            div()
                .text_size(px(BODY))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(color))
                .child(value.to_string()),
        )
        .child(
            div()
                .text_size(px(TINY))
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

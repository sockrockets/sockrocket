// === Config View ===
// Split impl block of `AppState` (moved from app.rs; behavior unchanged).
// Continuous card flow per docs/design-system.md §6: every card starts with a
// `section_label`, form rows are label-left / control-right, zones inside a
// card are separated by a 1px BORDER hairline (no nested cards).

use crate::app::*;
use crate::theme::*;
use crate::views::nodes::confirm_armed;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::Disableable as _;
use gpui_component::Sizable as _;
use gpui_component::button::{Button, ButtonVariants as _};
use sockrocket_core::config::model::ProxyProtocol;
use std::sync::atomic::{AtomicU64, Ordering};

/// Two-step confirm state for "Clear Nodes" (same pattern as the Nodes page
/// "Delete All", see views/nodes.rs).
static CLEAR_NODES_ARMED_AT: AtomicU64 = AtomicU64::new(0);

/// Map a legacy status-message prefix to an [`AlertKind`] (mirrors the
/// `status_text_color` mapping in app.rs).
fn alert_kind_for(message: &str) -> AlertKind {
    if message.starts_with('✓') || message.starts_with('✅') {
        AlertKind::Success
    } else if message.starts_with('✗') || message.starts_with('❌') {
        AlertKind::Danger
    } else if message.starts_with('⚠') {
        AlertKind::Warning
    } else {
        AlertKind::Info
    }
}

/// 1px BORDER hairline separating zones inside a card.
fn hairline() -> Div {
    div()
        .h(px(1.0))
        .w_full()
        .flex_shrink_0()
        .bg(rgba(with_alpha(BORDER, 0x99)))
}

/// Form row: SMALL muted label on the left, control on the right.
fn form_row(label: &str, control: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .py_0p5()
        .child(
            div()
                .flex_shrink_0()
                .text_size(px(SMALL))
                .text_color(rgb(TEXT_MUTED))
                .child(label.to_string()),
        )
        .child(control)
}

impl AppState {
    pub(crate) fn render_config(&mut self, cx: &mut Context<Self>) -> Div {
        let import_status = self.import_status.clone();
        let node_count = self.nodes.len();

        // Count nodes by protocol
        let mut ss_count = 0u32;
        let mut vmess_count = 0u32;
        let mut vless_count = 0u32;
        let mut tuic_count = 0u32;
        let mut trojan_count = 0u32;
        let mut hy2_count = 0u32;
        for node in &self.nodes {
            match &node.protocol {
                ProxyProtocol::Shadowsocks { .. } => ss_count += 1,
                ProxyProtocol::VMess { .. } => vmess_count += 1,
                ProxyProtocol::VLess { .. } => vless_count += 1,
                ProxyProtocol::Tuic { .. } => tuic_count += 1,
                ProxyProtocol::Trojan { .. } => trojan_count += 1,
                ProxyProtocol::Hysteria2 { .. } => hy2_count += 1,
            }
        }

        let import_url_input = self.import_url_input.clone();
        let node_uri_input = self.node_uri_input.clone();
        let is_import_tab = self.config_tab == ConfigTab::ImportSubscription;

        div()
            .flex()
            .flex_col()
            .gap_3()
            // Title
            .child(self.page_header("Configuration"))
            // Import card: Import Subscription | Add by Share Link
            .child(
                card()
                    .child(section_label("Import"))
                    .child(
                        // Tab strip (tab_button style: ACCENT underline on active)
                        div()
                            .flex()
                            .flex_row()
                            .gap_1()
                            .border_b_1()
                            .border_color(rgba(with_alpha(BORDER, 0x99)))
                            .child(
                                tab_button(
                                    "config-tab-import",
                                    "Import Subscription",
                                    is_import_tab,
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.config_tab = ConfigTab::ImportSubscription;
                                    cx.notify();
                                })),
                            )
                            .child(
                                tab_button("config-tab-uri", "Add by Share Link", !is_import_tab)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.config_tab = ConfigTab::AddByUri;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(if is_import_tab {
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                gpui_component::input::Input::new(&import_url_input)
                                    .xsmall()
                                    .cleanable(true),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap_2()
                                    .child(
                                        Button::new("import-btn").xsmall()
                                            .label("Import".to_string())
                                            .tooltip("Fetch nodes from subscription URL")
                                            .primary()
                                            .loading(self.import_status.contains("Importing"))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.import_subscription(cx);
                                            })),
                                    )
                                    .child({
                                        let armed = confirm_armed(&CLEAR_NODES_ARMED_AT);
                                        Button::new("clear-btn").xsmall()
                                            .label(if armed {
                                                "Confirm?".to_string()
                                            } else {
                                                "Clear Nodes".to_string()
                                            })
                                            .tooltip(if armed {
                                                "Click again within 3 s to remove all nodes"
                                            } else {
                                                "Remove all imported nodes"
                                            })
                                            .when(armed, |b| b.danger())
                                            .when(!armed, |b| b.ghost())
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                if confirm_armed(&CLEAR_NODES_ARMED_AT) {
                                                    CLEAR_NODES_ARMED_AT.store(0, Ordering::Relaxed);
                                                    this.clear_nodes(cx);
                                                } else {
                                                    this.arm_destructive_confirm(
                                                        &CLEAR_NODES_ARMED_AT,
                                                        cx,
                                                    );
                                                }
                                            }))
                                    }),
                            )
                            .child(if !import_status.is_empty() {
                                let is_importing = import_status.contains("Importing");
                                let strip =
                                    alert_strip(alert_kind_for(&import_status), import_status);
                                if is_importing {
                                    strip
                                        .with_animation(
                                            "import-pulse",
                                            Animation::new(std::time::Duration::from_millis(1000))
                                                .repeat()
                                                .with_easing(pulsating_between(0.3, 1.0)),
                                            |el, delta| el.opacity(delta),
                                        )
                                        .into_any_element()
                                } else {
                                    strip.into_any_element()
                                }
                            } else {
                                div().into_any_element()
                            })
                            .into_any_element()
                    } else {
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                gpui_component::input::Input::new(&node_uri_input)
                                    .xsmall()
                                    .cleanable(true),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        Button::new("add-uri-btn").xsmall()
                                            .label("Add Node".to_string())
                                            .tooltip("Parse and add node from URI")
                                            .primary()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.add_node_from_uri(cx);
                                            })),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .overflow_x_hidden()
                                            .text_size(px(MICRO))
                                            .font_family(MONO_FONT)
                                            .text_color(rgb(TEXT_MUTED))
                                            .child(
                                                "vless:// · vmess:// · ss:// · trojan:// · tuic:// · hy2://",
                                            ),
                                    ),
                            )
                            // add_node_from_uri reports via import_status too —
                            // show it on this tab as well, otherwise parse
                            // errors are silently swallowed.
                            .child(if !import_status.is_empty() {
                                alert_strip(alert_kind_for(&import_status), import_status)
                                    .into_any_element()
                            } else {
                                div().into_any_element()
                            })
                            .into_any_element()
                    }),
            )
            // Saved subscriptions card
            .child(self.render_subscription_list(cx))
            // Node summary card
            .child(
                card()
                    .child(section_label("Node Summary"))
                    .child(kv_row("Shadowsocks", ss_count.to_string()))
                    .child(kv_row("VMess", vmess_count.to_string()))
                    .child(kv_row("VLESS", vless_count.to_string()))
                    .child(kv_row("TUIC", tuic_count.to_string()))
                    .child(kv_row("Trojan", trojan_count.to_string()))
                    .child(kv_row("Hysteria2", hy2_count.to_string()))
                    .child(hairline())
                    .child(kv_row("Total", node_count.to_string())),
            )
            // Health check card
            .child(self.render_health_check_card(cx))
    }

    pub(crate) fn render_health_check_card(&mut self, cx: &mut Context<Self>) -> Div {
        let enabled = self.health_check.enabled;
        let auto_switch = self.health_check.auto_switch;
        let interval_label = format!("{}s", self.health_check.interval_secs);
        let threshold_label = format!("{}", self.health_check.failure_threshold);
        let health_status = self.health_status.clone();

        card()
            .child(section_label("Health Check"))
            .child(form_row(
                "Enabled",
                Button::new("hc-toggle").xsmall()
                    .label(if enabled {
                        "On".to_string()
                    } else {
                        "Off".to_string()
                    })
                    .tooltip("Periodically probe the active node")
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_health_check(cx);
                    })),
            ))
            .child(form_row(
                "Probe interval",
                Button::new("hc-interval").xsmall()
                    .label(interval_label)
                    .tooltip("Seconds between node health probes")
                    .ghost()
                    .disabled(!enabled)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.cycle_health_interval(cx);
                    })),
            ))
            .child(form_row(
                "Failure threshold",
                Button::new("hc-threshold").xsmall()
                    .label(threshold_label)
                    .tooltip("Consecutive probe failures before the node is declared dead")
                    .ghost()
                    .disabled(!enabled)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.cycle_health_threshold(cx);
                    })),
            ))
            .child(form_row(
                "Auto-switch node",
                Button::new("hc-auto-switch").xsmall()
                    .label(if auto_switch {
                        "On".to_string()
                    } else {
                        "Off".to_string()
                    })
                    .tooltip("Switch to the fastest reachable node when the active one dies")
                    .ghost()
                    .disabled(!enabled)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_health_auto_switch(cx);
                    })),
            ))
            .child(if !health_status.is_empty() {
                alert_strip(alert_kind_for(&health_status), health_status).into_any_element()
            } else {
                div().into_any_element()
            })
            .child(hairline())
            .child(
                div()
                    .text_size(px(SMALL))
                    .text_color(rgb(TEXT_MUTED))
                    .child("Probes the active node through the proxy; after repeated failures it switches to the lowest-latency reachable node without restarting the listeners."),
            )
    }
}

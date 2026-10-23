// === Settings View (v2) ===
// Split impl block of `AppState` (moved from app.rs; behavior unchanged).
//
// Layout per ui-prototype-v2 §page-settings: vertical flow of three cards
// (PROXY / SYSTEM / ABOUT), each opened by a `section_label`. Form rows are
// label-left (SMALL TEXT_SECONDARY) / control-right; zones inside a card are
// separated by a 1px BORDER hairline (no nested cards).
//
// v2 elements intentionally omitted (unsupported by this app, no fakes):
//   - "Auto-start on login" / "Minimize to tray" toggles
//   - Keyboard Shortcuts card (shortcuts live in tooltips)
//   - Theme picker (app uses the light Settings palette) — shown as a static ghost label

use crate::app::*;
use crate::theme::*;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::Sizable as _;
use gpui_component::input::InputState;
use gpui_component::{Icon, Size as ComponentSize};

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

fn dns_button(id: &'static str, label: impl Into<SharedString>) -> Stateful<Div> {
    primary_button(id, label)
        .bg(rgba(with_alpha(ACCENT, 0x15)))
        .text_color(rgb(ACCENT))
}

impl AppState {
    pub(crate) fn render_settings(&mut self, cx: &mut Context<Self>) -> Div {
        let listen_addr_input = self.listen_addr_input.clone();
        let socks_port_input = self.socks_port_input.clone();
        let http_port_input = self.http_port_input.clone();
        let settings_status = self.settings_status.clone();
        let system_proxy_on = self.system_proxy_enabled;
        let system_proxy_status = self.system_proxy_status.clone();

        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(self.page_header_v3(
                "icons/nav-settings.svg",
                ACCENT,
                sockrocket_gui::i18n::t("settings.title"),
                sockrocket_gui::i18n::t("settings.section.system"),
            ))
            // === PROXY card (v2: three label/input rows + Apply Changes) ===
            .child(
                card()
                    .child(section_label(sockrocket_gui::i18n::t(
                        "settings.section.proxy",
                    )))
                    .child(self.setting_row(
                        sockrocket_gui::i18n::t("settings.listen_address"),
                        &listen_addr_input,
                    ))
                    .child(self.setting_row(
                        sockrocket_gui::i18n::t("settings.socks_port"),
                        &socks_port_input,
                    ))
                    .child(self.setting_row(
                        sockrocket_gui::i18n::t("settings.http_port"),
                        &http_port_input,
                    ))
                    .child(alert_strip(
                        AlertKind::Warning,
                        sockrocket_gui::i18n::t("settings.listeners_restart_hint"),
                    ))
                    .child(hairline())
                    .child(
                        primary_button("apply-settings", sockrocket_gui::i18n::t("settings.apply"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.apply_settings(cx);
                            })),
                    )
                    .when(!settings_status.is_empty(), |d| {
                        d.child(alert_strip(
                            alert_kind_for(&settings_status),
                            settings_status,
                        ))
                    }),
            )
            .child(card().child(section_label("DNS"))
                .child(div().text_size(px(SMALL)).text_color(rgb(TEXT_SECONDARY)).child("One endpoint per line. Direct: UDP / TCP / TLS / HTTPS. Proxy: TCP / TLS / HTTPS."))
                .child(div().text_size(px(SMALL)).child("Direct DNS (required)"))
                .child(gpui_component::input::Input::new(&self.dns_direct_input).xsmall())
                .child(div().text_size(px(SMALL)).child("Proxy DNS (required)"))
                .child(gpui_component::input::Input::new(&self.dns_proxy_input).xsmall())
                .child(div().text_size(px(SMALL)).child("Bootstrap and node DNS (required IP:port)"))
                .child(gpui_component::input::Input::new(&self.dns_bootstrap_input).xsmall())
                .child(div().flex().flex_row().flex_wrap().gap_2()
                    .child(dns_button("dns-strategy",format!("Address policy: {:?}",self.dns_draft.strategy)).on_click(cx.listener(|this,_,_,cx|{
                        use sockrocket_core::config::dns::AddressStrategy::*;
                        this.dns_draft.strategy=match this.dns_draft.strategy{PreferIpv4=>PreferIpv6,PreferIpv6=>Ipv4Only,Ipv4Only=>Ipv6Only,Ipv6Only=>PreferIpv4};cx.notify();
                    })))
                    .child(dns_button("dns-cache",if self.dns_draft.cache_enabled{"Cache: On"}else{"Cache: Off"}).on_click(cx.listener(|this,_,_,cx|{this.dns_draft.cache_enabled = !this.dns_draft.cache_enabled;cx.notify();})))
                    .child(primary_button("dns-save","Apply DNS").on_click(cx.listener(|this,_,_,cx|{this.apply_dns_settings(cx);})))
                    .child(dns_button("dns-reload","Reload saved DNS").on_click(cx.listener(|this,_,window,cx|{this.reload_dns_form(window,cx);}))))
                .child(div().text_size(px(MICRO)).text_color(rgb(TEXT_MUTED)).child("Encrypted DNS verifies certificates. Applying DNS preserves the current proxy and TUN state; new lookups use the saved policy.")))
            // === SYSTEM card (v2: System Proxy toggle + static Theme label) ===
            .child(
                card()
                    .child(section_label(sockrocket_gui::i18n::t(
                        "settings.section.system",
                    )))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .py(px(4.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_size(px(SMALL))
                                            .text_color(rgb(TEXT_SECONDARY))
                                            .child(sockrocket_gui::i18n::t(
                                                "settings.system_proxy",
                                            )),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(MICRO))
                                            .text_color(rgb(TEXT_MUTED))
                                            .child(system_proxy_status.clone()),
                                    ),
                            )
                            .child(
                                toggle_switch("system-proxy-toggle", system_proxy_on).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        if this.system_proxy_enabled {
                                            this.disable_system_proxy(cx);
                                        } else {
                                            this.enable_system_proxy(cx);
                                        }
                                    }),
                                ),
                            ),
                    )
                    .child(hairline())
                    // TUN is a whole-app mode: one switch that routes all traffic
                    // through the virtual adapter and stays on across node changes.
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .py(px(4.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_size(px(SMALL))
                                            .text_color(rgb(TEXT_SECONDARY))
                                            .child("TUN Mode"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(MICRO))
                                            .text_color(rgb(TEXT_MUTED))
                                            .child(self.tun_status.clone()),
                                    ),
                            )
                            .child(
                                toggle_switch(
                                    "tun-mode-toggle",
                                    self.tun_enabled || self.tun_starting,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.set_tun_mode(
                                            !(this.tun_enabled || this.tun_starting),
                                            cx,
                                        );
                                    },
                                )),
                            ),
                    )
                    .child(hairline())
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .py(px(4.0))
                            .child(
                                div()
                                    .text_size(px(SMALL))
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .child(sockrocket_gui::i18n::t("settings.theme")),
                            )
                            // Static ghost label: the app uses the light Settings palette, so the
                            // v2 theme dropdown is rendered non-interactive.
                            .child(
                                div()
                                    .px_3()
                                    .py_1()
                                    .rounded(px(8.0))
                                    .border_1()
                                    .border_color(rgba(with_alpha(BORDER, 0x99)))
                                    .text_size(px(SMALL))
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .child(sockrocket_gui::i18n::t("settings.theme.light")),
                            ),
                    ),
            )
            .child(card().child(section_label("Network capabilities"))
                .child(div().text_size(px(SMALL)).text_color(rgb(TEXT_SECONDARY))
                    .child("HTTP/HTTPS · SOCKS5 TCP and UDP · TUN TCP and UDP"))
                .child(div().text_size(px(MICRO)).text_color(rgb(TEXT_MUTED))
                    .child("UDP requires a compatible node. SOCKS BIND is unsupported. Merlin offers IPv4 compatibility and opt-in dual-stack routing. Firmware and LAN acceptance checks are required for IPv6.")))
            // === ABOUT card (v2: gradient logo + name/version + GitHub) ===
            .child(
                card()
                    .child(section_label(sockrocket_gui::i18n::t(
                        "settings.section.about",
                    )))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .w(px(40.0))
                                    .h(px(40.0))
                                    .rounded(px(12.0))
                                    .flex_shrink_0()
                                    .bg(rgb(ACCENT))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        Icon::empty()
                                            .path("icons/sockrocket.svg")
                                            .with_size(ComponentSize::Size(px(20.0)))
                                            .text_color(rgb(0xffffff)),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_size(px(BODY))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(TEXT_PRIMARY))
                                            .child(sockrocket_gui::i18n::t("settings.about.name")),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(MICRO))
                                            .text_color(rgb(TEXT_MUTED))
                                            .child(format!(
                                                "v{} · Rust + GPUI",
                                                env!("CARGO_PKG_VERSION")
                                            )),
                                    ),
                            ),
                    )
                    .child(hairline())
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                div()
                                    .id("about-github")
                                    .flex()
                                    .flex_row()
                                    .px_3()
                                    .py(px(6.0))
                                    .rounded(px(8.0))
                                    .border_1()
                                    .border_color(rgba(with_alpha(BORDER, 0x99)))
                                    .text_size(px(SMALL))
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgb(BG_HOVER)))
                                    .child(sockrocket_gui::i18n::t("settings.about.github"))
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.open_url("https://github.com/sockrockets/sockrocket");
                                    })),
                            )
                            .child(
                                div()
                                    .id("about-check-update")
                                    .flex()
                                    .flex_row()
                                    .px_3()
                                    .py(px(6.0))
                                    .rounded(px(8.0))
                                    .border_1()
                                    .border_color(rgba(with_alpha(BORDER, 0x99)))
                                    .text_size(px(SMALL))
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgb(BG_HOVER)))
                                    .child(sockrocket_gui::i18n::t("settings.about.check_update"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.check_for_updates(cx);
                                    })),
                            )
                            .children(self.update_download_url.as_ref().map(|_| {
                                div()
                                    .id("about-open-download")
                                    .flex()
                                    .flex_row()
                                    .px_3()
                                    .py(px(6.0))
                                    .rounded(px(8.0))
                                    .bg(rgba(with_alpha(ACCENT, 0x22)))
                                    .border_1()
                                    .border_color(rgba(with_alpha(ACCENT, 0x66)))
                                    .text_size(px(SMALL))
                                    .text_color(rgb(TEXT_ACCENT))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgba(with_alpha(ACCENT, 0x33))))
                                    .child(sockrocket_gui::i18n::t("settings.about.open_download"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.open_update_download(cx);
                                    }))
                            })),
                    )
                    .children(if !self.settings_status.is_empty() {
                        Some(
                            div()
                                .mt_2()
                                .text_size(px(TINY))
                                .text_color(rgb(TEXT_MUTED))
                                .child(self.settings_status.clone()),
                        )
                    } else {
                        None
                    })
                    .children(self.update_download_url.as_ref().map(|_| {
                        div()
                            .mt_1()
                            .text_size(px(TINY))
                            .text_color(rgb(TEXT_MUTED))
                            .child(sockrocket_gui::i18n::t("settings.about.update_keep_config"))
                    })),
            )
    }

    /// v2 setting row: SMALL TEXT_SECONDARY label on the left, input in a
    /// 160px (w-40) BG_HOVER + BORDER rounded-lg well on the right.
    pub(crate) fn setting_row(&self, label: &str, input: &Entity<InputState>) -> Div {
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .text_size(px(SMALL))
                    .text_color(rgb(TEXT_SECONDARY))
                    .child(label.to_string()),
            )
            .child(
                div()
                    .w(px(160.0))
                    .flex_shrink_0()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(rgba(with_alpha(BORDER, 0x99)))
                    .bg(rgb(BG_HOVER))
                    .px_2()
                    .child(gpui_component::input::Input::new(input).xsmall()),
            )
    }
}

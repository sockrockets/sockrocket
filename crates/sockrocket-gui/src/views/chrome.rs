// === Chrome (v2): title bar, icon sidebar, status bar ===
// TitleBar 34px: logo | drag strip | mode capsule · node badge | controls.
// Borderless against the 38px icon rail below it — both share BG_SIDEBAR so
// the chrome reads as one continuous surface (ui-prototype-v2 §topbar).
// StatusBar 26px: status pill · node latency | ports · speeds · version.
// Visual recipes follow ui-prototype-v2 via `crate::theme`.

use crate::app::*;
use crate::theme::*;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::{Icon, Sizable as _, Size, TitleBar};
use sockrocket_core::ProxyMode;

/// Map the single connection state machine onto a [`StatusKind`] for the
/// status bar pill (design-system §5.1). Pure display derivation.
fn connection_status_kind(running: bool, status: &str) -> StatusKind {
    if running {
        if status.starts_with("Connected") {
            StatusKind::Connected
        } else if status.starts_with("Verifying") {
            StatusKind::Verifying
        } else if status.starts_with('⚠') {
            StatusKind::Unreachable
        } else {
            // Listeners bound, service still starting ("Connecting...").
            StatusKind::Connecting
        }
    } else if status.starts_with('✗') {
        StatusKind::Error
    } else if status.starts_with("Connecting") || status.starts_with("Disconnecting") {
        StatusKind::Connecting
    } else {
        StatusKind::Stopped
    }
}

/// 1px vertical separator used between status bar items.
fn vsep() -> Div {
    div()
        .w(px(1.0))
        .h(px(10.0))
        .flex_shrink_0()
        .bg(rgba(with_alpha(BORDER, 0x99)))
}

/// MICRO mono status bar item.
fn statusbar_item(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(MICRO))
        .font_family(MONO_FONT)
        .text_color(rgb(TEXT_SECONDARY))
        .child(text.into())
}

/// Nav entries of the v2 icon sidebar: (label, shortcut, icon svg path, view).
/// Icon artwork is the heroicon set from ui-prototype-v2.
/// Shortcut labels must match the key bindings in main.rs.
const NAV_ITEMS: &[(&str, &str, &str, ActiveView)] = &[
    (
        "nav.dashboard",
        "Ctrl+1",
        "icons/nav-dashboard.svg",
        ActiveView::Home,
    ),
    (
        "nav.nodes",
        "Ctrl+2",
        "icons/nav-nodes.svg",
        ActiveView::Nodes,
    ),
    (
        "nav.groups",
        "Ctrl+7",
        "icons/nav-groups.svg",
        ActiveView::Groups,
    ),
    (
        "nav.connections",
        "Ctrl+3",
        "icons/nav-connections.svg",
        ActiveView::Connections,
    ),
    (
        "nav.rules",
        "Ctrl+4",
        "icons/nav-rules.svg",
        ActiveView::Rules,
    ),
    ("nav.logs", "Ctrl+5", "icons/nav-logs.svg", ActiveView::Logs),
    (
        "nav.settings",
        "Ctrl+6",
        "icons/nav-settings.svg",
        ActiveView::Settings,
    ),
];

/// 16px nav icon from an embedded SVG asset path.
fn nav_icon(path: &str, color: u32) -> Icon {
    Icon::empty()
        .path(SharedString::from(path.to_string()))
        .with_size(Size::Size(px(16.0)))
        .text_color(rgb(color))
}

/// One caption button (min / max / close). On Windows the area is handed to
/// the OS via `window_control_area` (native snap layout, aero behaviors);
/// elsewhere it falls back to plain click handlers.
#[allow(dead_code)] // used on non-macOS window chrome
fn window_control_button(
    id: &str,
    icon_path: &str,
    area: WindowControlArea,
    is_close: bool,
) -> impl IntoElement {
    #[cfg(not(target_os = "windows"))]
    let _ = is_close;
    let btn = div()
        .id(SharedString::from(id.to_string()))
        .w(px(46.0))
        .h_full()
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(TEXT_SECONDARY))
        .child(
            Icon::empty()
                .path(SharedString::from(icon_path.to_string()))
                .with_size(Size::Size(px(14.0))),
        );
    #[cfg(target_os = "windows")]
    let btn = btn.window_control_area(area).hover(|s| {
        if is_close {
            s.bg(rgba(0xe81123ff)).text_color(rgba(0xffffffff))
        } else {
            s.bg(rgb(BG_HOVER)).text_color(rgb(TEXT_PRIMARY))
        }
    });
    #[cfg(not(target_os = "windows"))]
    let btn = btn
        .cursor_pointer()
        .on_click(move |_, window, _| match area {
            WindowControlArea::Min => window.minimize_window(),
            WindowControlArea::Max => window.zoom_window(),
            WindowControlArea::Close => window.remove_window(),
            WindowControlArea::Drag => {}
        });
    btn
}

/// Right-end window controls: minimize / maximize-restore / close.
fn window_control_buttons(window: &mut Window) -> impl IntoElement {
    #[cfg(target_os = "macos")]
    {
        let _ = window;
        div().id("window-controls")
    }
    #[cfg(not(target_os = "macos"))]
    div()
        .id("window-controls")
        .h_full()
        .flex()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .child(window_control_button(
            "win-min",
            "icons/window-minimize.svg",
            WindowControlArea::Min,
            false,
        ))
        .child(if window.is_maximized() {
            window_control_button(
                "win-restore",
                "icons/window-restore.svg",
                WindowControlArea::Max,
                false,
            )
        } else {
            window_control_button(
                "win-max",
                "icons/window-maximize.svg",
                WindowControlArea::Max,
                false,
            )
        })
        .child(window_control_button(
            "win-close",
            "icons/window-close.svg",
            WindowControlArea::Close,
            true,
        ))
}

impl AppState {
    pub fn titlebar_options() -> TitlebarOptions {
        TitleBar::title_bar_options()
    }

    pub(crate) fn render_titlebar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active_node_info = self
            .active_proxy_node
            .or(self.selected_node)
            .and_then(|i| self.nodes.get(i))
            .map(|n| {
                let latency = n
                    .latency_ms
                    .map(|ms| format!("{}ms", ms))
                    .unwrap_or_default();
                (n.name.clone(), latency)
            });

        div()
            .id("title-bar")
            .h(px(34.0))
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            // macOS traffic lights sit over the title bar; match gpui-component's
            // TITLE_BAR_LEFT_PADDING so the logo/brand is not covered.
            .when(cfg!(target_os = "macos"), |el| el.pl(px(80.0)))
            .when(!cfg!(target_os = "macos"), |el| el.pl_2())
            .bg(rgb(BG_SIDEBAR))
            // No bottom border: the title bar and the sidebar share
            // BG_SIDEBAR, so chrome reads as one continuous surface; the
            // content area separates by background color alone.
            // ── Left: logo (cyan block + sockrocket mark) ──────────────
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1p5()
                    .h_full()
                    // Logo mark: cyan gradient block + dark sockrocket
                    .child(
                        div()
                            .w(px(20.0))
                            .h(px(20.0))
                            .rounded(px(4.0))
                            .bg(linear_gradient(
                                135.0,
                                linear_color_stop(rgba(0x22d3eeff), 0.0),
                                linear_color_stop(rgba(0x22d3ee80), 1.0),
                            ))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::empty()
                                    .path("icons/sockrocket.svg")
                                    .with_size(Size::Size(px(12.0)))
                                    .text_color(rgb(BG_APP)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(PAGE_TITLE))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(TEXT_PRIMARY))
                            .child(sockrocket_gui::i18n::t("chrome.brand")),
                    ),
            )
            // ── Middle: the ONLY window-drag strip. gpui-component's
            // TitleBar marks the whole child area as a Drag region, whose
            // HTCAPTION hit test swallows every click inside it on Windows.
            // Keep interactive elements outside this strip so they stay
            // clickable; only the empty gap drags the window.
            .child(
                div()
                    .id("title-bar-drag")
                    .flex_1()
                    .h_full()
                    .window_control_area(WindowControlArea::Drag),
            )
            // ── Right: mode capsule + active node badge ─────────────────
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h_full()
                    .pr_2()
                    // Mode capsule: segmented control (Rule / Global / Direct)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(2.0))
                            .p(px(2.0))
                            .rounded(px(8.0))
                            .bg(rgb(BG_HOVER))
                            .border_1()
                            .border_color(rgba(with_alpha(BORDER, 0x99)))
                            .child(
                                seg_button("mode-rule", "Rule", self.proxy_mode == ProxyMode::Rule)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.set_proxy_mode(ProxyMode::Rule, cx);
                                    })),
                            )
                            .child(
                                seg_button(
                                    "mode-global",
                                    "Global",
                                    self.proxy_mode == ProxyMode::Global,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.set_proxy_mode(ProxyMode::Global, cx);
                                    },
                                )),
                            )
                            .child(
                                seg_button(
                                    "mode-direct",
                                    "Direct",
                                    self.proxy_mode == ProxyMode::Direct,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.set_proxy_mode(ProxyMode::Direct, cx);
                                    },
                                )),
                            ),
                    )
                    .child(vsep().h(px(14.0)))
                    // Active node badge (click → Nodes page)
                    .children(active_node_info.map(|(name, latency)| {
                        let connected = self.proxy_running;
                        div()
                            .id("titlebar-node-badge")
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1p5()
                            .px_2()
                            .py_0p5()
                            .rounded(px(6.0))
                            .border_1()
                            .cursor_pointer()
                            .bg(if connected {
                                rgba(with_alpha(SUCCESS, 0x0d))
                            } else {
                                rgba(with_alpha(TEXT_MUTED, 0x0d))
                            })
                            .border_color(if connected {
                                rgba(with_alpha(SUCCESS, 0x33))
                            } else {
                                rgba(with_alpha(TEXT_MUTED, 0x33))
                            })
                            .tooltip(|window, cx| {
                                gpui_component::tooltip::Tooltip::new(
                                    "Active node — open Nodes (Ctrl+2)",
                                )
                                .build(window, cx)
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_view(ActiveView::Nodes, cx);
                            }))
                            .child(status_dot(if connected { SUCCESS } else { TEXT_MUTED }))
                            .child(
                                div()
                                    .text_size(px(MICRO))
                                    .font_family(MONO_FONT)
                                    .text_color(if connected {
                                        rgb(SUCCESS)
                                    } else {
                                        rgb(TEXT_SECONDARY)
                                    })
                                    .child(name),
                            )
                            .when(!latency.is_empty(), |this| {
                                this.child(
                                    div()
                                        .text_size(px(TINY))
                                        .font_family(MONO_FONT)
                                        .text_color(rgb(TEXT_MUTED))
                                        .child(latency),
                                )
                            })
                    })),
            )
            // ── Window controls (min / max / close) ─────────────────────
            .child(window_control_buttons(window))
    }

    pub(crate) fn render_statusbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let kind = connection_status_kind(self.proxy_running, &self.proxy_status);

        let (up, down) = if self.proxy_running {
            (
                format_speed(self.upload_speed_bps),
                format_speed(self.download_speed_bps),
            )
        } else {
            ("0 B/s".to_string(), "0 B/s".to_string())
        };

        let node_summary = self
            .active_proxy_node
            .or(self.selected_node)
            .and_then(|i| self.nodes.get(i))
            .map(|n| {
                let latency = n
                    .latency_ms
                    .map(|ms| format!(" · {}ms", ms))
                    .unwrap_or_default();
                format!("{}{}", n.name, latency)
            })
            .unwrap_or_else(|| "no node".to_string());

        div()
            .w_full()
            .h(px(26.0))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_3()
            .bg(rgb(BG_SIDEBAR))
            .border_t_1()
            .border_color(rgba(with_alpha(BORDER, 0x99)))
            // ── Left: language · status · node summary ─────────────────
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .min_w_0()
                    // Compact language link (bottom-left; no chip chrome)
                    .child(
                        div()
                            .id("statusbar-language")
                            .text_size(px(MICRO))
                            .font_family(MONO_FONT)
                            .text_color(rgb(TEXT_MUTED))
                            .cursor_pointer()
                            .hover(|s| s.text_color(rgb(TEXT_SECONDARY)))
                            .tooltip(|window, cx| {
                                gpui_component::tooltip::Tooltip::new(sockrocket_gui::i18n::t(
                                    "settings.language",
                                ))
                                .build(window, cx)
                            })
                            .child(sockrocket_gui::i18n::current_locale().id().to_uppercase())
                            .on_click(cx.listener(|this, _, _, cx| {
                                let next = sockrocket_gui::i18n::current_locale().next();
                                sockrocket_gui::i18n::set_locale(next);
                                this.schedule_persist(cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1p5()
                            .child(status_dot(kind.color()))
                            .child(
                                div()
                                    .text_size(px(MICRO))
                                    .font_family(MONO_FONT)
                                    .text_color(rgb(kind.color()))
                                    .child(kind.label()),
                            ),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .text_size(px(MICRO))
                            .font_family(MONO_FONT)
                            .text_color(rgb(TEXT_MUTED))
                            .child(node_summary),
                    ),
            )
            // ── Right: ports / throughput / nodes / version ─────────────
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .flex_shrink_0()
                    .child(statusbar_item(format!("SOCKS :{}", self.socks_port)))
                    .child(statusbar_item(format!("HTTP :{}", self.http_port)))
                    .child(
                        div()
                            .text_size(px(MICRO))
                            .font_family(MONO_FONT)
                            .text_color(rgb(ACCENT))
                            .child(format!("↑ {}", up)),
                    )
                    .child(
                        div()
                            .text_size(px(MICRO))
                            .font_family(MONO_FONT)
                            .text_color(rgb(SUCCESS))
                            .child(format!("↓ {}", down)),
                    )
                    .child(statusbar_item(format!("{} nodes", self.nodes.len())))
                    .child(statusbar_item(format!("v{}", env!("CARGO_PKG_VERSION")))),
            )
    }

    pub(crate) fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_view.clone();

        let mut nav = div()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .pt_2()
            .flex_1();

        for (i, (key, shortcut, icon, view)) in NAV_ITEMS.iter().enumerate() {
            // Separator before the secondary group (Settings), per v2
            if i == 6 {
                nav = nav.child(
                    div()
                        .w(px(20.0))
                        .h(px(1.0))
                        .my_1()
                        .bg(rgba(with_alpha(BORDER, 0x99))),
                );
            }
            let label = sockrocket_gui::i18n::t(key);
            nav = nav.child(self.nav_icon_item(
                key,
                label,
                shortcut,
                icon,
                view.clone(),
                &active,
                cx,
            ));
        }

        div()
            .w(px(38.0))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(BG_SIDEBAR))
            .border_r_1()
            .border_color(rgba(with_alpha(BORDER, 0x99)))
            .child(nav)
    }

    /// v2 icon-only nav button: 32×32, active = cyan/10 tinted bg + accent
    /// icon + 2px accent left bar; tooltip carries label + shortcut.
    #[allow(clippy::too_many_arguments)]
    fn nav_icon_item(
        &self,
        id_key: &str,
        label: &str,
        shortcut: &str,
        icon: &str,
        view: ActiveView,
        active: &ActiveView,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_active = *active == view;
        let tooltip_text = SharedString::from(format!("{} · {}", label, shortcut));

        div()
            .id(SharedString::from(format!("nav-{}", id_key)))
            .relative()
            .w(px(32.0))
            .h(px(32.0))
            .rounded(px(8.0))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .when(is_active, |d| d.bg(rgba(with_alpha(ACCENT, 0x1a))))
            .when(!is_active, |d| d.hover(|s| s.bg(rgb(BG_HOVER))))
            .tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(tooltip_text.clone()).build(window, cx)
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_view(view.clone(), cx);
            }))
            // Active indicator: 2px ACCENT bar on the left edge
            .child(
                div()
                    .absolute()
                    .left(px(-3.0))
                    .top(px(9.0))
                    .w(px(2.0))
                    .h(px(14.0))
                    .rounded_r(px(2.0))
                    .bg(if is_active {
                        rgb(ACCENT)
                    } else {
                        rgba(0x00000000)
                    }),
            )
            .child(nav_icon(icon, if is_active { ACCENT } else { TEXT_MUTED }))
    }
}

// === Chrome (macOS-leaning): title bar, labeled sidebar, status bar ===
// TitleBar ~44px: brand | drag | mode segmented control · node.
// Sidebar ~168px with icon + label (System Settings style).
// StatusBar 28px: quiet meta, hairline top.

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

/// Quiet status bar meta item.
fn statusbar_item(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(MICRO))
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
        "nav.subscriptions",
        "Ctrl+8",
        "icons/nav-nodes.svg",
        ActiveView::Config,
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
            .h(px(52.0))
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            // macOS traffic lights sit over the title bar; keep brand clear.
            .when(cfg!(target_os = "macos"), |el| el.pl(px(80.0)))
            .when(!cfg!(target_os = "macos"), |el| el.pl_3())
            .bg(rgb(BG_SIDEBAR))
            .border_b_1()
            .border_color(rgba(with_alpha(BORDER, 0xcc)))
            // ── Left: quiet brand mark ────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h_full()
                    .child(
                        div()
                            .w(px(22.0))
                            .h(px(22.0))
                            .rounded(px(6.0))
                            .bg(rgb(ACCENT))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::empty()
                                    .path("icons/sockrocket.svg")
                                    .with_size(Size::Size(px(12.0)))
                                    .text_color(rgb(0xffffff)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(PAGE_TITLE))
                            .font_weight(FontWeight::SEMIBOLD)
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
                    // Mode: NSSegmentedControl-style
                    .child(
                        seg_group()
                            .child(
                                seg_button(
                                    "mode-rule",
                                    sockrocket_gui::i18n::t("chrome.mode.rule"),
                                    self.proxy_mode == ProxyMode::Rule,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.set_proxy_mode(ProxyMode::Rule, cx);
                                    },
                                )),
                            )
                            .child(
                                seg_button(
                                    "mode-global",
                                    sockrocket_gui::i18n::t("chrome.mode.global"),
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
                                    sockrocket_gui::i18n::t("chrome.mode.direct"),
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
                    .children(active_node_info.map(|(name, latency)| {
                        let connected = self.proxy_running;
                        div()
                            .id("titlebar-node-badge")
                            .max_w(px(260.0))
                            .min_w_0()
                            .overflow_hidden()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1p5()
                            .px_2()
                            .py_0p5()
                            .rounded(px(6.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(BG_HOVER)))
                            .tooltip(|window, cx| {
                                gpui_component::tooltip::Tooltip::new(sockrocket_gui::i18n::t(
                                    "chrome.node_tip",
                                ))
                                .build(window, cx)
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_view(ActiveView::Nodes, cx);
                            }))
                            .child(status_dot(if connected { SUCCESS } else { TEXT_MUTED }))
                            .child(
                                div()
                                    .text_size(px(MICRO))
                                    .text_color(rgb(TEXT_PRIMARY))
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(name),
                            )
                            .when(!latency.is_empty(), |this| {
                                this.child(
                                    div()
                                        .text_size(px(TINY))
                                        .text_color(rgb(TEXT_SECONDARY))
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
            .unwrap_or_else(|| sockrocket_gui::i18n::t("chrome.no_node").to_string());

        div()
            .w_full()
            .h(px(28.0))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_4()
            .bg(rgb(BG_SIDEBAR))
            .border_t_1()
            .border_color(rgba(with_alpha(BORDER, 0xcc)))
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
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(TEXT_PRIMARY))
                                    .child(kind.label()),
                            ),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .text_size(px(MICRO))
                            .text_color(rgb(TEXT_SECONDARY))
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
                    .child(statusbar_item(format!("SOCKS {}", self.socks_port)))
                    .child(statusbar_item(format!("HTTP {}", self.http_port)))
                    .child(statusbar_item(format!("↑ {up}")))
                    .child(statusbar_item(format!("↓ {down}")))
                    .child(statusbar_item(format!("{} nodes", self.nodes.len())))
                    .child(statusbar_item(format!("v{}", env!("CARGO_PKG_VERSION")))),
            )
    }

    pub(crate) fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_view.clone();

        let mut nav = div().flex().flex_col().gap(px(4.0)).px_3().pt_4().flex_1();

        for (i, (key, shortcut, icon, view)) in NAV_ITEMS.iter().enumerate() {
            // Settings sits apart as a secondary group.
            if i == NAV_ITEMS.len() - 1 {
                nav = nav.child(
                    div()
                        .h(px(1.0))
                        .mx_2()
                        .my_2()
                        .bg(rgba(with_alpha(BORDER, 0xaa))),
                );
            }
            let label = sockrocket_gui::i18n::t(key);
            nav =
                nav.child(self.nav_row_item(key, label, shortcut, icon, view.clone(), &active, cx));
        }

        div()
            .w(px(224.0))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(BG_SIDEBAR))
            .border_r_1()
            .border_color(rgba(with_alpha(BORDER, 0xcc)))
            .child(
                div()
                    .id("sidebar-search")
                    .mx_3()
                    .mt_4()
                    .px_2()
                    .py_2()
                    .rounded(px(8.0))
                    .bg(rgba(0xffffff99))
                    .border_1()
                    .border_color(rgba(with_alpha(BORDER, 0x99)))
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|d| d.bg(rgb(BG_PANEL)))
                    .child(nav_icon("icons/search.svg", TEXT_SECONDARY))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(SMALL))
                            .text_color(rgb(TEXT_SECONDARY))
                            .child(sockrocket_gui::i18n::t("chrome.search")),
                    )
                    .child(
                        div()
                            .text_size(px(MICRO))
                            .text_color(rgb(TEXT_MUTED))
                            .child(if cfg!(target_os = "macos") {
                                "⌘K"
                            } else {
                                "Ctrl K"
                            }),
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.open_palette(window, cx))),
            )
            .child(nav)
    }

    /// Labeled sidebar row (System Settings style).
    #[allow(clippy::too_many_arguments)]
    fn nav_row_item(
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
        let shortcut = if cfg!(target_os = "macos") {
            shortcut.replace("Ctrl+", "⌘")
        } else {
            shortcut.to_string()
        };
        let tooltip_text = SharedString::from(format!("{} · {}", label, shortcut));

        div()
            .id(SharedString::from(format!("nav-{}", id_key)))
            .w_full()
            .h(px(36.0))
            .px_2()
            .rounded(px(8.0))
            .cursor_pointer()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .when(is_active, |d| d.bg(rgb(ACCENT)))
            .when(!is_active, |d| d.hover(|s| s.bg(rgb(BG_HOVER))))
            .tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(tooltip_text.clone()).build(window, cx)
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_view(view.clone(), cx);
            }))
            .child(
                div()
                    .size(px(24.0))
                    .rounded(px(6.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgb(match id_key {
                        "nav.dashboard" => ACCENT,
                        "nav.nodes" => 0x34a853,
                        "nav.groups" => 0x8856d9,
                        "nav.connections" => 0x00a3a3,
                        "nav.rules" => 0xf09628,
                        "nav.logs" => 0x6e7785,
                        _ => 0x8b8d98,
                    }))
                    .child(nav_icon(icon, 0xffffff)),
            )
            .child(
                div()
                    .text_size(px(BODY))
                    .font_weight(if is_active {
                        FontWeight::MEDIUM
                    } else {
                        FontWeight::NORMAL
                    })
                    .text_color(rgb(if is_active { 0xffffff } else { TEXT_PRIMARY }))
                    .child(label.to_string()),
            )
    }
}

//! Design tokens & shared UI primitives for the Sockrocket console UI.
//!
//! macOS Settings palette (light appearance): system blue accent,
//! soft elevated surfaces, hairline separators, SF-style type scale.
//! Views should use these helpers instead of scattering magic values.

use gpui::prelude::FluentBuilder as _;
use gpui::*;

// === Semantic colors: macOS Settings light appearance ===

/// Main grouped window background.
pub(crate) const BG_APP: u32 = 0xf5f5f7;
/// White grouped content surfaces.
pub(crate) const BG_PANEL: u32 = 0xffffff;
/// Quiet sidebar and title-bar background.
pub(crate) const BG_SIDEBAR: u32 = 0xececf0;
pub(crate) const BG_HOVER: u32 = 0xe5e5ea;
pub(crate) const BG_ACTIVE_ROW: u32 = 0xe8f1ff;
pub(crate) const BORDER: u32 = 0xdcdce1;
pub(crate) const TEXT_PRIMARY: u32 = 0x1d1d1f;
pub(crate) const TEXT_SECONDARY: u32 = 0x6e6e73;
pub(crate) const TEXT_MUTED: u32 = 0x76767c;
pub(crate) const ACCENT: u32 = 0x007aff;
pub(crate) const SUCCESS: u32 = 0x248a3d;
pub(crate) const WARNING: u32 = 0x946200;
pub(crate) const DANGER: u32 = 0xc9342b;
pub(crate) const PURPLE: u32 = 0x8944ab;
pub(crate) const TEXT_ACCENT: u32 = 0x0065d1;

// === Type scale (px) — closer to macOS control sizes ===

/// Navigation / window brand.
pub(crate) const PAGE_TITLE: f32 = 13.0;
/// Page large title (macOS Settings-style).
pub(crate) const LARGE_TITLE: f32 = 24.0;
/// Hero connection status.
pub(crate) const HEADING: f32 = 22.0;
/// Section headers (sentence case, not shouty mono).
pub(crate) const SECTION_LABEL: f32 = 11.0;
/// Body / buttons.
pub(crate) const BODY: f32 = 13.0;
/// Secondary meta.
pub(crate) const SMALL: f32 = 12.0;
/// Compact chrome / status bar.
pub(crate) const MICRO: f32 = 11.0;
/// Badges.
pub(crate) const TINY: f32 = 10.0;

/// Latency color thresholds: <80 good, 80–200 fair, >200 poor.
pub(crate) fn latency_color(ms: u64) -> u32 {
    if ms < 80 {
        SUCCESS
    } else if ms < 200 {
        WARNING
    } else {
        DANGER
    }
}

/// Monospace for data (ports, IPs, logs). Prefer SF Mono on Apple platforms.
pub(crate) const MONO_FONT: &str = if cfg!(target_os = "macos") {
    "SF Mono"
} else {
    "JetBrains Mono"
};

/// Bake an alpha channel into an `0xRRGGBB` token, returning `0xRRGGBBAA`.
pub(crate) fn with_alpha(color: u32, alpha: u8) -> u32 {
    (color << 8) | alpha as u32
}

// === Component recipes ===

/// Alert strip severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AlertKind {
    Info,
    Success,
    Warning,
    Danger,
}

impl AlertKind {
    pub(crate) fn color(self) -> u32 {
        match self {
            AlertKind::Info => ACCENT,
            AlertKind::Success => SUCCESS,
            AlertKind::Warning => WARNING,
            AlertKind::Danger => DANGER,
        }
    }
}

/// Connection-state categories for status chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatusKind {
    Connected,
    Verifying,
    Connecting,
    Unreachable,
    Error,
    Stopped,
}

impl StatusKind {
    pub(crate) fn color(self) -> u32 {
        match self {
            StatusKind::Connected => SUCCESS,
            StatusKind::Verifying | StatusKind::Connecting => ACCENT,
            StatusKind::Unreachable | StatusKind::Error => DANGER,
            StatusKind::Stopped => TEXT_MUTED,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            StatusKind::Connected => "Connected",
            StatusKind::Verifying => "Verifying",
            StatusKind::Connecting => "Connecting",
            StatusKind::Unreachable => "Unreachable",
            StatusKind::Error => "Error",
            StatusKind::Stopped => "Not Connected",
        }
    }
}

/// Small uppercase-ish section caption (Settings style: secondary, medium).
pub(crate) fn section_label(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(SECTION_LABEL))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(TEXT_SECONDARY))
        .child(text.into())
}

/// 8px semantic status dot (slightly larger, Mac-like).
pub(crate) fn status_dot(color: u32) -> Div {
    div()
        .w(px(8.0))
        .h(px(8.0))
        .rounded_full()
        .flex_shrink_0()
        .bg(rgb(color))
}

/// In-page toggle tab.
pub(crate) fn tab_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
) -> Stateful<Div> {
    let btn = div()
        .id(id)
        .px_2()
        .py(px(4.0))
        .cursor_pointer()
        .border_b_2()
        .text_size(px(BODY));
    let btn = if active {
        btn.border_color(rgb(ACCENT)).text_color(rgb(TEXT_PRIMARY))
    } else {
        btn.border_color(rgba(0x00000000))
            .text_color(rgb(TEXT_MUTED))
            .hover(|s| s.text_color(rgb(TEXT_SECONDARY)))
    };
    btn.child(label.into())
}

/// Status / error strip: soft tint, no loud neon bar.
pub(crate) fn alert_strip(kind: AlertKind, msg: impl Into<SharedString>) -> Div {
    let color = kind.color();
    div()
        .flex()
        .flex_row()
        .w_full()
        .rounded(px(8.0))
        .bg(rgba(with_alpha(color, 0x18)))
        .child(
            div()
                .px_3()
                .py_2()
                .min_w_0()
                .text_size(px(SMALL))
                .text_color(rgb(color))
                .child(msg.into()),
        )
}

/// Detail key/value row.
pub(crate) fn kv_row(key: impl Into<SharedString>, value: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .py_1()
        .child(
            div()
                .flex_shrink_0()
                .text_size(px(SMALL))
                .text_color(rgb(TEXT_SECONDARY))
                .child(key.into()),
        )
        .child(
            div()
                .min_w_0()
                .text_size(px(SMALL))
                .text_color(rgb(TEXT_PRIMARY))
                .child(value.into()),
        )
}

/// Empty-list placeholder.
pub(crate) fn empty_state(hint: impl Into<SharedString>, action: Option<AnyElement>) -> Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_3()
        .p_8()
        .rounded(px(12.0))
        .border_1()
        .border_color(rgba(with_alpha(BORDER, 0xb0)))
        .bg(rgb(BG_PANEL))
        .child(
            div()
                .text_size(px(BODY))
                .text_color(rgb(TEXT_SECONDARY))
                .child(hint.into()),
        )
        .when_some(action, |d, action| d.child(action))
}

/// Grouped inset panel (macOS Settings-style), not a heavy bordered card.
pub(crate) fn card() -> Div {
    div()
        .p(px(16.0))
        .flex()
        .flex_col()
        .gap_2()
        .rounded(px(12.0))
        .border_1()
        .border_color(rgba(with_alpha(BORDER, 0xb0)))
        .bg(rgb(BG_PANEL))
}

/// Mini status badge.
pub(crate) fn mini_badge(text: impl Into<SharedString>, color: u32) -> Div {
    div()
        .px_1p5()
        .py(px(1.0))
        .rounded(px(4.0))
        .bg(rgba(with_alpha(color, 0x22)))
        .text_size(px(TINY))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(color))
        .child(text.into())
}

/// Protocol badge colors.
pub(crate) fn proto_badge_color(short: &str) -> u32 {
    match short {
        "vless" => ACCENT,
        "ss" => SUCCESS,
        "vmess" => PURPLE,
        "trojan" => WARNING,
        "hy2" | "tuic" => DANGER,
        _ => TEXT_MUTED,
    }
}

/// iOS / macOS toggle switch.
pub(crate) fn toggle_switch(id: impl Into<ElementId>, on: bool) -> Stateful<Div> {
    let track = div()
        .id(id)
        .w(px(36.0))
        .h(px(20.0))
        .rounded_full()
        .flex_shrink_0()
        .cursor_pointer()
        .bg(if on {
            rgb(SUCCESS)
        } else {
            rgba(with_alpha(TEXT_MUTED, 0x66))
        });
    track.child(
        div()
            .w(px(16.0))
            .h(px(16.0))
            .rounded_full()
            .mt(px(2.0))
            .ml(if on { px(18.0) } else { px(2.0) })
            .bg(rgb(0xffffff)),
    )
}

/// Throughput sparkline.
pub(crate) fn throughput_sparkline(upload: &[f64], download: &[f64], height: f32) -> Div {
    let max = upload
        .iter()
        .chain(download.iter())
        .cloned()
        .fold(1.0f64, f64::max);
    let n = upload.len().max(download.len());
    if n == 0 || max <= 1.0 {
        return div()
            .flex()
            .flex_col()
            .justify_end()
            .flex_1()
            .min_w_0()
            .h(px(height))
            .child(
                div()
                    .w_full()
                    .h(px(1.0))
                    .bg(rgba(with_alpha(TEXT_MUTED, 0x40))),
            );
    }
    let mut row = div()
        .flex()
        .flex_row()
        .items_end()
        .gap(px(1.0))
        .h(px(height))
        .flex_1()
        .min_w_0()
        .overflow_hidden();
    for i in 0..n {
        let up = upload.get(i).cloned().unwrap_or(0.0);
        let down = download.get(i).cloned().unwrap_or(0.0);
        let v = up.max(down);
        let frac = if max > 0.0 { v / max } else { 0.0 };
        let h = (frac * (height - 4.0) as f64).max(2.0) as f32;
        let color = if v <= 0.0 {
            rgba(with_alpha(TEXT_MUTED, 0x40))
        } else if down >= up {
            rgba(with_alpha(ACCENT, 0x99))
        } else {
            rgba(with_alpha(SUCCESS, 0x99))
        };
        row = row.child(div().w(px(2.0)).h(px(h)).rounded(px(1.0)).bg(color));
    }
    row
}

/// Filled primary action (Connect / Apply).
pub(crate) fn primary_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    div()
        .id(id)
        .px_4()
        .py(px(6.0))
        .rounded(px(8.0))
        .cursor_pointer()
        .bg(rgb(ACCENT))
        .text_size(px(SMALL))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(0xffffff))
        .hover(|s| s.bg(rgba(with_alpha(ACCENT, 0xcc))))
        .child(label.into())
}

/// Quiet secondary / tint button (no loud neon border).
pub(crate) fn tint_action(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    div()
        .id(id)
        .px_3()
        .py(px(5.0))
        .rounded(px(7.0))
        .cursor_pointer()
        .bg(rgba(with_alpha(ACCENT, 0x22)))
        .text_size(px(SMALL))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(TEXT_ACCENT))
        .hover(|s| s.bg(rgba(with_alpha(ACCENT, 0x33))))
        .child(label.into())
}

/// Segmented control segment (macOS NSSegmentedControl feel).
pub(crate) fn seg_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
) -> Stateful<Div> {
    let btn = div()
        .id(id)
        .px_3()
        .py(px(3.0))
        .rounded(px(6.0))
        .cursor_pointer()
        .text_size(px(MICRO))
        .font_weight(FontWeight::MEDIUM);
    let btn = if active {
        btn.bg(rgb(BG_PANEL)).text_color(rgb(TEXT_PRIMARY))
    } else {
        btn.text_color(rgb(TEXT_SECONDARY))
            .hover(|s| s.text_color(rgb(TEXT_PRIMARY)))
    };
    btn.child(label.into())
}

/// Capsule wrapper for a row of [`seg_button`]s (title-bar mode control).
pub(crate) fn seg_group() -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(1.0))
        .p(px(2.0))
        .rounded(px(8.0))
        .bg(rgba(with_alpha(BORDER, 0x66)))
}

/// Soft filter / scene chip (Nodes protocol chips, Rules presets).
pub(crate) fn filter_chip(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
) -> Stateful<Div> {
    let base = div()
        .id(id)
        .px_2p5()
        .py(px(4.0))
        .rounded(px(7.0))
        .text_size(px(MICRO))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer();
    if active {
        base.bg(rgba(with_alpha(ACCENT, 0x33)))
            .text_color(rgb(TEXT_PRIMARY))
            .child(label.into())
    } else {
        base.bg(rgba(with_alpha(BORDER, 0x66)))
            .text_color(rgb(TEXT_SECONDARY))
            .hover(|s| s.bg(rgb(BG_HOVER)).text_color(rgb(TEXT_PRIMARY)))
            .child(label.into())
    }
}

/// Compact inline metric (replaces heavy stat cards on Home).
pub(crate) fn inline_metric(
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
    value_color: u32,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .min_w(px(76.0))
        .flex_shrink_0()
        .child(
            div()
                .text_size(px(TINY))
                .text_color(rgb(TEXT_SECONDARY))
                .child(label.into()),
        )
        .child(
            div()
                .text_size(px(BODY))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(value_color))
                .child(value.into()),
        )
}

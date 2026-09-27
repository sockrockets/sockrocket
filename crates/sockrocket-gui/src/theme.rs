//! Design tokens & shared UI primitives for the Sockrocket console UI.
//!
//! Source of truth: `docs/design-system.md` §1–§4. All views must reference
//! these constants and helpers instead of scattering magic values.
//!
//! Token values match the spec exactly; the legacy `app.rs` palette is gone.

use gpui::prelude::FluentBuilder as _;
use gpui::*;

// === §1 Color tokens (v2: ui-prototype-v2/index.html tailwind config) ===

/// App background (`app` #080a10).
pub(crate) const BG_APP: u32 = 0x080a10;
/// Card / panel background (`surface` #0d1117).
pub(crate) const BG_PANEL: u32 = 0x0d1117;
/// Sidebar / statusbar background (`surface/80` ≈ #0d1117).
pub(crate) const BG_SIDEBAR: u32 = 0x0d1117;
/// Row / item hover background (`surface-hover` #131922).
pub(crate) const BG_HOVER: u32 = 0x131922;
/// Hairline border base color (`border-subtle` rgba(30,40,55,0.6) — apply as
/// `rgba(with_alpha(BORDER, 0x99))` to match the prototype's translucency).
pub(crate) const BORDER: u32 = 0x1e2837;
/// Primary readable text (#f0f2f5).
pub(crate) const TEXT_PRIMARY: u32 = 0xf0f2f5;
/// Secondary / meta text (#8b95a5).
pub(crate) const TEXT_SECONDARY: u32 = 0x8b95a5;
/// Muted labels / placeholders / disabled (#4a5568).
pub(crate) const TEXT_MUTED: u32 = 0x4a5568;
/// Brand accent: active / connecting / selected / links.
pub(crate) const ACCENT: u32 = 0x22d3ee;
/// Connected, low latency, success.
pub(crate) const SUCCESS: u32 = 0x34d399;
/// Warning, medium latency.
pub(crate) const WARNING: u32 = 0xfbbf24;
/// Failure, high latency, destructive actions.
pub(crate) const DANGER: u32 = 0xf87171;
/// Secondary accent: proxy groups, VMess protocol badges.
pub(crate) const PURPLE: u32 = 0xa78bfa;
/// Table row selected state (steadier than hover).
pub(crate) const BG_ACTIVE_ROW: u32 = 0x161d28;
/// Bright accent text variant (hover / active emphasis).
pub(crate) const TEXT_ACCENT: u32 = 0x7ee7f8;

// === §2 Type scale (px) ===

/// Page header title, semibold.
pub(crate) const PAGE_TITLE: f32 = 13.0;
/// Hero status heading, bold.
pub(crate) const HEADING: f32 = 16.0;
/// Section label, semibold + uppercase.
pub(crate) const SECTION_LABEL: f32 = 10.0;
/// Body text, buttons, forms.
pub(crate) const BODY: f32 = 12.0;
/// Auxiliary info, secondary table columns.
pub(crate) const SMALL: f32 = 11.0;
/// Data values, status bar, chips (usually mono).
pub(crate) const MICRO: f32 = 10.0;
/// Tiny status badges, minimal labels.
pub(crate) const TINY: f32 = 9.0;

/// Latency color thresholds (v2): <80 good, 80–200 fair, >200 poor.
pub(crate) fn latency_color(ms: u64) -> u32 {
    if ms < 80 {
        SUCCESS
    } else if ms < 200 {
        WARNING
    } else {
        DANGER
    }
}

/// Monospace font family for data (latency, ports, IPs, URLs, tokens, logs).
/// JetBrains Mono is embedded at startup (see main.rs), so every platform
/// renders the v2 `font-mono-data` typeface identically.
pub(crate) const MONO_FONT: &str = "JetBrains Mono";

/// Bake an alpha channel into an `0xRRGGBB` token, returning `0xRRGGBBAA`.
pub(crate) fn with_alpha(color: u32, alpha: u8) -> u32 {
    (color << 8) | alpha as u32
}

// === §4 Component recipes ===

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

/// Connection-state categories for [`status_pill`], mapped from the single
/// connection state machine (design-system §5.1).
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
            StatusKind::Stopped => "Stopped",
        }
    }
}

/// MICRO mono, muted, uppercase section label (e.g. "CONNECTION").
pub(crate) fn section_label(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(SECTION_LABEL))
        .font_family(MONO_FONT)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(TEXT_MUTED))
        .child(text.into().to_uppercase())
}

/// 6px semantic status dot.
pub(crate) fn status_dot(color: u32) -> Div {
    div()
        .w(px(6.0))
        .h(px(6.0))
        .rounded_full()
        .flex_shrink_0()
        .bg(rgb(color))
}

/// In-page toggle tab: active = 2px ACCENT bottom border + primary text;
/// inactive = muted text, hover brightens. No background fill.
///
/// Returns a `Stateful<Div>` so callers can attach `.on_click(...)`.
pub(crate) fn tab_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
) -> Stateful<Div> {
    let btn = div()
        .id(id)
        .px_2()
        .py(px(2.0))
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

/// Status / error strip: 3px semantic left bar + 8% tinted bg + SMALL text.
pub(crate) fn alert_strip(kind: AlertKind, msg: impl Into<SharedString>) -> Div {
    let color = kind.color();
    div()
        .flex()
        .flex_row()
        .w_full()
        .rounded(px(4.0))
        .bg(rgba(with_alpha(color, 0x14)))
        .child(div().w(px(3.0)).flex_shrink_0().bg(rgb(color)))
        .child(
            div()
                .px_2()
                .py_1p5()
                .min_w_0()
                .text_size(px(SMALL))
                .text_color(rgb(color))
                .child(msg.into()),
        )
}

/// Detail key/value row: key SMALL muted, value MICRO mono primary.
pub(crate) fn kv_row(key: impl Into<SharedString>, value: impl Into<SharedString>) -> Div {
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
                .child(key.into()),
        )
        .child(
            div()
                .min_w_0()
                .text_size(px(MICRO))
                .font_family(MONO_FONT)
                .text_color(rgb(TEXT_PRIMARY))
                .child(value.into()),
        )
}

/// Empty-list placeholder: bordered container, generous padding, centered
/// SMALL muted hint plus an optional primary action element.
pub(crate) fn empty_state(hint: impl Into<SharedString>, action: Option<AnyElement>) -> Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .p_6()
        .rounded(px(6.0))
        .border_1()
        .border_color(rgba(with_alpha(BORDER, 0x99)))
        .child(
            div()
                .text_size(px(SMALL))
                .text_color(rgb(TEXT_MUTED))
                .child(hint.into()),
        )
        .when_some(action, |d, action| d.child(action))
}

/// Panel card (v2 `glass-panel`): `BG_PANEL` + 1px translucent BORDER +
/// rounded-xl (12px) + p 14 + gap 8.
/// Never nest cards; use a hairline divider + [`section_label`] instead.
pub(crate) fn card() -> Div {
    div()
        .p(px(14.0))
        .flex()
        .flex_col()
        .gap_2()
        .rounded(px(12.0))
        .bg(rgb(BG_PANEL))
        .border_1()
        .border_color(rgba(with_alpha(BORDER, 0x99)))
}

// === v2 component recipes ===

/// Mini status badge: TINY mono, tinted bg + hairline border, rounded 3.
/// Used for protocol types, rule targets, group types in dense tables.
pub(crate) fn mini_badge(text: impl Into<SharedString>, color: u32) -> Div {
    div()
        .px_1()
        .rounded(px(3.0))
        .border_1()
        .border_color(rgba(with_alpha(color, 0x40)))
        .bg(rgba(with_alpha(color, 0x14)))
        .text_size(px(TINY))
        .font_family(MONO_FONT)
        .text_color(rgb(color))
        .child(text.into())
}

/// v2 protocol badge colors: VLESS cyan / SS green / VMess purple /
/// Trojan amber / Hysteria2+Tuic red.
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

/// iOS-style toggle switch (v2). Returns `Stateful<Div>` for `.on_click`.
pub(crate) fn toggle_switch(id: impl Into<ElementId>, on: bool) -> Stateful<Div> {
    let track = div()
        .id(id)
        .w(px(32.0))
        .h(px(17.0))
        .rounded_full()
        .flex_shrink_0()
        .cursor_pointer()
        .bg(if on {
            rgba(with_alpha(ACCENT, 0x4d))
        } else {
            rgba(with_alpha(TEXT_MUTED, 0x4d))
        });
    track.child(
        div()
            .w(px(13.0))
            .h(px(13.0))
            .rounded_full()
            .mt(px(2.0))
            .ml(if on { px(17.0) } else { px(2.0) })
            .bg(if on { rgb(ACCENT) } else { rgb(TEXT_MUTED) }),
    )
}

/// Throughput sparkline: up to 60 bars of 2px, green = upload / cyan = download.
/// Static render of the sampled history ring buffers (no animation, §9.1).
pub(crate) fn throughput_sparkline(upload: &[f64], download: &[f64], height: f32) -> Div {
    let max = upload
        .iter()
        .chain(download.iter())
        .cloned()
        .fold(1.0f64, f64::max);
    let n = upload.len().max(download.len());
    // Flat history reads as a glitchy dotted strip — render a clean hairline
    // baseline instead (v2 sparkline zero-state).
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
            rgba(with_alpha(ACCENT, 0x8c))
        } else {
            rgba(with_alpha(SUCCESS, 0x73))
        };
        row = row.child(div().w(px(2.0)).h(px(h)).rounded(px(1.0)).bg(color));
    }
    row
}

/// Segmented capsule button (mode switcher): active = tinted ACCENT bg +
/// border + accent text; inactive = muted text. `Stateful` for `.on_click`.
pub(crate) fn seg_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
) -> Stateful<Div> {
    let btn = div()
        .id(id)
        .px_2p5()
        .py_0p5()
        .rounded(px(6.0))
        .cursor_pointer()
        .border_1()
        .text_size(px(MICRO))
        .font_weight(FontWeight::MEDIUM);
    let btn = if active {
        btn.bg(rgba(with_alpha(ACCENT, 0x1a)))
            .border_color(rgba(with_alpha(ACCENT, 0x40)))
            .text_color(rgb(ACCENT))
    } else {
        btn.border_color(rgba(0x00000000))
            .text_color(rgb(TEXT_SECONDARY))
            .hover(|s| s.text_color(rgb(TEXT_PRIMARY)))
    };
    btn.child(label.into())
}

/// Dashboard stat card (v2): uppercase micro label / 24px bold mono value /
/// micro mono note.
pub(crate) fn stat_card(
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
    sub: impl Into<SharedString>,
    value_color: u32,
) -> Div {
    card()
        .p(px(16.0))
        .gap(px(4.0))
        .child(
            div()
                .text_size(px(MICRO))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(TEXT_MUTED))
                .child(label.into().to_uppercase()),
        )
        .child(
            div()
                .text_size(px(24.0))
                .font_family(MONO_FONT)
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(value_color))
                .child(value.into()),
        )
        .child(
            div()
                .text_size(px(MICRO))
                .font_family(MONO_FONT)
                .text_color(rgb(TEXT_MUTED))
                .child(sub.into()),
        )
}

// === Logs View (v2) ===
// Split impl block of `AppState` (moved from app.rs; behavior unchanged).
// Layout per ui-prototype-v2 §page-logs: tinted icon header + entry-count
// subtitle + Search focus button, level chips with per-level tints, Pause /
// Clear / Export icon buttons, glass table card (Time / Level / Target /
// Message) with internally scrolling mono body. Level filter, text filter,
// pause snapshot, ring-buffer reads and the 500-row cap are unchanged.

use crate::app::*;
use crate::log_buffer::LogEntry;
use crate::theme::*;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::Sizable as _;
use gpui_component::{Icon, Size as ComponentSize};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// How long the transient toolbar notice (e.g. export result) stays visible.
const NOTICE_TTL: Duration = Duration::from_secs(4);

/// Filtered log view cache: (buffer/snapshot generation, paused, level
/// filter, text filter, filtered entries).
type LogFilteredCache = (u64, bool, tracing::Level, String, Rc<Vec<LogEntry>>);

thread_local! {
    /// Display-only pause: when set, the view renders LOG_SNAPSHOT instead of the
    /// live buffer. Recording into the buffer is unaffected (pure UI state).
    static LOG_PAUSED: Cell<bool> = const { Cell::new(false) };
    /// Frozen entries shown while paused.
    static LOG_SNAPSHOT: RefCell<Vec<LogEntry>> = const { RefCell::new(Vec::new()) };
    /// Bumped each time a new snapshot is frozen, so the filtered cache can
    /// tell two pause sessions apart.
    static LOG_SNAPSHOT_GEN: Cell<u64> = const { Cell::new(0) };
    /// Transient toolbar feedback, cleared after NOTICE_TTL.
    static LOG_NOTICE: RefCell<Option<(SharedString, Instant)>> = const { RefCell::new(None) };
    /// Reversed copy of the live buffer, rebuilt only when the buffer's
    /// generation changes — cloning 500 entries every frame was measurable.
    static LOG_CACHE: RefCell<(u64, Rc<Vec<LogEntry>>)> = RefCell::new((0, Rc::new(Vec::new())));
    /// Filtered view of LOG_CACHE, keyed on (generation, paused, level filter,
    /// text filter) and rebuilt only when one of those changes — the filter
    /// pass lowercases message+target per entry, which at ~1k Strings per
    /// frame was the costliest part of the Logs render.
    static LOG_FILTERED_CACHE: RefCell<LogFilteredCache> = RefCell::new((
        u64::MAX,
        false,
        tracing::Level::INFO,
        String::new(),
        Rc::new(Vec::new()),
    ));
}

fn log_paused() -> bool {
    LOG_PAUSED.with(|p| p.get())
}

fn set_log_paused(paused: bool, snapshot: Vec<LogEntry>) {
    LOG_PAUSED.with(|p| p.set(paused));
    if paused {
        LOG_SNAPSHOT_GEN.with(|g| g.set(g.get().wrapping_add(1)));
        LOG_SNAPSHOT.with(|s| *s.borrow_mut() = snapshot);
    }
}

fn clear_log_snapshot() {
    LOG_SNAPSHOT.with(|s| s.borrow_mut().clear());
}

fn set_log_notice(msg: impl Into<SharedString>) {
    LOG_NOTICE.with(|n| *n.borrow_mut() = Some((msg.into(), Instant::now())));
}

fn active_log_notice() -> Option<SharedString> {
    LOG_NOTICE.with(|n| match &*n.borrow() {
        Some((msg, t)) if t.elapsed() < NOTICE_TTL => Some(msg.clone()),
        _ => None,
    })
}

/// "1247" → "1,247" for the header entry-count subtitle.
fn fmt_count(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// v2 icon-only toolbar button: rounded-lg, hairline border, hover brighten.
fn icon_btn(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .p(px(6.0))
        .rounded(px(8.0))
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(rgba(with_alpha(BORDER, 0x99)))
        .text_color(rgb(TEXT_MUTED))
        .cursor_pointer()
        .hover(|s| s.bg(rgb(BG_HOVER)).text_color(rgb(TEXT_PRIMARY)))
}

/// Two-bar pause glyph (no pause SVG asset; built from divs).
fn pause_glyph() -> Div {
    let bar = || {
        div()
            .w(px(2.0))
            .h(px(10.0))
            .rounded(px(1.0))
            .bg(rgb(TEXT_MUTED))
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(2.0))
        .child(bar())
        .child(bar())
}

impl AppState {
    pub(crate) fn render_logs(&mut self, cx: &mut Context<Self>) -> Div {
        let paused = log_paused();
        let generation = if paused {
            LOG_SNAPSHOT_GEN.with(|g| g.get())
        } else {
            self.log_buffer.lock().map(|b| b.generation).unwrap_or(0)
        };
        let entries: Rc<Vec<LogEntry>> = if paused {
            LOG_SNAPSHOT.with(|s| Rc::new(s.borrow().clone()))
        } else {
            LOG_CACHE.with(|c| {
                let mut cache = c.borrow_mut();
                if cache.0 != generation {
                    let fresh: Vec<LogEntry> = self
                        .log_buffer
                        .lock()
                        .map(|buf| buf.entries.iter().rev().cloned().collect())
                        .unwrap_or_default();
                    *cache = (generation, Rc::new(fresh));
                }
                cache.1.clone()
            })
        };

        let total_count = entries.len();
        let current_level = self.log_level_filter;

        // Sync filter from InputState (same pattern as node filter). The
        // stored value is already trimmed + lowercased.
        let filter_text = self.log_filter_input.read(cx).value().trim().to_lowercase();
        if filter_text != self.log_filter {
            self.log_filter = filter_text;
        }
        let log_filter = self.log_filter.clone();

        // Apply both level and text filters — generation-keyed so repaints
        // that change nothing reuse the previous result.
        let filtered: Rc<Vec<LogEntry>> = LOG_FILTERED_CACHE.with(|c| {
            let mut cache = c.borrow_mut();
            if cache.0 != generation
                || cache.1 != paused
                || cache.2 != current_level
                || cache.3 != log_filter
            {
                let fresh: Vec<LogEntry> = entries
                    .iter()
                    .filter(|e| {
                        // Level filter: only show entries at or above the selected level
                        let level_ok = e.level <= current_level;
                        let text_ok = log_filter.is_empty()
                            || e.message.to_lowercase().contains(&log_filter)
                            || e.target.to_lowercase().contains(&log_filter);
                        level_ok && text_ok
                    })
                    .cloned()
                    .collect();
                *cache = (
                    generation,
                    paused,
                    current_level,
                    log_filter.clone(),
                    Rc::new(fresh),
                );
            }
            cache.4.clone()
        });

        let log_filter_input = self.log_filter_input.clone();

        // Level chips (v2): always tinted per level; active = stronger tint.
        let level_tabs = {
            let levels: &[(&str, tracing::Level, u32)] = &[
                ("Error", tracing::Level::ERROR, DANGER),
                ("Warn", tracing::Level::WARN, WARNING),
                ("Info", tracing::Level::INFO, ACCENT),
                ("Debug", tracing::Level::DEBUG, TEXT_MUTED),
            ];
            let mut row = div().flex().flex_row().gap_1p5();
            for &(label, level, color) in levels {
                let active = current_level == level;
                let chip = div()
                    .id(SharedString::from(format!("log-lvl-{}", label)))
                    .px_2p5()
                    .py(px(4.0))
                    .rounded(px(8.0))
                    .border_1()
                    .text_size(px(TINY))
                    .font_weight(FontWeight::MEDIUM)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.log_level_filter = level;
                        cx.notify();
                    }));
                row = row.child(if active {
                    chip.bg(rgba(with_alpha(color, 0x1a)))
                        .border_color(rgba(with_alpha(color, 0x33)))
                        .text_color(rgb(color))
                        .child(label)
                } else {
                    chip.bg(rgba(with_alpha(color, 0x0d)))
                        .border_color(rgba(with_alpha(color, 0x1a)))
                        .text_color(rgb(TEXT_MUTED))
                        .hover(|s| s.text_color(rgb(TEXT_SECONDARY)))
                        .child(label)
                });
            }
            row
        };

        let content = div()
            .flex()
            .flex_col()
            .gap_3()
            .h_full()
            // === Page header (v2): tinted icon block + title + count | Search ===
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            // 32px cyan-tinted rounded-lg icon block
                            .child(
                                div()
                                    .w(px(32.0))
                                    .h(px(32.0))
                                    .rounded(px(8.0))
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .border_1()
                                    .bg(rgba(with_alpha(ACCENT, 0x1a)))
                                    .border_color(rgba(with_alpha(ACCENT, 0x33)))
                                    .child(
                                        Icon::empty()
                                            .path("icons/square-terminal.svg")
                                            .with_size(ComponentSize::Size(px(16.0)))
                                            .text_color(rgb(ACCENT)),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_size(px(PAGE_TITLE))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(TEXT_PRIMARY))
                                            .child("Logs"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(MICRO))
                                            .text_color(rgb(TEXT_MUTED))
                                            .child(format!(
                                                "{} entries · {}",
                                                fmt_count(total_count),
                                                if paused { "paused" } else { "live" }
                                            )),
                                    ),
                            ),
                    )
                    // Ghost Search button: focuses the text filter input
                    .child(
                        div()
                            .id("log-search-btn")
                            .px_3()
                            .py(px(6.0))
                            .rounded(px(8.0))
                            .border_1()
                            .border_color(rgba(with_alpha(BORDER, 0x99)))
                            .cursor_pointer()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1p5()
                            .text_size(px(SMALL))
                            .text_color(rgb(TEXT_SECONDARY))
                            .hover(|s| s.bg(rgb(BG_HOVER)).text_color(rgb(TEXT_PRIMARY)))
                            .child(
                                Icon::empty()
                                    .path("icons/search.svg")
                                    .with_size(ComponentSize::Size(px(14.0))),
                            )
                            .child("Search")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.log_filter_input.update(cx, |state, cx| {
                                    state.focus(window, cx);
                                });
                            })),
                    ),
            )
            // === Filter row (v2): level chips | spacer | filter input | icons ===
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(level_tabs)
                    .child(div().flex_1().min_w(px(80.0)))
                    .child(
                        div().w(px(240.0)).flex_shrink_0().child(
                            gpui_component::input::Input::new(&log_filter_input)
                                .xsmall()
                                .w_full(),
                        ),
                    )
                    // Pause / Resume (display freeze; recording continues)
                    .child(
                        if paused {
                            icon_btn("pause-logs")
                                .child(
                                    Icon::empty()
                                        .path("icons/chevron-right.svg")
                                        .with_size(ComponentSize::Size(px(14.0))),
                                )
                                .tooltip(|window, cx| {
                                    gpui_component::tooltip::Tooltip::new("Resume live log display")
                                        .build(window, cx)
                                })
                        } else {
                            icon_btn("pause-logs")
                                .child(pause_glyph())
                                .tooltip(|window, cx| {
                                    gpui_component::tooltip::Tooltip::new(
                                        "Freeze the log display (recording continues)",
                                    )
                                    .build(window, cx)
                                })
                        }
                        .on_click(cx.listener(|this, _, _, cx| {
                            if log_paused() {
                                set_log_paused(false, Vec::new());
                            } else {
                                let snapshot: Vec<LogEntry> = this
                                    .log_buffer
                                    .lock()
                                    .map(|buf| buf.entries.iter().rev().cloned().collect())
                                    .unwrap_or_default();
                                set_log_paused(true, snapshot);
                            }
                            cx.notify();
                        })),
                    )
                    // Clear buffer + snapshot
                    .child(
                        icon_btn("clear-logs")
                            .child(
                                Icon::empty()
                                    .path("icons/trash-2.svg")
                                    .with_size(ComponentSize::Size(px(14.0))),
                            )
                            .tooltip(|window, cx| {
                                gpui_component::tooltip::Tooltip::new("Clear all log entries")
                                    .build(window, cx)
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Ok(mut buf) = this.log_buffer.lock() {
                                    buf.entries.clear();
                                    buf.generation = buf.generation.wrapping_add(1);
                                }
                                clear_log_snapshot();
                                cx.notify();
                            })),
                    )
                    // Export visible entries to the clipboard
                    .child(
                        icon_btn("export-logs")
                            .child(
                                Icon::empty()
                                    .path("icons/clipboard.svg")
                                    .with_size(ComponentSize::Size(px(14.0))),
                            )
                            .tooltip(|window, cx| {
                                gpui_component::tooltip::Tooltip::new(
                                    "Copy visible log entries to the clipboard",
                                )
                                .build(window, cx)
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                let entries: Vec<LogEntry> = if log_paused() {
                                    LOG_SNAPSHOT.with(|s| s.borrow().clone())
                                } else {
                                    this.log_buffer
                                        .lock()
                                        .map(|buf| buf.entries.iter().rev().cloned().collect())
                                        .unwrap_or_default()
                                };
                                let level = this.log_level_filter;
                                let filter = this.log_filter.to_lowercase();
                                let mut text = String::new();
                                let mut count = 0usize;
                                for entry in entries
                                    .iter()
                                    .filter(|e| {
                                        e.level <= level
                                            && (filter.is_empty()
                                                || e.message.to_lowercase().contains(&filter)
                                                || e.target.to_lowercase().contains(&filter))
                                    })
                                    .take(500)
                                {
                                    text.push_str(&entry.to_string());
                                    text.push('\n');
                                    count += 1;
                                }
                                cx.write_to_clipboard(ClipboardItem::new_string(text));
                                set_log_notice(format!(
                                    "✓ Copied {} log entr{} to clipboard",
                                    count,
                                    if count == 1 { "y" } else { "ies" }
                                ));
                                cx.spawn(async move |weak, cx| {
                                    Timer::after(NOTICE_TTL).await;
                                    weak.update(cx, |_, cx| cx.notify()).ok();
                                })
                                .detach();
                                cx.notify();
                            })),
                    ),
            )
            .when(paused, |d| d.child(alert_strip(AlertKind::Info, "Paused")))
            .when_some(active_log_notice(), |d, msg| {
                d.child(alert_strip(AlertKind::Success, msg))
            });

        // === Log table (v2 glass card): header + internally scrolling rows ===
        let hcell = |text: &str, w: f32| {
            div()
                .w(px(w))
                .flex_shrink_0()
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(TINY))
                .text_color(rgb(TEXT_MUTED))
                .child(text.to_string())
        };
        let table_head = div()
            .flex()
            .flex_row()
            .items_center()
            .px_4()
            .py(px(8.0))
            .bg(rgba(with_alpha(BG_HOVER, 0x4d)))
            .border_b_1()
            .border_color(rgba(with_alpha(BORDER, 0x99)))
            .child(hcell("Time", 100.0))
            .child(hcell("Level", 60.0))
            .child(hcell("Target", 120.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_weight(FontWeight::MEDIUM)
                    .text_size(px(TINY))
                    .text_color(rgb(TEXT_MUTED))
                    .child("Message"),
            );

        let mut body = div().flex().flex_col().flex_1().min_h_0();
        if filtered.is_empty() {
            body = body.child(empty_state(
                if total_count == 0 {
                    "No log entries yet"
                } else {
                    "No entries match the filter"
                },
                None,
            ));
        } else {
            for (log_idx, entry) in filtered.iter().take(500).enumerate() {
                // v2 level badges: ERROR red / WARN amber / INFO cyan / DEBUG muted
                let (tag, tag_color) = match entry.level {
                    tracing::Level::ERROR => ("ERROR", DANGER),
                    tracing::Level::WARN => ("WARN", WARNING),
                    tracing::Level::INFO => ("INFO", ACCENT),
                    tracing::Level::DEBUG => ("DEBUG", TEXT_MUTED),
                    tracing::Level::TRACE => ("TRACE", TEXT_MUTED),
                };

                // Shorten target: e.g. "sockrocket_core::proxy::tun" → "proxy::tun"
                let short_target = entry
                    .target
                    .strip_prefix("sockrocket_core::")
                    .or_else(|| entry.target.strip_prefix("sockrocket_gui::"))
                    .or_else(|| entry.target.strip_prefix("sockrocket::"))
                    .unwrap_or(&entry.target);
                let target_text = if short_target.is_empty() {
                    "—"
                } else {
                    short_target
                };

                // v2: ERROR / WARN rows carry a faint semantic row tint.
                let row_tint = match entry.level {
                    tracing::Level::ERROR => rgba(with_alpha(DANGER, 0x0d)),
                    tracing::Level::WARN => rgba(with_alpha(WARNING, 0x0d)),
                    _ => rgba(0x00000000u32),
                };
                body = body.child(
                    div()
                        .id(SharedString::from(format!("log-{}", log_idx)))
                        .flex()
                        .flex_row()
                        .items_center()
                        .px_4()
                        .py(px(6.0))
                        .border_b_1()
                        .border_color(rgba(with_alpha(BORDER, 0x4d)))
                        .bg(row_tint)
                        .hover(|s| s.bg(rgba(with_alpha(BG_HOVER, 0x33))))
                        .child(
                            div()
                                .w(px(100.0))
                                .flex_shrink_0()
                                .text_size(px(TINY))
                                .font_family(MONO_FONT)
                                .text_color(rgb(TEXT_MUTED))
                                .child(entry.time_hms()),
                        )
                        .child(
                            div()
                                .w(px(60.0))
                                .flex_shrink_0()
                                .child(mini_badge(tag, tag_color)),
                        )
                        .child(
                            div()
                                .w(px(120.0))
                                .flex_shrink_0()
                                .overflow_x_hidden()
                                .text_size(px(TINY))
                                .font_family(MONO_FONT)
                                .text_color(rgb(ACCENT))
                                .child(target_text.to_string()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_x_hidden()
                                .text_size(px(TINY))
                                .font_family(MONO_FONT)
                                .text_color(rgb(TEXT_SECONDARY))
                                .child(entry.message.clone()),
                        ),
                );
            }
        }

        content.child(
            card()
                .p_0()
                .gap_0()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .child(table_head)
                .child(
                    div()
                        .id("log-table-body")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .child(body),
                ),
        )
    }
}

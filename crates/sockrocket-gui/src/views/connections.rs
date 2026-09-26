// === Connections View (live connection monitor, v2) ===
// Data source: `ProxyStats::recent_connections()` — a 120-entry ring of
// metadata-only records (dest/proto/bytes/duration) maintained by sockrocket-core.
// Only proxied connections are recorded (SOCKS5 always; HTTP only CONNECT
// tunnels), which matches the "what is going through the proxy" mental model.
//
// Layout per ui-prototype-v2 §page-connections: title + cyan active badge,
// glass table (Proto / Destination / Up / Down / Node / Rule), hover rows.

use crate::app::*;
use crate::theme::*;
use gpui::*;

impl AppState {
    pub(crate) fn render_connections(&mut self, cx: &mut Context<Self>) -> Div {
        let filter = self.conn_filter;
        let (records, active_n, total_n) = match self.proxy_stats.as_ref() {
            Some(stats) => (
                // Clone only what can be rendered (row cap is 60) — the proto
                // filter is applied inside the ring lock.
                stats.recent_connections(60, |r| filter.matches(r.proto)),
                stats.active_connections(),
                stats.total_connections(),
            ),
            None => (Vec::new(), 0, 0),
        };
        // Oldest → newest top-to-bottom; `recent_connections` yields newest
        // first. (This also means the 60-row cap now keeps the *newest*
        // records — previously it silently dropped them.)
        let rows: Vec<_> = records.into_iter().rev().collect();

        // The node a proxied connection egresses through (v2 "Node" column).
        let active_node_name = self
            .active_proxy_node
            .or(self.selected_node)
            .and_then(|i| self.nodes.get(i))
            .map(|n| n.name.clone())
            .filter(|_| self.proxy_running);

        // --- Header (v2): title + cyan active badge | total count ──
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .pb_1()
            .mb_1()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(PAGE_TITLE))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(TEXT_PRIMARY))
                            .child("Connections"),
                    )
                    .child(mini_badge(format!("{} active", active_n), ACCENT)),
            )
            .child(
                div()
                    .text_size(px(MICRO))
                    .font_family(MONO_FONT)
                    .text_color(rgb(TEXT_MUTED))
                    .child(format!("{} total", total_n)),
            );

        // --- Filter chips ---
        let mut chips = div().flex().flex_row().gap_1().mb_2();
        for f in [ConnFilter::All, ConnFilter::Socks5, ConnFilter::Http] {
            chips = chips.child(
                seg_button(
                    SharedString::from(format!("conn-filter-{}", f.label())),
                    f.label(),
                    f == filter,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.conn_filter = f;
                    cx.notify();
                })),
            );
        }

        // --- Table ---
        let table_head = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .py(px(8.0))
            .border_b_1()
            .border_color(rgba(with_alpha(BORDER, 0x99)))
            .child(head_col("proto", 32.0))
            .child(div().flex_1().min_w_0().child(head_text("destination")))
            .child(head_col("up", 56.0))
            .child(head_col("down", 56.0))
            .child(head_col("node", 80.0))
            .child(
                div()
                    .w(px(56.0))
                    .flex_shrink_0()
                    .flex()
                    .justify_center()
                    .child(head_text("rule")),
            );

        let mut body = div().flex().flex_col().flex_1().min_h_0();
        if rows.is_empty() {
            let hint = if self.proxy_running {
                "No connections recorded yet — traffic will appear here."
            } else {
                "Start the proxy to monitor live connections."
            };
            body = body.child(empty_state(hint, None));
        } else {
            // Row cap (60) already applied by recent_connections inside the lock.
            for r in rows.into_iter() {
                // v2: destination splits into host (sans primary) + :port (micro muted)
                let (host, port) = match r.dest.rfind(':') {
                    Some(pos) => (r.dest[..pos].to_string(), r.dest[pos..].to_string()),
                    None => (r.dest.clone(), String::new()),
                };
                // v2: proxied rows name the egress node; direct rows show "—".
                let direct = !r.active && (active_node_name.is_none() || !self.proxy_running);
                let (node_text, node_color) = if direct {
                    ("—".to_string(), TEXT_MUTED)
                } else {
                    (
                        active_node_name.clone().unwrap_or_else(|| "—".to_string()),
                        ACCENT,
                    )
                };
                let rule_badge = if self.proxy_mode == sockrocket_core::ProxyMode::Direct || direct
                {
                    rule_badge("direct", SUCCESS)
                } else {
                    rule_badge("proxy", ACCENT)
                };
                let row = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgba(with_alpha(BORDER, 0x66)))
                    .hover(|s| s.bg(rgb(BG_HOVER)))
                    .child(
                        // v2: proto is plain mono muted text
                        div()
                            .w(px(32.0))
                            .flex_shrink_0()
                            .text_size(px(MICRO))
                            .font_family(MONO_FONT)
                            .text_color(rgb(TEXT_MUTED))
                            .child(r.proto),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .flex()
                            .flex_row()
                            .items_baseline()
                            .child(
                                div()
                                    .text_size(px(SMALL))
                                    .text_color(rgb(TEXT_PRIMARY))
                                    .child(host),
                            )
                            .child(
                                div()
                                    .text_size(px(MICRO))
                                    .font_family(MONO_FONT)
                                    .text_color(rgb(TEXT_MUTED))
                                    .child(port),
                            ),
                    )
                    .child(data_col(format_bytes(r.bytes_up), 56.0, TEXT_MUTED))
                    .child(data_col(format_bytes(r.bytes_down), 56.0, TEXT_MUTED))
                    .child(
                        div()
                            .w(px(80.0))
                            .flex_shrink_0()
                            .overflow_hidden()
                            .text_size(px(MICRO))
                            .font_family(MONO_FONT)
                            .text_color(rgb(node_color))
                            .child(node_text),
                    )
                    .child(
                        div()
                            .w(px(56.0))
                            .flex_shrink_0()
                            .flex()
                            .justify_center()
                            .child(rule_badge),
                    );
                body = body.child(row);
            }
        }

        div()
            .flex()
            .flex_col()
            .gap_3()
            .h_full()
            .child(header)
            .child(chips)
            .child(
                card()
                    .p_0()
                    .gap_0()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(table_head)
                    .child(
                        div()
                            .id("conn-table-body")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(body),
                    ),
            )
    }
}

fn head_text(label: &str) -> Div {
    div()
        .text_size(px(MICRO))
        .font_family(MONO_FONT)
        .text_color(rgb(TEXT_MUTED))
        .child(label.to_uppercase())
}

fn head_col(label: &str, w: f32) -> Div {
    div().w(px(w)).flex_shrink_0().child(head_text(label))
}

fn data_col(text: String, w: f32, color: u32) -> Div {
    div()
        .w(px(w))
        .flex_shrink_0()
        .overflow_hidden()
        .text_size(px(MICRO))
        .font_family(MONO_FONT)
        .text_color(rgb(color))
        .child(text)
}

/// v2 rule target badge: tinted bg + hairline border, centered content.
fn rule_badge(text: &str, color: u32) -> Div {
    div()
        .px_1p5()
        .py(px(2.0))
        .rounded(px(4.0))
        .bg(rgba(with_alpha(color, 0x0d)))
        .child(
            div()
                .text_size(px(MICRO))
                .text_color(rgb(color))
                .child(text.to_string()),
        )
}

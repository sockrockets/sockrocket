// === Command palette (Ctrl+K overlay) ===
// Fuzzy jump across pages / actions / nodes, per ui-prototype-v3.
// Rendered as a deferred full-window overlay so it paints above all pages.

use crate::app::*;
use crate::theme::*;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::Sizable as _;
use gpui_component::input::Input;

impl AppState {
    pub(crate) fn render_palette(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if !self.palette_open {
            return div().into_any_element();
        }

        let items = self.palette_items(cx);
        let count = items.len();

        let mut list = div()
            .id("palette-list")
            .flex()
            .flex_col()
            .py_1()
            .max_h(px(320.0))
            .overflow_y_scroll();
        if items.is_empty() {
            list = list.child(
                div()
                    .px_3()
                    .py_4()
                    .text_size(px(SMALL))
                    .text_color(rgb(TEXT_MUTED))
                    .child("No matches"),
            );
        }
        for (i, item) in items.into_iter().take(12).enumerate() {
            let kind_color = match item.kind {
                "page" => PURPLE,
                "action" => ACCENT,
                _ => SUCCESS,
            };
            let selected = i == self.palette_index;
            let row = div()
                .id(SharedString::from(format!("palette-item-{}", i)))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .py_1p5()
                .cursor_pointer()
                .when(selected, |d| d.bg(rgba(with_alpha(ACCENT, 0x1a))))
                .hover(|s| s.bg(rgba(with_alpha(ACCENT, 0x14))))
                .child(
                    div()
                        .w(px(56.0))
                        .flex_shrink_0()
                        .child(mini_badge(item.kind, kind_color)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(SMALL))
                        .text_color(rgb(if selected { ACCENT } else { TEXT_PRIMARY }))
                        .child(item.label.clone()),
                )
                .when(!item.hint.is_empty(), |d| {
                    d.child(
                        div()
                            .text_size(px(TINY))
                            .font_family(MONO_FONT)
                            .text_color(rgb(TEXT_MUTED))
                            .child(item.hint.clone()),
                    )
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.run_palette_item(item.clone(), cx);
                }));
            list = list.child(row);
        }

        deferred(
            div()
                .id("palette-backdrop")
                .absolute()
                .inset_0()
                .size_full()
                .bg(rgba(0x000000a0))
                .flex()
                .items_start()
                .justify_center()
                .pt(px(96.0))
                .on_click(cx.listener(|this, _, _, cx| this.close_palette(cx)))
                .child(
                    div()
                        .key_context("Palette")
                        .w(px(420.0))
                        .flex_shrink_0()
                        .rounded(px(8.0))
                        .bg(rgb(BG_PANEL))
                        .border_1()
                        .border_color(rgba(with_alpha(ACCENT, 0x40)))
                        .shadow_lg()
                        .occlude()
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .px_3()
                                .py_2()
                                .border_b_1()
                                .border_color(rgba(with_alpha(BORDER, 0x99)))
                                .child(
                                    div()
                                        .text_size(px(BODY))
                                        .font_family(MONO_FONT)
                                        .text_color(rgb(ACCENT))
                                        .child(">_"),
                                )
                                .child(Input::new(&self.palette_input).small().w_full()),
                        )
                        .child(list)
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .px_3()
                                .py_1p5()
                                .border_t_1()
                                .border_color(rgba(with_alpha(BORDER, 0x99)))
                                .child(
                                    div()
                                        .text_size(px(TINY))
                                        .text_color(rgb(TEXT_MUTED))
                                        .child("↑↓ navigate · Enter run · Esc close"),
                                )
                                .child(
                                    div()
                                        .text_size(px(TINY))
                                        .font_family(MONO_FONT)
                                        .text_color(rgb(TEXT_MUTED))
                                        .child(format!("{} results", count)),
                                ),
                        ),
                ),
        )
        .with_priority(1)
        .into_any_element()
    }
}

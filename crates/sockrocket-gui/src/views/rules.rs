//! Rules view — compact Clash/Surge-style layout: add/batch → list → optional templates.

use crate::app::*;
use crate::theme::*;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::{Icon, Sizable as _};
use sockrocket_core::{ProxyMode, RoutingRule, builtin_rule_scenes};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

const CONFIRM_WINDOW_MS: u64 = 3000;
const NO_RULE: usize = usize::MAX;

static HOVERED_RULE: AtomicUsize = AtomicUsize::new(NO_RULE);
static CLEAR_CONFIRM_AT: AtomicU64 = AtomicU64::new(0);
static DELETE_CONFIRM_INDEX: AtomicUsize = AtomicUsize::new(NO_RULE);
static DELETE_CONFIRM_AT: AtomicU64 = AtomicU64::new(0);
static GROUP_DEL_AT: AtomicU64 = AtomicU64::new(0);
static GROUP_DEL_KEY: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn hovered_rule() -> Option<usize> {
    match HOVERED_RULE.load(Ordering::Relaxed) {
        NO_RULE => None,
        i => Some(i),
    }
}

fn set_hovered_rule(index: Option<usize>) {
    HOVERED_RULE.store(index.unwrap_or(NO_RULE), Ordering::Relaxed);
}

fn confirm_slot_armed(slot: &AtomicU64) -> bool {
    let armed_at = slot.load(Ordering::Relaxed);
    armed_at != 0 && now_millis().saturating_sub(armed_at) < CONFIRM_WINDOW_MS
}

fn delete_confirm_armed(index: usize) -> bool {
    DELETE_CONFIRM_INDEX.load(Ordering::Relaxed) == index && confirm_slot_armed(&DELETE_CONFIRM_AT)
}

fn group_delete_armed(key: &str) -> bool {
    let Ok(guard) = GROUP_DEL_KEY.lock() else {
        return false;
    };
    *guard == key && confirm_slot_armed(&GROUP_DEL_AT)
}

fn arm_group_delete(key: &str) {
    if let Ok(mut guard) = GROUP_DEL_KEY.lock() {
        *guard = key.to_string();
    }
    GROUP_DEL_AT.store(now_millis(), Ordering::Relaxed);
}

fn disarm_delete_confirm() {
    DELETE_CONFIRM_INDEX.store(NO_RULE, Ordering::Relaxed);
    DELETE_CONFIRM_AT.store(0, Ordering::Relaxed);
}

fn schedule_confirm_reset(this: &AppState, cx: &mut Context<AppState>) {
    let handle = this.tokio_handle.clone();
    cx.spawn(async move |weak, cx| {
        handle
            .spawn(async {
                tokio::time::sleep(std::time::Duration::from_millis(CONFIRM_WINDOW_MS + 50)).await;
            })
            .await
            .ok();
        weak.update(cx, |_, cx| cx.notify()).ok();
    })
    .detach();
}

fn rule_matches_query(rule: &RoutingRule, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    rule.pattern.to_lowercase().contains(&q)
        || rule.rule_type.to_lowercase().contains(&q)
        || rule.target.to_lowercase().contains(&q)
        || rule.group.to_lowercase().contains(&q)
}

const TYPE_OPTIONS: &[(&str, &str)] = &[
    ("domain-suffix", "rules.kind.domain-suffix"),
    ("domain", "rules.kind.domain"),
    ("domain-keyword", "rules.kind.domain-keyword"),
    ("ip-cidr", "rules.kind.ip-cidr"),
    ("geoip", "rules.kind.geoip"),
    ("dst-port", "rules.kind.dst-port"),
    ("match", "rules.kind.match"),
];

const TARGET_OPTIONS: &[(&str, &str)] = &[
    ("proxy", "rules.act.proxy"),
    ("direct", "rules.act.direct"),
    ("reject", "rules.act.reject"),
];

fn short_type(rule_type: &str) -> &'static str {
    let key = TYPE_OPTIONS
        .iter()
        .find(|(value, _)| {
            *value == rule_type
                || (*value == "dst-port" && rule_type == "port")
                || (*value == "match" && rule_type == "final")
        })
        .map(|(_, key)| *key)
        .unwrap_or("rules.kind.domain-suffix");
    sockrocket_gui::i18n::t(key)
}

fn short_target(target: &str) -> &'static str {
    let key = TARGET_OPTIONS
        .iter()
        .find(|(value, _)| *value == target)
        .map(|(_, key)| *key)
        .unwrap_or("rules.act.proxy");
    sockrocket_gui::i18n::t(key)
}

/// `on` = all enabled, `partial` = mixed (border only).
fn quiet_mark(id: impl Into<ElementId>, on: bool, partial: bool) -> Stateful<Div> {
    let border = if on || partial { ACCENT } else { TEXT_MUTED };
    let fill = if on {
        rgb(ACCENT)
    } else if partial {
        rgba(with_alpha(ACCENT, 0x55))
    } else {
        rgba(0x00000000)
    };
    div()
        .id(id)
        .w(px(10.0))
        .h(px(10.0))
        .rounded_full()
        .flex_shrink_0()
        .cursor_pointer()
        .border_1()
        .border_color(rgb(border))
        .bg(fill)
}

fn display_pattern(rule: &RoutingRule) -> String {
    if matches!(rule.rule_type.as_str(), "match" | "final") {
        sockrocket_gui::i18n::t("rules.match.label").to_string()
    } else {
        rule.pattern.clone()
    }
}

impl AppState {
    pub(crate) fn render_rules(&mut self, cx: &mut Context<Self>) -> Div {
        let rule_count = self.rules.len();
        let rules_status = self.rules_status.clone();
        let clear_armed = confirm_slot_armed(&CLEAR_CONFIRM_AT);
        let batch_group = self.batch_edit_group.clone();

        let mut content = div().flex().flex_col().gap_2p5().child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .child(self.page_header_v3(
                    "icons/nav-rules.svg",
                    ACCENT,
                    sockrocket_gui::i18n::t("rules.title"),
                    sockrocket_gui::i18n::t("rules.subtitle"),
                ))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .id("rules-export")
                                .cursor_pointer()
                                .text_size(px(TINY))
                                .text_color(rgb(TEXT_MUTED))
                                .hover(|s| s.text_color(rgb(TEXT_PRIMARY)))
                                .child(sockrocket_gui::i18n::t("rules.export").to_string())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.export_rules_to_clipboard(cx);
                                })),
                        )
                        .child(
                            div()
                                .id("clear-rules")
                                .cursor_pointer()
                                .text_size(px(TINY))
                                .text_color(rgb(if clear_armed { DANGER } else { TEXT_MUTED }))
                                .hover(|s| s.text_color(rgb(TEXT_PRIMARY)))
                                .child(if clear_armed {
                                    sockrocket_gui::i18n::t("rules.clear_confirm").to_string()
                                } else {
                                    sockrocket_gui::i18n::t("rules.clear").to_string()
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if confirm_slot_armed(&CLEAR_CONFIRM_AT) {
                                        CLEAR_CONFIRM_AT.store(0, Ordering::Relaxed);
                                        this.clear_rules(cx);
                                    } else {
                                        CLEAR_CONFIRM_AT.store(now_millis(), Ordering::Relaxed);
                                        schedule_confirm_reset(this, cx);
                                        cx.notify();
                                    }
                                })),
                        ),
                ),
        );
        let filter_q = self
            .rule_filter_input
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        let rule_filter_input = self.rule_filter_input.clone();
        let new_group_open = self.rules_new_group_open;
        content = content.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    div().flex_1().min_w(px(120.0)).child(
                        gpui_component::input::Input::new(&rule_filter_input)
                            .xsmall()
                            .w_full(),
                    ),
                )
                .when(!new_group_open, |d| {
                    d.child(
                        div()
                            .id("open-new-rule-group")
                            .cursor_pointer()
                            .text_size(px(TINY))
                            .text_color(rgb(TEXT_MUTED))
                            .hover(|s| s.text_color(rgb(TEXT_PRIMARY)))
                            .child(sockrocket_gui::i18n::t("rules.group.new").to_string())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.rules_new_group_open = true;
                                this.rule_group_input.update(cx, |state, cx| {
                                    state.set_value("", window, cx);
                                    state.focus(window, cx);
                                });
                                cx.notify();
                            })),
                    )
                }),
        );
        if new_group_open {
            content = content.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1p5()
                    .rounded(px(6.0))
                    .bg(rgba(with_alpha(BG_HOVER, 0x66)))
                    .child(
                        div().flex_1().min_w(px(100.0)).child(
                            gpui_component::input::Input::new(&self.rule_group_input)
                                .xsmall()
                                .w_full(),
                        ),
                    )
                    .child(
                        div()
                            .id("confirm-new-rule-group")
                            .cursor_pointer()
                            .text_size(px(TINY))
                            .text_color(rgb(ACCENT))
                            .child(sockrocket_gui::i18n::t("rules.group.new").to_string())
                            .on_click(cx.listener(|this, _, window, cx| {
                                let name =
                                    this.rule_group_input.read(cx).value().trim().to_string();
                                if name.is_empty() {
                                    this.rules_status =
                                        sockrocket_gui::i18n::t("rules.status.group_name").into();
                                    cx.notify();
                                } else {
                                    this.begin_add_in_group(&name, window, cx);
                                }
                            })),
                    )
                    .child(
                        div()
                            .id("cancel-new-rule-group")
                            .cursor_pointer()
                            .text_size(px(TINY))
                            .text_color(rgb(TEXT_MUTED))
                            .child(sockrocket_gui::i18n::t("rules.cancel").to_string())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.rules_new_group_open = false;
                                this.rule_group_input.update(cx, |state, cx| {
                                    state.set_value("", window, cx);
                                });
                                cx.notify();
                            })),
                    ),
            );
        }
        content = content.children(if rules_status.is_empty() {
            None
        } else {
            Some(
                div()
                    .text_size(px(TINY))
                    .text_color(rgb(TEXT_MUTED))
                    .child(rules_status),
            )
        });

        let show_empty = rule_count == 0 && batch_group.is_none();
        if show_empty {
            content = content.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .pt_4()
                    .child(empty_state(
                        sockrocket_gui::i18n::t("rules.empty"),
                        Some(
                            div()
                                .id("empty-new-group")
                                .cursor_pointer()
                                .text_size(px(SMALL))
                                .text_color(rgb(TEXT_ACCENT))
                                .child(sockrocket_gui::i18n::t("rules.empty.cta").to_string())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.rules_new_group_open = true;
                                    this.rule_group_input.update(cx, |state, cx| {
                                        state.set_value("", window, cx);
                                        state.focus(window, cx);
                                    });
                                    cx.notify();
                                }))
                                .into_any_element(),
                        ),
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .items_center()
                            .gap_3()
                            .justify_center()
                            .child(
                                div()
                                    .text_size(px(TINY))
                                    .text_color(rgb(TEXT_MUTED))
                                    .child(sockrocket_gui::i18n::t("rules.scenes.title")),
                            )
                            .children(builtin_rule_scenes().iter().map(|scene| {
                                let id = scene.id;
                                div()
                                    .id(SharedString::from(format!("scene-empty-{id}")))
                                    .cursor_pointer()
                                    .text_size(px(TINY))
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .hover(|s| s.text_color(rgb(TEXT_PRIMARY)))
                                    .child(sockrocket_gui::i18n::t(scene.title_key).to_string())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.apply_rule_scene(id, cx);
                                    }))
                            })),
                    ),
            );
        } else {
            let groups = sockrocket_core::group_rules_for_display(&self.rules);
            let mut list = div().flex().flex_col().gap_3();
            let mut any_shown = false;
            for bucket in groups {
                let gkey = bucket.name.clone();
                let collapsed = self.rules_collapsed_groups.contains(&gkey);
                let title = if gkey.is_empty() {
                    sockrocket_gui::i18n::t("rules.group.ungrouped").to_string()
                } else {
                    gkey.clone()
                };
                let count = bucket.indices.len();
                let enabled_n = bucket
                    .indices
                    .iter()
                    .filter(|&&i| self.rules.get(i).map(|r| r.enabled).unwrap_or(false))
                    .count();
                let gkey_toggle = gkey.clone();
                let gkey_switch = gkey.clone();
                let gkey_edit = gkey.clone();
                let gkey_del = gkey.clone();
                let del_armed = group_delete_armed(&gkey);
                let all_on = count > 0 && enabled_n == count;
                let partial = enabled_n > 0 && enabled_n < count;

                let shown: Vec<usize> = bucket
                    .indices
                    .iter()
                    .copied()
                    .filter(|&i| {
                        filter_q.is_empty()
                            || self
                                .rules
                                .get(i)
                                .is_some_and(|r| rule_matches_query(r, &filter_q))
                    })
                    .collect();
                if !filter_q.is_empty() && shown.is_empty() {
                    continue;
                }
                any_shown = true;

                let count_label = if !filter_q.is_empty() && shown.len() != count {
                    format!("{} / {count}", shown.len())
                } else {
                    format!("{count}")
                };

                let header_group = SharedString::from(format!("grp-hover-{gkey}"));
                let batch_here = batch_group.as_deref() == Some(gkey.as_str());
                let mut section = div()
                    .group(header_group.clone())
                    .flex()
                    .flex_col()
                    .when(batch_here, |d| {
                        d.pl_2()
                            .border_l_2()
                            .border_color(rgba(with_alpha(ACCENT, 0x55)))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .py_1()
                            .child(
                                div()
                                    .id(SharedString::from(format!("grp-toggle-{gkey}")))
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_1p5()
                                    .flex_1()
                                    .min_w_0()
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.toggle_rule_group_collapsed(&gkey_toggle, cx);
                                    }))
                                    .child(
                                        div()
                                            .text_size(px(TINY))
                                            .text_color(rgb(TEXT_MUTED))
                                            .child(if collapsed { "▸" } else { "▾" }),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(BODY))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(TEXT_PRIMARY))
                                            .child(title),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(TINY))
                                            .text_color(rgb(TEXT_MUTED))
                                            .child(count_label),
                                    ),
                            )
                            .child(
                                quiet_mark(
                                    SharedString::from(format!("grp-switch-{gkey}")),
                                    all_on,
                                    partial,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.set_rule_group_enabled(&gkey_switch, !all_on, cx);
                                    },
                                )),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("grp-edit-{gkey}")))
                                    .cursor_pointer()
                                    .text_size(px(TINY))
                                    .text_color(rgb(if batch_here {
                                        TEXT_ACCENT
                                    } else {
                                        TEXT_MUTED
                                    }))
                                    .hover(|s| s.text_color(rgb(TEXT_PRIMARY)))
                                    .child(sockrocket_gui::i18n::t("rules.batch.mode").to_string())
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if this.batch_edit_group.as_deref()
                                            == Some(gkey_edit.as_str())
                                        {
                                            this.cancel_edit_rule(window, cx);
                                        } else {
                                            this.begin_batch_edit_group(&gkey_edit, window, cx);
                                        }
                                    })),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("grp-del-{gkey}")))
                                    .cursor_pointer()
                                    .text_size(px(TINY))
                                    .text_color(rgb(if del_armed { DANGER } else { TEXT_MUTED }))
                                    .when(!del_armed, |d| {
                                        d.invisible().group_hover(header_group, |s| s.visible())
                                    })
                                    .child(if del_armed {
                                        sockrocket_gui::i18n::t("rules.clear_confirm").to_string()
                                    } else {
                                        "×".to_string()
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if group_delete_armed(&gkey_del) {
                                            GROUP_DEL_AT.store(0, Ordering::Relaxed);
                                            this.delete_rule_group(&gkey_del, cx);
                                        } else {
                                            arm_group_delete(&gkey_del);
                                            schedule_confirm_reset(this, cx);
                                            cx.notify();
                                        }
                                    })),
                            ),
                    );

                if batch_here {
                    section = section.child(self.render_batch_editor(cx));
                } else if !collapsed || !filter_q.is_empty() {
                    let last = *shown.last().unwrap_or(&0);
                    for (pos, &i) in shown.iter().enumerate() {
                        section = section.child(self.render_rule_item(
                            i,
                            i == last || pos + 1 == shown.len(),
                            cx,
                        ));
                    }
                }
                list = list.child(section);
            }
            if let Some(name) = batch_group.clone() {
                let exists = sockrocket_core::group_rules_for_display(&self.rules)
                    .iter()
                    .any(|b| b.name == name);
                if !exists {
                    any_shown = true;
                    let title = if name.is_empty() {
                        sockrocket_gui::i18n::t("rules.group.ungrouped").to_string()
                    } else {
                        name
                    };
                    list = list.child(
                        div()
                            .pl_2()
                            .border_l_2()
                            .border_color(rgba(with_alpha(ACCENT, 0x55)))
                            .child(
                                div()
                                    .py_1()
                                    .text_size(px(BODY))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(TEXT_PRIMARY))
                                    .child(title),
                            )
                            .child(self.render_batch_editor(cx)),
                    );
                }
            }
            if !any_shown && !filter_q.is_empty() {
                list = list.child(
                    div()
                        .pl_4()
                        .py_2()
                        .text_size(px(TINY))
                        .text_color(rgb(TEXT_MUTED))
                        .child(sockrocket_gui::i18n::t("rules.filter.empty")),
                );
            }
            content = content.child(list);

            if rule_count > 0 {
                content = content.child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap_3()
                        .pt_1()
                        .child(
                            div()
                                .text_size(px(TINY))
                                .text_color(rgb(TEXT_MUTED))
                                .child(sockrocket_gui::i18n::t("rules.scenes.title")),
                        )
                        .children(builtin_rule_scenes().iter().map(|scene| {
                            let id = scene.id;
                            div()
                                .id(SharedString::from(format!("scene-{id}")))
                                .cursor_pointer()
                                .text_size(px(TINY))
                                .text_color(rgb(TEXT_SECONDARY))
                                .hover(|s| s.text_color(rgb(TEXT_PRIMARY)))
                                .child(sockrocket_gui::i18n::t(scene.title_key).to_string())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.apply_rule_scene(id, cx);
                                }))
                        })),
                );
            }
        }

        content
    }

    fn render_rule_menu(&mut self, menu: RuleMenu, cx: &mut Context<Self>) -> Div {
        let open = self.rule_menu == Some(menu);
        let (id, current, options): (&'static str, String, &'static [(&str, &str)]) = match menu {
            RuleMenu::Type => (
                "rule-dd-type",
                short_type(&self.rule_type_sel).to_string(),
                TYPE_OPTIONS,
            ),
            RuleMenu::Target => (
                "rule-dd-target",
                short_target(&self.rule_target_sel).to_string(),
                TARGET_OPTIONS,
            ),
        };
        let selected = match menu {
            RuleMenu::Type => self.rule_type_sel.clone(),
            RuleMenu::Target => self.rule_target_sel.clone(),
        };
        let mut panel = div()
            .mt_1()
            .py_1()
            .min_w(px(100.0))
            .rounded(px(6.0))
            .bg(rgb(BG_PANEL))
            .border_1()
            .border_color(rgba(with_alpha(BORDER, 0xaa)))
            .shadow_md()
            .occlude();
        for (value, key) in options {
            let value = *value;
            let active = selected == value
                || (menu == RuleMenu::Type && selected == "final" && value == "match")
                || (menu == RuleMenu::Type && selected == "port" && value == "dst-port");
            panel = panel.child(
                div()
                    .id(SharedString::from(format!("{id}-{value}")))
                    .px_2p5()
                    .py_1()
                    .cursor_pointer()
                    .text_size(px(SMALL))
                    .text_color(rgb(if active { TEXT_ACCENT } else { TEXT_SECONDARY }))
                    .hover(|s| s.bg(rgb(BG_HOVER)).text_color(rgb(TEXT_PRIMARY)))
                    .child(sockrocket_gui::i18n::t(key).to_string())
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            match menu {
                                RuleMenu::Type => this.select_rule_type(value, window, cx),
                                RuleMenu::Target => {
                                    this.rule_target_sel = value.to_string();
                                }
                            }
                            this.rule_menu = None;
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    ),
            );
        }
        div()
            .relative()
            .on_mouse_down_out(cx.listener(move |this, _, _, cx| {
                if this.rule_menu == Some(menu) {
                    this.rule_menu = None;
                    cx.notify();
                }
            }))
            .child(
                div()
                    .id(id)
                    .px_1()
                    .cursor_pointer()
                    .text_size(px(TINY))
                    .text_color(rgb(TEXT_SECONDARY))
                    .hover(|s| s.text_color(rgb(TEXT_PRIMARY)))
                    .child(format!("{current} ▾"))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.rule_menu = if this.rule_menu == Some(menu) {
                                None
                            } else {
                                Some(menu)
                            };
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    ),
            )
            .when(open, |d| {
                d.child(deferred(
                    anchored().snap_to_window_with_margin(px(8.0)).child(panel),
                ))
            })
    }

    /// Batch mode: edit the whole group as one-line-per-rule text.
    fn render_batch_editor(&mut self, cx: &mut Context<Self>) -> Div {
        let batch_input = self.rule_batch_input.clone();
        div()
            .ml_2()
            .mr_1()
            .mb_1()
            .mt_0p5()
            .px_2p5()
            .py_2()
            .rounded(px(6.0))
            .bg(rgba(with_alpha(BG_HOVER, 0x55)))
            .flex()
            .flex_col()
            .gap_1p5()
            .child(
                div()
                    .text_size(px(TINY))
                    .text_color(rgb(TEXT_MUTED))
                    .child(sockrocket_gui::i18n::t("rules.batch.hint").to_string()),
            )
            .child(
                div().w_full().child(
                    gpui_component::input::Input::new(&batch_input)
                        .xsmall()
                        .w_full(),
                ),
            )
            .child({
                let row = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(self.render_rule_menu(RuleMenu::Target, cx));
                row.child(div().flex_1())
                    .child(
                        div()
                            .id("batch-cancel")
                            .cursor_pointer()
                            .text_size(px(TINY))
                            .text_color(rgb(TEXT_MUTED))
                            .hover(|s| s.text_color(rgb(TEXT_PRIMARY)))
                            .child(sockrocket_gui::i18n::t("rules.cancel").to_string())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.cancel_edit_rule(window, cx);
                            })),
                    )
                    .child(
                        div()
                            .id("batch-save")
                            .px_2()
                            .py_0p5()
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .text_size(px(TINY))
                            .font_weight(FontWeight::MEDIUM)
                            .bg(rgba(with_alpha(ACCENT, 0x22)))
                            .text_color(rgb(TEXT_ACCENT))
                            .hover(|s| s.bg(rgba(with_alpha(ACCENT, 0x33))))
                            .child(sockrocket_gui::i18n::t("rules.batch.save").to_string())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.rule_menu = None;
                                this.save_batch_edit_group(window, cx);
                            })),
                    )
            })
    }

    fn render_inline_editor(&mut self, index: usize, is_last: bool, cx: &mut Context<Self>) -> Div {
        let cur_type = self.rule_type_sel.clone();
        let pattern_input = self.rule_pattern_input.clone();
        let is_final = matches!(cur_type.as_str(), "match" | "final");
        let is_geoip = cur_type == "geoip";
        div()
            .ml_2()
            .mr_1()
            .my_0p5()
            .px_2p5()
            .py_1p5()
            .rounded(px(6.0))
            .bg(rgba(with_alpha(ACCENT, 0x10)))
            .flex()
            .flex_col()
            .gap_1()
            .when(is_geoip, |d| {
                d.child(
                    div()
                        .text_size(px(TINY))
                        .text_color(rgb(TEXT_MUTED))
                        .child(sockrocket_gui::i18n::t("rules.hint.geoip").to_string()),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .when(!is_last, |d| d)
                    .child(if is_final {
                        div()
                            .flex_1()
                            .text_size(px(SMALL))
                            .text_color(rgb(TEXT_MUTED))
                            .child(sockrocket_gui::i18n::t("rules.hint.match"))
                            .into_any_element()
                    } else {
                        div()
                            .flex_1()
                            .min_w(px(80.0))
                            .child(
                                gpui_component::input::Input::new(&pattern_input)
                                    .xsmall()
                                    .w_full(),
                            )
                            .into_any_element()
                    })
                    .child(self.render_rule_menu(RuleMenu::Type, cx))
                    .child(self.render_rule_menu(RuleMenu::Target, cx))
                    .child(
                        div()
                            .id(SharedString::from(format!("inline-cancel-{index}")))
                            .cursor_pointer()
                            .text_size(px(TINY))
                            .text_color(rgb(TEXT_MUTED))
                            .child(sockrocket_gui::i18n::t("rules.cancel").to_string())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.cancel_edit_rule(window, cx);
                            })),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("inline-save-{index}")))
                            .px_2()
                            .py_0p5()
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .text_size(px(TINY))
                            .font_weight(FontWeight::MEDIUM)
                            .bg(rgba(with_alpha(ACCENT, 0x22)))
                            .text_color(rgb(TEXT_ACCENT))
                            .child(sockrocket_gui::i18n::t("rules.save").to_string())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_rule(window, cx);
                            })),
                    ),
            )
    }

    pub(crate) fn render_rule_item(
        &mut self,
        index: usize,
        is_last: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.editing_rule_index == Some(index) {
            return self
                .render_inline_editor(index, is_last, cx)
                .into_any_element();
        }
        let rule = &self.rules[index];
        let is_enabled = rule.enabled;
        let pattern = display_pattern(rule);
        let meta = format!(
            "{} · {}",
            short_type(&rule.rule_type),
            short_target(&rule.target)
        );
        let pattern_color = if is_enabled { TEXT_PRIMARY } else { TEXT_MUTED };
        let del_armed = delete_confirm_armed(index);
        let weak = cx.entity().downgrade();
        let row_group = SharedString::from(format!("rule-row-{index}"));

        div()
            .id(SharedString::from(format!("rule-{index}")))
            .group(row_group.clone())
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .pl_4()
            .pr_1()
            .py_1()
            .rounded(px(4.0))
            .when(!is_last, |d| {
                d.border_b_1().border_color(rgba(with_alpha(BORDER, 0x18)))
            })
            .when(!is_enabled, |d| d.opacity(0.4))
            .hover(|s| s.bg(rgba(with_alpha(BG_HOVER, 0x40))))
            .on_hover(move |is_hovered, _window, cx| {
                let next = if *is_hovered { Some(index) } else { None };
                if hovered_rule() != next {
                    set_hovered_rule(next);
                    weak.update(cx, |_, cx| cx.notify()).ok();
                }
            })
            .child(
                div()
                    .id(SharedString::from(format!("rule-pat-{index}")))
                    .flex_1()
                    .min_w_0()
                    .cursor_pointer()
                    .text_size(px(SMALL))
                    .font_family(MONO_FONT)
                    .text_color(rgb(pattern_color))
                    .overflow_x_hidden()
                    .child(pattern)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.start_edit_rule(index, window, cx);
                    })),
            )
            .child(
                div()
                    .text_size(px(TINY))
                    .text_color(rgb(TEXT_MUTED))
                    .invisible()
                    .group_hover(row_group.clone(), |s| s.visible())
                    .child(meta),
            )
            .child(
                quiet_mark(("rule-switch", index), is_enabled, false).on_click(cx.listener(
                    move |this, _, _, cx| {
                        if let Some(rule) = this.rules.get_mut(index) {
                            rule.enabled = !rule.enabled;
                        }
                        this.schedule_persist(cx);
                        if this.proxy_running && this.proxy_mode == ProxyMode::Rule {
                            this.restart_proxy_with_current_state(cx);
                        }
                        cx.notify();
                    },
                )),
            )
            .child(if del_armed {
                div()
                    .id(("rule-del-confirm", index))
                    .px_1()
                    .cursor_pointer()
                    .text_size(px(TINY))
                    .text_color(rgb(DANGER))
                    .child(sockrocket_gui::i18n::t("rules.clear_confirm").to_string())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        disarm_delete_confirm();
                        this.delete_rule(index, cx);
                    }))
                    .into_any_element()
            } else {
                div()
                    .id(("rule-del", index))
                    .w(px(18.0))
                    .h(px(18.0))
                    .rounded(px(4.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .invisible()
                    .group_hover(row_group, |s| s.visible())
                    .hover(|s| s.bg(rgba(with_alpha(DANGER, 0x18))))
                    .child(
                        Icon::empty()
                            .path("icons/trash-2.svg")
                            .with_size(gpui_component::Size::Size(px(11.0)))
                            .text_color(rgb(TEXT_MUTED)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        DELETE_CONFIRM_INDEX.store(index, Ordering::Relaxed);
                        DELETE_CONFIRM_AT.store(now_millis(), Ordering::Relaxed);
                        set_hovered_rule(Some(index));
                        schedule_confirm_reset(this, cx);
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .into_any_element()
    }
}

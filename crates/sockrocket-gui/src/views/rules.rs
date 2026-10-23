//! Rules view — macOS Settings-style groups and list rows.

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

fn short_target(target: &str) -> &str {
    if let Some(name) = target.strip_prefix("group:") {
        return name;
    }
    let key = TARGET_OPTIONS
        .iter()
        .find(|(value, _)| *value == target)
        .map(|(_, key)| *key)
        .unwrap_or("rules.act.proxy");
    sockrocket_gui::i18n::t(key)
}

/// Group enable control: full / mixed / off (Mac checkbox-ish mark).
fn quiet_mark(id: impl Into<ElementId>, on: bool, partial: bool) -> Stateful<Div> {
    let fill = if on {
        rgb(SUCCESS)
    } else if partial {
        rgba(with_alpha(SUCCESS, 0x66))
    } else {
        rgba(with_alpha(TEXT_MUTED, 0x55))
    };
    div()
        .id(id)
        .w(px(18.0))
        .h(px(18.0))
        .rounded(px(5.0))
        .flex_shrink_0()
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .bg(fill)
        .child(
            div()
                .text_size(px(TINY))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(0xffffff))
                .child(if on {
                    "✓"
                } else if partial {
                    "–"
                } else {
                    ""
                }),
        )
}

fn display_pattern(rule: &RoutingRule) -> String {
    if matches!(rule.rule_type.as_str(), "match" | "final") {
        sockrocket_gui::i18n::t("rules.match.label").to_string()
    } else {
        rule.pattern.clone()
    }
}

fn target_color(target: &str) -> u32 {
    match target {
        "direct" => SUCCESS,
        "reject" => DANGER,
        _ => ACCENT,
    }
}

fn text_action(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    color: u32,
) -> Stateful<Div> {
    div()
        .id(id)
        .px_2()
        .py(px(3.0))
        .rounded(px(6.0))
        .cursor_pointer()
        .text_size(px(SMALL))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(color))
        .hover(|s| s.bg(rgb(BG_HOVER)))
        .child(label.into())
}

fn scene_chip(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    filter_chip(id, label, false)
}

fn rules_status_kind(msg: &str) -> AlertKind {
    let t = msg.trim_start();
    if t.starts_with('✗') {
        AlertKind::Danger
    } else if t.starts_with('⚠') {
        AlertKind::Warning
    } else if t.starts_with('✓') {
        AlertKind::Success
    } else {
        AlertKind::Info
    }
}

impl AppState {
    pub(crate) fn render_rules(&mut self, cx: &mut Context<Self>) -> Div {
        let rule_count = self.rules.len();
        let rules_status = self.rules_status.clone();
        let clear_armed = confirm_slot_armed(&CLEAR_CONFIRM_AT);
        let batch_group = self.batch_edit_group.clone();

        let mut content = div().flex().flex_col().gap_3().child(
            div()
                .flex()
                .flex_row()
                .items_start()
                .justify_between()
                .gap_3()
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
                        .gap_1()
                        .pt_1()
                        .child(
                            text_action(
                                "rules-export",
                                sockrocket_gui::i18n::t("rules.export"),
                                TEXT_SECONDARY,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.export_rules_to_clipboard(cx);
                            })),
                        )
                        .child(
                            text_action(
                                "clear-rules",
                                if clear_armed {
                                    sockrocket_gui::i18n::t("rules.clear_confirm")
                                } else {
                                    sockrocket_gui::i18n::t("rules.clear")
                                },
                                if clear_armed { DANGER } else { TEXT_SECONDARY },
                            )
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
            card()
                .p(px(10.0))
                .gap_2()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div().flex_1().min_w(px(140.0)).child(
                                gpui_component::input::Input::new(&rule_filter_input)
                                    .xsmall()
                                    .w_full(),
                            ),
                        )
                        .when(!new_group_open, |d| {
                            d.child(
                                tint_action(
                                    "open-new-rule-group",
                                    sockrocket_gui::i18n::t("rules.group.new"),
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.rules_new_group_open = true;
                                        this.rule_group_input.update(cx, |state, cx| {
                                            state.set_value("", window, cx);
                                            state.focus(window, cx);
                                        });
                                        cx.notify();
                                    },
                                )),
                            )
                        }),
                )
                .when(new_group_open, |d| {
                    d.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div().flex_1().min_w(px(120.0)).child(
                                    gpui_component::input::Input::new(&self.rule_group_input)
                                        .xsmall()
                                        .w_full(),
                                ),
                            )
                            .child(
                                primary_button(
                                    "confirm-new-rule-group",
                                    sockrocket_gui::i18n::t("rules.group.new"),
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        let name = this
                                            .rule_group_input
                                            .read(cx)
                                            .value()
                                            .trim()
                                            .to_string();
                                        if name.is_empty() {
                                            this.rules_status =
                                                sockrocket_gui::i18n::t("rules.status.group_name")
                                                    .into();
                                            cx.notify();
                                        } else {
                                            this.begin_add_in_group(&name, window, cx);
                                        }
                                    },
                                )),
                            )
                            .child(
                                text_action(
                                    "cancel-new-rule-group",
                                    sockrocket_gui::i18n::t("rules.cancel"),
                                    TEXT_SECONDARY,
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.rules_new_group_open = false;
                                        this.rule_group_input.update(cx, |state, cx| {
                                            state.set_value("", window, cx);
                                        });
                                        cx.notify();
                                    },
                                )),
                            ),
                    )
                }),
        );
        content = content.children(if rules_status.is_empty() {
            None
        } else {
            Some(alert_strip(rules_status_kind(&rules_status), rules_status))
        });

        let adding_group = self.adding_rule_group.clone();
        let show_empty = rule_count == 0 && batch_group.is_none() && adding_group.is_none();
        if show_empty {
            content = content.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(empty_state(
                        sockrocket_gui::i18n::t("rules.empty"),
                        Some(
                            div()
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap_2()
                                .child(
                                    tint_action(
                                        "empty-new-group",
                                        sockrocket_gui::i18n::t("rules.empty.cta"),
                                    )
                                    .on_click(cx.listener(
                                        |this, _, window, cx| {
                                            this.rules_new_group_open = true;
                                            this.rule_group_input.update(cx, |state, cx| {
                                                state.set_value("", window, cx);
                                                state.focus(window, cx);
                                            });
                                            cx.notify();
                                        },
                                    )),
                                )
                                .child(
                                    div()
                                        .text_size(px(MICRO))
                                        .text_color(rgb(TEXT_MUTED))
                                        .child(sockrocket_gui::i18n::t("rules.empty.or_preset")),
                                )
                                .into_any_element(),
                        ),
                    ))
                    .child(self.render_scene_bar("scene-empty", cx)),
            );
        } else {
            let groups = sockrocket_core::group_rules_for_display(&self.rules);
            let mut list = div().flex().flex_col().gap_2p5();
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
                let gkey_add = gkey.clone();
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
                let adding_here = adding_group.as_deref() == Some(gkey.as_str());
                let expanded = !collapsed || !filter_q.is_empty() || batch_here || adding_here;
                let mut section = card()
                    .p_0()
                    .gap_0()
                    .group(header_group.clone())
                    .when(batch_here || adding_here, |d| {
                        d.border_1().border_color(rgba(with_alpha(ACCENT, 0x55)))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1p5()
                            .px_3()
                            .py_2()
                            .child(
                                div()
                                    .id(SharedString::from(format!("grp-toggle-{gkey}")))
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .flex_1()
                                    .min_w_0()
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.toggle_rule_group_collapsed(&gkey_toggle, cx);
                                    }))
                                    .child(
                                        div()
                                            .w(px(16.0))
                                            .text_size(px(TINY))
                                            .text_color(rgb(TEXT_MUTED))
                                            .child(if collapsed && !batch_here && !adding_here {
                                                "▸"
                                            } else {
                                                "▾"
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(BODY))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(TEXT_PRIMARY))
                                            .child(title),
                                    )
                                    .child(mini_badge(count_label, TEXT_MUTED)),
                            )
                            .child(
                                text_action(
                                    SharedString::from(format!("grp-add-{gkey}")),
                                    sockrocket_gui::i18n::t("rules.group.add"),
                                    if adding_here {
                                        TEXT_ACCENT
                                    } else {
                                        TEXT_SECONDARY
                                    },
                                )
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        if this.adding_rule_group.as_deref()
                                            == Some(gkey_add.as_str())
                                        {
                                            this.cancel_edit_rule(window, cx);
                                        } else {
                                            this.begin_add_in_group(&gkey_add, window, cx);
                                        }
                                    },
                                )),
                            )
                            .child(
                                text_action(
                                    SharedString::from(format!("grp-edit-{gkey}")),
                                    sockrocket_gui::i18n::t("rules.batch.mode"),
                                    if batch_here {
                                        TEXT_ACCENT
                                    } else {
                                        TEXT_SECONDARY
                                    },
                                )
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        if this.batch_edit_group.as_deref()
                                            == Some(gkey_edit.as_str())
                                        {
                                            this.cancel_edit_rule(window, cx);
                                        } else {
                                            this.begin_batch_edit_group(&gkey_edit, window, cx);
                                        }
                                    },
                                )),
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
                                text_action(
                                    SharedString::from(format!("grp-del-{gkey}")),
                                    if del_armed {
                                        sockrocket_gui::i18n::t("rules.clear_confirm")
                                    } else {
                                        "×"
                                    },
                                    if del_armed { DANGER } else { TEXT_SECONDARY },
                                )
                                .when(!del_armed, |d| {
                                    d.invisible().group_hover(header_group, |s| s.visible())
                                })
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        if group_delete_armed(&gkey_del) {
                                            GROUP_DEL_AT.store(0, Ordering::Relaxed);
                                            this.delete_rule_group(&gkey_del, cx);
                                        } else {
                                            arm_group_delete(&gkey_del);
                                            schedule_confirm_reset(this, cx);
                                            cx.notify();
                                        }
                                    },
                                )),
                            ),
                    );

                if batch_here {
                    section = section.child(
                        div()
                            .border_t_1()
                            .border_color(rgba(with_alpha(BORDER, 0x40)))
                            .child(self.render_batch_editor(cx)),
                    );
                } else if expanded {
                    let mut rows = div()
                        .flex()
                        .flex_col()
                        .border_t_1()
                        .border_color(rgba(with_alpha(BORDER, 0x40)));
                    let show_footer = adding_here || filter_q.is_empty();
                    let last = *shown.last().unwrap_or(&0);
                    for (pos, &i) in shown.iter().enumerate() {
                        let is_last_row = !show_footer && (i == last || pos + 1 == shown.len());
                        rows = rows.child(self.render_rule_item(i, is_last_row, cx));
                    }
                    if adding_here {
                        rows = rows.child(self.render_add_composer(cx));
                    } else if filter_q.is_empty() {
                        let gkey_footer = gkey.clone();
                        rows = rows.child(
                            div()
                                .id(SharedString::from(format!("grp-add-row-{gkey}")))
                                .px_3()
                                .py_2()
                                .cursor_pointer()
                                .text_size(px(SMALL))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(TEXT_ACCENT))
                                .hover(|s| s.bg(rgba(with_alpha(BG_HOVER, 0x55))))
                                .child(format!("+ {}", sockrocket_gui::i18n::t("rules.add.row")))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.begin_add_in_group(&gkey_footer, window, cx);
                                })),
                        );
                    }
                    section = section.child(rows);
                }
                list = list.child(section);
            }

            // Brand-new group (no rules yet): composer or batch card.
            for mode in ["add", "batch"] {
                let name = if mode == "add" {
                    adding_group.clone()
                } else {
                    batch_group.clone()
                };
                let Some(name) = name else { continue };
                let exists = sockrocket_core::group_rules_for_display(&self.rules)
                    .iter()
                    .any(|b| b.name == name);
                if exists {
                    continue;
                }
                any_shown = true;
                let title = if name.is_empty() {
                    sockrocket_gui::i18n::t("rules.group.ungrouped").to_string()
                } else {
                    name.clone()
                };
                let name_cancel = name.clone();
                list = list.child(
                    card()
                        .p_0()
                        .gap_0()
                        .border_1()
                        .border_color(rgba(with_alpha(ACCENT, 0x55)))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .px_3()
                                .py_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .text_size(px(BODY))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(TEXT_PRIMARY))
                                        .child(title),
                                )
                                .child(
                                    text_action(
                                        SharedString::from(format!(
                                            "orphan-cancel-{mode}-{name_cancel}"
                                        )),
                                        sockrocket_gui::i18n::t("rules.cancel"),
                                        TEXT_SECONDARY,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, window, cx| {
                                            this.cancel_edit_rule(window, cx);
                                        },
                                    )),
                                ),
                        )
                        .child(
                            div()
                                .border_t_1()
                                .border_color(rgba(with_alpha(BORDER, 0x40)))
                                .child(if mode == "add" {
                                    self.render_add_composer(cx)
                                } else {
                                    self.render_batch_editor(cx)
                                }),
                        ),
                );
            }

            if !any_shown && !filter_q.is_empty() {
                list = list.child(
                    card().child(
                        div()
                            .py_2()
                            .text_size(px(SMALL))
                            .text_color(rgb(TEXT_MUTED))
                            .child(sockrocket_gui::i18n::t("rules.filter.empty")),
                    ),
                );
            }
            content = content.child(list);
            content = content.child(self.render_scene_bar("scene", cx));
        }

        content
    }

    fn render_scene_bar(&mut self, id_prefix: &str, cx: &mut Context<Self>) -> Div {
        card()
            .p(px(12.0))
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .text_size(px(MICRO))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(TEXT_SECONDARY))
                            .child(sockrocket_gui::i18n::t("rules.scenes.title")),
                    )
                    .child(
                        div()
                            .text_size(px(MICRO))
                            .text_color(rgb(TEXT_MUTED))
                            .child(sockrocket_gui::i18n::t("rules.scenes.hint")),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap_1p5()
                    .children(builtin_rule_scenes().iter().map(|scene| {
                        let id = scene.id;
                        let eid = SharedString::from(format!("{id_prefix}-{id}"));
                        scene_chip(eid, sockrocket_gui::i18n::t(scene.title_key).to_string())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.apply_rule_scene(id, cx);
                            }))
                    })),
            )
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
            .min_w(px(120.0))
            .rounded(px(8.0))
            .bg(rgb(BG_PANEL))
            .border_1()
            .border_color(rgba(with_alpha(BORDER, 0x88)))
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
                    .px_3()
                    .py_1p5()
                    .cursor_pointer()
                    .text_size(px(SMALL))
                    .font_weight(if active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .text_color(rgb(if active { TEXT_ACCENT } else { TEXT_SECONDARY }))
                    .when(active, |d| d.bg(rgba(with_alpha(ACCENT, 0x14))))
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
        if menu == RuleMenu::Target {
            for group in &self.groups {
                let target = format!("group:{}", group.name);
                let label = format!("Group · {}", group.name);
                panel = panel.child(
                    div()
                        .id(SharedString::from(target.clone()))
                        .px_3()
                        .py_1p5()
                        .child(label)
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.rule_target_sel = target.clone();
                                this.rule_menu = None;
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        ),
                );
            }
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
                    .px_2()
                    .py(px(3.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .bg(rgba(with_alpha(BORDER, 0x55)))
                    .text_size(px(MICRO))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(TEXT_SECONDARY))
                    .hover(|s| s.bg(rgb(BG_HOVER)).text_color(rgb(TEXT_PRIMARY)))
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
            .px_3()
            .py_2p5()
            .flex()
            .flex_col()
            .gap_2()
            .bg(rgba(with_alpha(ACCENT, 0x0a)))
            .child(
                div()
                    .text_size(px(MICRO))
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
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(self.render_rule_menu(RuleMenu::Target, cx))
                    .child(div().flex_1())
                    .child(
                        text_action(
                            "batch-cancel",
                            sockrocket_gui::i18n::t("rules.cancel"),
                            TEXT_SECONDARY,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.cancel_edit_rule(window, cx);
                        })),
                    )
                    .child(
                        tint_action("batch-save", sockrocket_gui::i18n::t("rules.batch.save"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.rule_menu = None;
                                this.save_batch_edit_group(window, cx);
                            })),
                    ),
            )
    }

    fn render_rule_composer(&mut self, id_prefix: &str, cx: &mut Context<Self>) -> Div {
        let cur_type = self.rule_type_sel.clone();
        let pattern_input = self.rule_pattern_input.clone();
        let is_final = matches!(cur_type.as_str(), "match" | "final");
        let hint_key = match cur_type.as_str() {
            "geoip" => Some("rules.hint.geoip"),
            "domain" => Some("rules.hint.domain"),
            "domain-keyword" => Some("rules.hint.domain-keyword"),
            "ip-cidr" => Some("rules.hint.ip-cidr"),
            "dst-port" | "port" => Some("rules.hint.dst-port"),
            "match" | "final" => Some("rules.hint.match"),
            _ => Some("rules.hint.domain-suffix"),
        };
        div()
            .px_3()
            .py_2p5()
            .flex()
            .flex_col()
            .gap_1p5()
            .bg(rgba(with_alpha(ACCENT, 0x0c)))
            .when_some(hint_key, |d, key| {
                d.child(
                    div()
                        .text_size(px(MICRO))
                        .text_color(rgb(TEXT_MUTED))
                        .child(sockrocket_gui::i18n::t(key).to_string()),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(if is_final {
                        div()
                            .flex_1()
                            .text_size(px(SMALL))
                            .text_color(rgb(TEXT_MUTED))
                            .child(sockrocket_gui::i18n::t("rules.match.label"))
                            .into_any_element()
                    } else {
                        div()
                            .flex_1()
                            .min_w(px(100.0))
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
                        text_action(
                            SharedString::from(format!("{id_prefix}-cancel")),
                            sockrocket_gui::i18n::t("rules.cancel"),
                            TEXT_SECONDARY,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.cancel_edit_rule(window, cx);
                        })),
                    )
                    .child(
                        tint_action(
                            SharedString::from(format!("{id_prefix}-save")),
                            sockrocket_gui::i18n::t("rules.save"),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.add_rule(window, cx);
                        })),
                    ),
            )
    }

    fn render_add_composer(&mut self, cx: &mut Context<Self>) -> Div {
        self.render_rule_composer("add", cx)
    }

    fn render_inline_editor(&mut self, index: usize, cx: &mut Context<Self>) -> Div {
        self.render_rule_composer(&format!("inline-{index}"), cx)
    }

    pub(crate) fn render_rule_item(
        &mut self,
        index: usize,
        is_last: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.editing_rule_index == Some(index) {
            return self.render_inline_editor(index, cx).into_any_element();
        }
        let rule = &self.rules[index];
        let is_enabled = rule.enabled;
        let pattern = display_pattern(rule);
        let type_label = short_type(&rule.rule_type).to_string();
        let target_label = short_target(&rule.target).to_string();
        let tgt_color = target_color(&rule.target);
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
            .px_3()
            .py(px(8.0))
            .when(!is_last, |d| {
                d.border_b_1().border_color(rgba(with_alpha(BORDER, 0x28)))
            })
            .hover(|s| s.bg(rgba(with_alpha(BG_HOVER, 0x55))))
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
            .child(mini_badge(type_label, TEXT_MUTED))
            .child(mini_badge(target_label, tgt_color))
            .child(
                toggle_switch(("rule-switch", index), is_enabled).on_click(cx.listener(
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
                text_action(
                    ("rule-del-confirm", index),
                    sockrocket_gui::i18n::t("rules.clear_confirm"),
                    DANGER,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    disarm_delete_confirm();
                    this.delete_rule(index, cx);
                }))
                .into_any_element()
            } else {
                div()
                    .id(("rule-del", index))
                    .w(px(22.0))
                    .h(px(22.0))
                    .rounded(px(6.0))
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
                            .with_size(gpui_component::Size::Size(px(12.0)))
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

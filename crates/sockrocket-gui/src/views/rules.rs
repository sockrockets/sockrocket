// === Rules View (v2) ===
// Layout per ui-prototype-v2 §page-rules: icon page header + "Add Rule" action,
// "Custom Rules" glass card (type/pattern/target add row + live rule tester),
// bordered rule table (type/target badges, hover-revealed row actions,
// enable switches), bottom hint card. There is no ruleset-subscription data
// source in this app, so the v2 "Ruleset Subscriptions" panel is omitted and
// its two real actions (Load China Direct / Clear All) live on the Custom
// Rules card header instead. All behavior (add/delete/reorder/edit state
// machine, China Direct ruleset, rule tester, Rule-mode restart) unchanged.

use crate::app::*;
use crate::theme::*;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{Icon, Sizable as _};
use sockrocket_core::{ProxyMode, RoutingRule};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Two-click destructive confirm window, milliseconds (design-system §5.2).
const CONFIRM_WINDOW_MS: u64 = 3000;

/// Sentinel for "no rule row hovered / no delete confirm armed".
const NO_RULE: usize = usize::MAX;

/// Row index currently hovered; drives the hover-revealed row actions.
static HOVERED_RULE: AtomicUsize = AtomicUsize::new(NO_RULE);
/// Armed "Clear All" confirm timestamp (millis since epoch, 0 = disarmed).
static CLEAR_CONFIRM_AT: AtomicU64 = AtomicU64::new(0);
/// Armed single-rule delete confirm: rule index + timestamp.
static DELETE_CONFIRM_INDEX: AtomicUsize = AtomicUsize::new(NO_RULE);
static DELETE_CONFIRM_AT: AtomicU64 = AtomicU64::new(0);

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

fn disarm_delete_confirm() {
    DELETE_CONFIRM_INDEX.store(NO_RULE, Ordering::Relaxed);
    DELETE_CONFIRM_AT.store(0, Ordering::Relaxed);
}

/// Re-render once the confirm window lapses so armed buttons revert (§5.2 timeout reset).
fn schedule_confirm_reset(this: &AppState, cx: &mut Context<AppState>) {
    let handle = this.tokio_handle.clone();
    cx.spawn(async move |weak, cx| {
        // Must run inside the tokio runtime (via handle.spawn) because
        // gpui's own executor does not provide a tokio reactor.
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

/// Map a legacy ✓/✗/⚠-prefixed status message to an alert severity so all
/// operation feedback renders through `alert_strip` (design-system §5.3).
fn status_alert_kind(message: &str) -> AlertKind {
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

/// Client-side approximation of router matching for the rule tester.
/// Returns `(rule_index, target)` of the first *enabled* rule matching `host`.
/// `geoip` rules need a database and are not evaluated client-side.
fn match_rule(rules: &[RoutingRule], host: &str) -> Option<(usize, String)> {
    let host = host.trim().trim_end_matches('.').to_lowercase();
    if host.is_empty() {
        return None;
    }
    for (i, r) in rules.iter().enumerate() {
        if !r.enabled {
            continue;
        }
        let p = r.pattern.trim().to_lowercase();
        let hit = match r.rule_type.as_str() {
            "domain" | "domain-suffix" => host == p || host.ends_with(&format!(".{}", p)),
            "domain-keyword" => !p.is_empty() && host.contains(&p),
            "ip-cidr" => ip_in_cidr(&host, &p),
            "match" => true,
            _ => false,
        };
        if hit {
            return Some((i, r.target.clone()));
        }
    }
    None
}

/// IPv4 `addr` inside `cidr` ("a.b.c.d/bits")?
fn ip_in_cidr(addr: &str, cidr: &str) -> bool {
    let (net, bits) = cidr.split_once('/').unwrap_or((cidr, "32"));
    let (Ok(ip), Ok(net)) = (
        addr.parse::<std::net::Ipv4Addr>(),
        net.trim().parse::<std::net::Ipv4Addr>(),
    ) else {
        return false;
    };
    let bits: u32 = bits.trim().parse().unwrap_or(32).min(32);
    let mask = if bits == 0 {
        0
    } else {
        u32::MAX << (32 - bits)
    };
    (u32::from(ip) & mask) == (u32::from(net) & mask)
}

/// Color for a rule-target label (proxy/direct/reject).
fn target_label_color(target: &str) -> u32 {
    match target {
        "proxy" => ACCENT,
        "direct" => SUCCESS,
        "reject" => DANGER,
        _ => TEXT_SECONDARY,
    }
}

/// v2 select chip: px-2.5 py-1 rounded-md border; active = ACCENT tint,
/// inactive = BORDER border + muted text. `Stateful` for `.on_click`.
fn rule_chip(id: impl Into<ElementId>, label: &str, active: bool) -> Stateful<Div> {
    let chip = div()
        .id(id)
        .px_2p5()
        .py_1()
        .rounded(px(6.0))
        .border_1()
        .cursor_pointer()
        .text_size(px(SMALL))
        .font_weight(FontWeight::MEDIUM)
        .child(label.to_string());
    if active {
        chip.bg(rgba(with_alpha(ACCENT, 0x1a)))
            .border_color(rgba(with_alpha(ACCENT, 0x33)))
            .text_color(rgb(ACCENT))
    } else {
        chip.border_color(rgba(with_alpha(BORDER, 0x99)))
            .text_color(rgb(TEXT_MUTED))
            .hover(|s| s.bg(rgb(BG_HOVER)).text_color(rgb(TEXT_PRIMARY)))
    }
}

/// v2 cyan-tint action button (bg ACCENT/10 + border ACCENT/20, SMALL cyan).
fn tint_btn(id: impl Into<ElementId>, label: &str, icon_path: Option<&str>) -> Stateful<Div> {
    let mut btn = div()
        .id(id)
        .px_3()
        .py_1p5()
        .rounded(px(8.0))
        .flex()
        .flex_row()
        .items_center()
        .gap_1p5()
        .cursor_pointer()
        .bg(rgba(with_alpha(ACCENT, 0x1a)))
        .border_1()
        .border_color(rgba(with_alpha(ACCENT, 0x33)))
        .hover(|s| s.bg(rgba(with_alpha(ACCENT, 0x33))))
        .text_size(px(SMALL))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(ACCENT))
        .child(label.to_string());
    if let Some(path) = icon_path {
        btn = btn.child(
            Icon::empty()
                .path(SharedString::from(path.to_string()))
                .with_size(gpui_component::Size::Size(px(14.0)))
                .text_color(rgb(ACCENT)),
        );
    }
    btn
}

/// 20px hoverable row-action square; `hovered` tints the background.
fn action_btn(
    id: impl Into<ElementId>,
    child: impl IntoElement,
    hovered: bool,
    armed: bool,
) -> Stateful<Div> {
    let btn = div()
        .id(id)
        .w(px(20.0))
        .h(px(20.0))
        .rounded(px(4.0))
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .cursor_pointer();
    let btn = if armed {
        btn.bg(rgba(with_alpha(DANGER, 0x14)))
    } else if hovered {
        btn.bg(rgba(with_alpha(BG_HOVER, 0x4d)))
    } else {
        btn
    };
    btn.child(child)
}

/// 1px hairline divider between in-card sections.
fn hairline() -> Div {
    div()
        .h(px(1.0))
        .w_full()
        .flex_shrink_0()
        .bg(rgba(with_alpha(BORDER, 0x99)))
}

/// TINY muted field label above a form control (v2 "Type"/"Pattern"/"Target").
fn field_label(text: &str) -> Div {
    div()
        .text_size(px(TINY))
        .text_color(rgb(TEXT_MUTED))
        .child(text.to_string())
}

/// TINY muted mono table header cell.
fn head_cell(text: &str) -> Div {
    div()
        .text_size(px(TINY))
        .font_family(MONO_FONT)
        .text_color(rgb(TEXT_MUTED))
        .child(text.to_string())
}

impl AppState {
    pub(crate) fn render_rules(&mut self, cx: &mut Context<Self>) -> Div {
        let rule_count = self.rules.len();
        let rules_status = self.rules_status.clone();
        let rule_pattern_input = self.rule_pattern_input.clone();
        let cur_type = self.rule_type_sel.clone();
        let cur_target = self.rule_target_sel.clone();
        let editing = self.editing_rule_index.is_some();

        let pattern_hint: &'static str = match cur_type.as_str() {
            "domain" => "e.g. google.com",
            "domain-suffix" => "e.g. google.com (matches *.google.com too)",
            "domain-keyword" => "e.g. youtube",
            "ip-cidr" => "e.g. 192.168.0.0/24",
            "geoip" => "e.g. CN",
            "match" => "no pattern needed — matches all",
            _ => "",
        };

        let clear_armed = confirm_slot_armed(&CLEAR_CONFIRM_AT);

        // Hoist the tester match so the matched rule row can be highlighted.
        let tester_value_pre = self.rule_tester_input.read(cx).value().to_string();
        let tester_match: Option<usize> = if tester_value_pre.trim().is_empty() {
            None
        } else {
            match_rule(&self.rules, tester_value_pre.trim()).map(|(i, _)| i)
        };

        // (Both "Add Rule" entry points — header + empty state — focus the
        // pattern input via their own `cx.listener` below.)

        let mut content = div()
            .flex()
            .flex_col()
            .gap_4()
            // === Page header (v2): icon block + title/subtitle | Add Rule ===
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(self.page_header_v3(
                        "icons/nav-rules.svg",
                        ACCENT,
                        "Rules",
                        "Routing & filtering rules",
                    ))
                    .child(
                        tint_btn("rules-add-btn", "Add Rule", Some("icons/plus.svg")).on_click(
                            cx.listener(|this, _, window, cx| {
                                this.rule_pattern_input.update(cx, |state, cx| {
                                    state.focus(window, cx);
                                });
                            }),
                        ),
                    ),
            )
            // === Custom Rules card (v2): header actions + add row + tester ===
            .child(
                card()
                    // Card header: section label | Load China Direct · Clear All
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(section_label("Custom Rules"))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_3()
                                    .child(
                                        div()
                                            .id("load-china-rules")
                                            .px_1()
                                            .py_0p5()
                                            .rounded(px(4.0))
                                            .cursor_pointer()
                                            .text_size(px(MICRO))
                                            .text_color(rgb(ACCENT))
                                            .hover(|s| {
                                                s.bg(rgba(with_alpha(ACCENT, 0x14)))
                                                    .text_color(rgb(TEXT_ACCENT))
                                            })
                                            .child("Load China Direct")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.load_china_rules(cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("clear-rules")
                                            .xsmall()
                                            .label(if clear_armed {
                                                "Confirm?".to_string()
                                            } else {
                                                "Clear All".to_string()
                                            })
                                            .tooltip("Remove all routing rules")
                                            .when(clear_armed, |b| b.danger())
                                            .when(!clear_armed, |b| b.ghost())
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                if confirm_slot_armed(&CLEAR_CONFIRM_AT) {
                                                    CLEAR_CONFIRM_AT.store(0, Ordering::Relaxed);
                                                    this.clear_rules(cx);
                                                } else {
                                                    CLEAR_CONFIRM_AT
                                                        .store(now_millis(), Ordering::Relaxed);
                                                    schedule_confirm_reset(this, cx);
                                                    cx.notify();
                                                }
                                            })),
                                    ),
                            ),
                    )
                    // Add row (v2): labeled Type chips + Pattern input + Target chips + Add
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .items_end()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(field_label("Type"))
                                    .child({
                                        let t = cur_type.clone();
                                        let types: &[(&str, &str)] = &[
                                            ("domain", "Domain"),
                                            ("domain-suffix", "Suffix"),
                                            ("domain-keyword", "Keyword"),
                                            ("ip-cidr", "IP CIDR"),
                                            ("geoip", "GeoIP"),
                                            ("match", "Match All"),
                                        ];
                                        let mut row = div().flex().flex_row().gap_1().flex_wrap();
                                        for &(val, label) in types {
                                            let active = t == val;
                                            row = row.child(
                                                rule_chip(
                                                    SharedString::from(format!(
                                                        "rule-type-{}",
                                                        val
                                                    )),
                                                    label,
                                                    active,
                                                )
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.rule_type_sel = val.to_string();
                                                    cx.notify();
                                                })),
                                            );
                                        }
                                        row
                                    }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(200.0))
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(field_label("Pattern"))
                                    .child(
                                        gpui_component::input::Input::new(&rule_pattern_input)
                                            .xsmall()
                                            .w_full(),
                                    )
                                    .when(!pattern_hint.is_empty(), |d| {
                                        d.child(
                                            div()
                                                .text_size(px(TINY))
                                                .font_family(MONO_FONT)
                                                .text_color(rgb(TEXT_MUTED))
                                                .child(pattern_hint),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(field_label("Target"))
                                    .child({
                                        let mut row = div().flex().flex_row().gap_1().flex_wrap();
                                        for &(val, label) in &[
                                            ("direct", "Direct"),
                                            ("proxy", "Proxy"),
                                            ("reject", "Block"),
                                        ] {
                                            let active = cur_target == val;
                                            row = row.child(
                                                rule_chip(
                                                    SharedString::from(format!(
                                                        "rule-target-{}",
                                                        val
                                                    )),
                                                    label,
                                                    active,
                                                )
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.rule_target_sel = val.to_string();
                                                    cx.notify();
                                                })),
                                            );
                                        }
                                        row
                                    }),
                            )
                            .child(
                                tint_btn(
                                    "add-rule-btn",
                                    if editing { "Save" } else { "Add" },
                                    if editing {
                                        None
                                    } else {
                                        Some("icons/plus.svg")
                                    },
                                )
                                .px_4()
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.add_rule(cx);
                                    },
                                )),
                            )
                            .children(if editing {
                                Some(
                                    Button::new("cancel-edit-btn")
                                        .xsmall()
                                        .label("Cancel".to_string())
                                        .tooltip("Discard changes and stop editing")
                                        .ghost()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.cancel_edit_rule(cx);
                                        })),
                                )
                            } else {
                                None
                            }),
                    )
                    .children(if !rules_status.is_empty() {
                        Some(alert_strip(status_alert_kind(&rules_status), rules_status))
                    } else {
                        None
                    })
                    // === Rule tester (v2): one labeled input row, live first match ===
                    .child(hairline())
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(div().w(px(28.0)).flex_shrink_0().child(field_label("Test")))
                            .child(
                                div().flex_1().min_w_0().child(
                                    gpui_component::input::Input::new(&self.rule_tester_input)
                                        .xsmall()
                                        .w_full(),
                                ),
                            )
                            .when(!tester_value_pre.trim().is_empty(), |d| {
                                match match_rule(&self.rules, tester_value_pre.trim()) {
                                    Some((i, target)) => d
                                        .child(
                                            div()
                                                .flex_shrink_0()
                                                .text_size(px(SMALL))
                                                .font_family(MONO_FONT)
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(ACCENT))
                                                .child(format!("#{}", i + 1)),
                                        )
                                        .child(mini_badge(
                                            target.clone(),
                                            target_label_color(&target),
                                        )),
                                    None => d
                                        .child(
                                            div()
                                                .flex_shrink_0()
                                                .text_size(px(SMALL))
                                                .font_family(MONO_FONT)
                                                .text_color(rgb(TEXT_MUTED))
                                                .child("no match →"),
                                        )
                                        .child(mini_badge("FINAL", WARNING)),
                                }
                            }),
                    )
                    .child(div().text_size(px(TINY)).text_color(rgb(TEXT_MUTED)).child(
                        "First enabled match wins · geoip rules are not evaluated client-side",
                    )),
            );

        // === Rule table (v2): bordered container, tinted header, hover rows ===
        if rule_count == 0 {
            let add_action = Button::new("empty-add-rule")
                .xsmall()
                .label("Add Rule".to_string())
                .tooltip("Add your first routing rule")
                .primary()
                .on_click(cx.listener(|this, _, window, cx| {
                    this.rule_pattern_input.update(cx, |state, cx| {
                        state.focus(window, cx);
                    });
                }));
            content = content.child(empty_state(
                "No rules yet",
                Some(add_action.into_any_element()),
            ));
        } else {
            let mut list = div().flex().flex_col();
            // Header: # 32px + toggle spacer 32px | TYPE | PATTERN | TARGET | ACTIONS
            let head = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                .bg(rgba(with_alpha(BG_HOVER, 0x4d)))
                .border_b_1()
                .border_color(rgba(with_alpha(BORDER, 0x99)))
                .child(div().w(px(32.0)).flex_shrink_0().child(head_cell("#")))
                .child(div().w(px(32.0)).flex_shrink_0())
                .child(div().w(px(112.0)).flex_shrink_0().child(head_cell("TYPE")))
                .child(div().flex_1().min_w_0().child(head_cell("PATTERN")))
                .child(div().w(px(72.0)).flex_shrink_0().child(head_cell("TARGET")))
                .child(
                    div().flex_1().min_w_0().flex().justify_end().child(
                        div()
                            .w(px(48.0))
                            .flex_shrink_0()
                            .flex()
                            .justify_center()
                            .child(head_cell("ACTIONS")),
                    ),
                );
            list = list.child(head);
            for i in 0..rule_count {
                list = list.child(self.render_rule_item(
                    i,
                    tester_match == Some(i),
                    i == rule_count - 1,
                    cx,
                ));
            }
            content = content.child(
                div()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(rgba(with_alpha(BORDER, 0x99)))
                    .overflow_hidden()
                    .child(list),
            );
        }

        // === Info card (v2): cyan info icon + hint with highlighted rule types ===
        content = content.child(
            card()
                .p_4()
                .flex()
                .flex_row()
                .items_start()
                .gap_3()
                .child(
                    div().mt(px(1.0)).child(
                        Icon::empty()
                            .path("icons/info.svg")
                            .with_size(gpui_component::Size::Size(px(16.0)))
                            .text_color(rgb(ACCENT)),
                    ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .gap_1()
                        .text_size(px(SMALL))
                        .text_color(rgb(TEXT_SECONDARY))
                        .child("Rules are evaluated top-down. First match wins. Use ")
                        .child(
                            div()
                                .font_family(MONO_FONT)
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(ACCENT))
                                .child("DOMAIN-SUFFIX"),
                        )
                        .child(" for whole domains, ")
                        .child(
                            div()
                                .font_family(MONO_FONT)
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(ACCENT))
                                .child("IP-CIDR"),
                        )
                        .child(" for network ranges."),
                ),
        );

        content
    }

    pub(crate) fn render_rule_item(
        &mut self,
        index: usize,
        tester_hit: bool,
        is_last: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let rule = &self.rules[index];
        let is_enabled = rule.enabled;
        // Type column: purple mini_badge, uppercase (v2).
        let type_label: String = match rule.rule_type.as_str() {
            "domain" => "DOMAIN".to_string(),
            "domain-suffix" => "DOMAIN-SUFFIX".to_string(),
            "domain-keyword" => "DOMAIN-KEYWORD".to_string(),
            "ip-cidr" => "IP-CIDR".to_string(),
            "geoip" => "GEOIP".to_string(),
            "match" => "MATCH".to_string(),
            other => other.to_uppercase(),
        };
        let type_chip = mini_badge(type_label, PURPLE);
        let target_color = if !is_enabled {
            TEXT_MUTED
        } else {
            match rule.target.as_str() {
                "proxy" => ACCENT,
                "direct" => SUCCESS,
                "reject" => DANGER,
                _ => TEXT_SECONDARY,
            }
        };
        let pattern = rule.pattern.clone();
        let target = rule.target.clone();
        let pattern_color = if is_enabled { TEXT_PRIMARY } else { TEXT_MUTED };

        let hovered = hovered_rule() == Some(index);
        let del_armed = delete_confirm_armed(index);

        let weak = cx.entity().downgrade();

        div()
            .id(SharedString::from(format!("rule-{}", index)))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .when(!is_last, |d| {
                d.border_b_1().border_color(rgba(with_alpha(BORDER, 0x66)))
            })
            .when(tester_hit, |d| d.bg(rgba(with_alpha(ACCENT, 0x0d))))
            .when(!is_enabled, |d| d.opacity(0.5))
            .hover(|s| s.bg(rgba(with_alpha(BG_HOVER, 0x4d))))
            .on_hover(move |is_hovered, _window, cx| {
                let next = if *is_hovered { Some(index) } else { None };
                if hovered_rule() != next {
                    set_hovered_rule(next);
                    weak.update(cx, |_, cx| cx.notify()).ok();
                }
            })
            // # (1-based rule ordinal)
            .child(
                div()
                    .w(px(32.0))
                    .flex_shrink_0()
                    .text_size(px(TINY))
                    .font_family(MONO_FONT)
                    .text_color(rgb(TEXT_MUTED))
                    .child(format!("{}", index + 1)),
            )
            // ENABLE switch (always visible)
            .child(div().w(px(32.0)).flex_shrink_0().child(
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
            ))
            // TYPE (purple badge)
            .child(div().w(px(112.0)).flex_shrink_0().child(type_chip))
            // PATTERN (mono, primary)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(SMALL))
                    .font_family(MONO_FONT)
                    .text_color(rgb(pattern_color))
                    .overflow_x_hidden()
                    .child(pattern),
            )
            // TARGET (colored badge)
            .child(
                div()
                    .w(px(72.0))
                    .flex_shrink_0()
                    .child(mini_badge(target.to_uppercase(), target_color)),
            )
            // ACTIONS (up/down/edit icons + destructive delete, two-step confirm)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .gap_0p5()
                    .child(
                        action_btn(
                            ("rule-up", index),
                            div()
                                .text_size(px(12.0))
                                .text_color(rgb(if hovered { TEXT_PRIMARY } else { TEXT_MUTED }))
                                .child("↑"),
                            hovered,
                            false,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.move_rule_up(index, cx);
                        }))
                        .tooltip(|window, cx| {
                            gpui_component::tooltip::Tooltip::new("Move rule up (higher priority)")
                                .build(window, cx)
                        }),
                    )
                    .child(
                        action_btn(
                            ("rule-down", index),
                            Icon::empty()
                                .path("icons/chevron-down.svg")
                                .with_size(gpui_component::Size::Size(px(12.0)))
                                .text_color(rgb(if hovered { TEXT_PRIMARY } else { TEXT_MUTED })),
                            hovered,
                            false,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.move_rule_down(index, cx);
                        }))
                        .tooltip(|window, cx| {
                            gpui_component::tooltip::Tooltip::new("Move rule down (lower priority)")
                                .build(window, cx)
                        }),
                    )
                    .child(
                        action_btn(
                            ("rule-edit", index),
                            Icon::empty()
                                .path("icons/edit.svg")
                                .with_size(gpui_component::Size::Size(px(12.0)))
                                .text_color(rgb(if hovered { TEXT_PRIMARY } else { TEXT_MUTED })),
                            hovered,
                            false,
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.start_edit_rule(index, window, cx);
                        }))
                        .tooltip(|window, cx| {
                            gpui_component::tooltip::Tooltip::new("Edit this rule")
                                .build(window, cx)
                        }),
                    )
                    .child(
                        action_btn(
                            ("rule-del", index),
                            Icon::empty()
                                .path("icons/trash-2.svg")
                                .with_size(gpui_component::Size::Size(px(12.0)))
                                .text_color(rgb(if del_armed {
                                    DANGER
                                } else if hovered {
                                    TEXT_PRIMARY
                                } else {
                                    TEXT_MUTED
                                })),
                            hovered,
                            del_armed,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if delete_confirm_armed(index) {
                                disarm_delete_confirm();
                                this.delete_rule(index, cx);
                            } else {
                                DELETE_CONFIRM_INDEX.store(index, Ordering::Relaxed);
                                DELETE_CONFIRM_AT.store(now_millis(), Ordering::Relaxed);
                                set_hovered_rule(Some(index));
                                schedule_confirm_reset(this, cx);
                                cx.notify();
                            }
                        }))
                        .tooltip(move |window, cx| {
                            gpui_component::tooltip::Tooltip::new(if delete_confirm_armed(index) {
                                "Click again to confirm deletion"
                            } else {
                                "Delete this rule"
                            })
                            .build(window, cx)
                        }),
                    ),
            )
    }
}

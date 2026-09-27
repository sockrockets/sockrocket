// === Groups View (proxy groups, user-defined + persisted) ===
// Groups are user-defined and persisted in gui-state.yaml (AppConfig.groups,
// snapshot via AppState::snapshot_config). Members are node *fingerprints*,
// so membership survives renames and subscription refreshes; fingerprints
// that no longer resolve to a node are silently omitted from the chips row
// (never an error) and stay in config so a later refresh can resurrect them.
// Clicking a member chip makes it the active outbound; "Test Now" re-probes
// the group's members and picks via the group's semantics (url-test = lowest
// latency, fallback = first reachable in order) — see AppState::group_test_now
// / finish_group_test in app.rs.

use crate::app::*;
use crate::theme::*;
use crate::views::nodes::confirm_armed;
use gpui::*;
use gpui_component::Sizable as _;
use gpui_component::button::{Button, ButtonVariants as _};
use sockrocket_core::{GroupType, node_fingerprint};
use std::sync::atomic::{AtomicU64, Ordering};

/// Armed per-group delete confirm timestamp (millis since epoch, 0 =
/// disarmed); which group is armed lives in AppState::group_delete_armed.
static GROUP_DELETE_ARMED_AT: AtomicU64 = AtomicU64::new(0);

fn group_type_label(t: GroupType) -> &'static str {
    match t {
        GroupType::Select => "select",
        GroupType::UrlTest => "url-test",
        GroupType::Fallback => "fallback",
    }
}

fn group_type_color(t: GroupType) -> u32 {
    match t {
        GroupType::Select => SUCCESS,
        GroupType::UrlTest => ACCENT,
        GroupType::Fallback => WARNING,
    }
}

/// v2 cyan-tint button (same recipe as the Nodes page tint_button): used for
/// "Add Group" and the empty-state template action.
fn tint_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .px_3()
        .py(px(6.0))
        .rounded(px(8.0))
        .cursor_pointer()
        .border_1()
        .bg(rgba(with_alpha(ACCENT, 0x1a)))
        .border_color(rgba(with_alpha(ACCENT, 0x33)))
        .text_size(px(SMALL))
        .text_color(rgb(ACCENT))
        .hover(|s| s.bg(rgba(with_alpha(ACCENT, 0x33))))
        .child(label.into())
}

/// Small bordered action chip in group card headers (Test Now / + Nodes / ✎ / ✕).
fn action_chip(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    color: u32,
) -> Stateful<Div> {
    div()
        .id(id)
        .px_2()
        .py_0p5()
        .rounded(px(4.0))
        .border_1()
        .border_color(rgba(with_alpha(color, 0x40)))
        .cursor_pointer()
        .text_size(px(MICRO))
        .font_family(MONO_FONT)
        .text_color(rgb(color))
        .hover(|s| s.bg(rgba(with_alpha(color, 0x14))))
        .child(label.into())
}

/// Type selector chip in the creation row (same recipe as the Rules page chips).
fn type_chip(id: impl Into<ElementId>, label: &str, active: bool) -> Stateful<Div> {
    let chip = div()
        .id(id)
        .px_2p5()
        .py_1()
        .rounded(px(6.0))
        .border_1()
        .cursor_pointer()
        .text_size(px(SMALL))
        .font_family(MONO_FONT)
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

impl AppState {
    pub(crate) fn render_groups(&mut self, cx: &mut Context<Self>) -> Div {
        let mut page = div().flex().flex_col().gap_2().child(self.page_header_v3(
            "icons/nav-groups.svg",
            PURPLE,
            "Proxy Groups",
            "select · url-test · fallback",
        ));

        // --- Creation row: name input + type chips + Add button ---
        let mut type_chips = div().flex().flex_row().gap_1();
        for (t, label) in [
            (GroupType::Select, "select"),
            (GroupType::UrlTest, "url-test"),
            (GroupType::Fallback, "fallback"),
        ] {
            let active = self.group_type_sel == t;
            type_chips = type_chips.child(
                type_chip(
                    SharedString::from(format!("group-type-{}", label)),
                    label,
                    active,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.group_type_sel = t;
                    cx.notify();
                })),
            );
        }
        page = page.child(
            card().child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        div().w(px(200.0)).child(
                            gpui_component::input::Input::new(&self.group_name_input)
                                .xsmall()
                                .w_full(),
                        ),
                    )
                    .child(type_chips)
                    .child(div().flex_1())
                    .child(
                        tint_button("group-add-btn", "Add Group").on_click(cx.listener(
                            |this, _, window, cx| {
                                this.group_create(window, cx);
                            },
                        )),
                    ),
            ),
        );

        // --- Empty state: brief explanation + one-click template ---
        if self.groups.is_empty() {
            if self.nodes.is_empty() {
                return page.child(empty_state(
                    "No nodes yet — import a subscription or add a node first.",
                    None,
                ));
            }
            return page.child(empty_state(
                "No proxy groups yet. Groups reference members by node fingerprint, so renaming nodes or refreshing subscriptions never loses membership.",
                Some(
                    tint_button("group-template-btn", "Create auto-select group (all nodes)")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.group_template_auto_select(cx);
                        }))
                        .into_any_element(),
                ),
            ));
        }

        for gi in 0..self.groups.len() {
            let group = self.groups[gi].clone();
            let current_fp = group.current.clone();
            let delete_armed =
                self.group_delete_armed == Some(gi) && confirm_armed(&GROUP_DELETE_ARMED_AT);
            let add_open = self.group_add_open == Some(gi);
            let editing = self.editing_group_index == Some(gi);
            let testing = self.group_test_pending == Some(gi);
            let current_name = current_fp.as_deref().and_then(|fp| {
                self.nodes
                    .iter()
                    .find(|n| node_fingerprint(n) == fp)
                    .map(|n| n.name.clone())
            });

            // --- Card header: name + type badge + current pick + actions ---
            let mut header = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_size(px(BODY))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(TEXT_PRIMARY))
                        .child(group.name.clone()),
                )
                .child(mini_badge(
                    group_type_label(group.gtype),
                    group_type_color(group.gtype),
                ));

            if let Some(name) = current_name {
                header = header.child(
                    div()
                        .text_size(px(MICRO))
                        .text_color(rgb(TEXT_MUTED))
                        .child(format!("Current · {}", name)),
                );
            }

            header = header.child(div().flex_1());

            // "+ Nodes" toggles the inline add-list of not-yet-member nodes.
            header = header.child(
                action_chip(
                    ("group-add-toggle", gi),
                    if add_open { "− Nodes" } else { "+ Nodes" },
                    TEXT_SECONDARY,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.group_add_open = if this.group_add_open == Some(gi) {
                        None
                    } else {
                        Some(gi)
                    };
                    cx.notify();
                })),
            );

            // Test Now: url-test / fallback groups re-probe members and re-pick.
            if matches!(group.gtype, GroupType::UrlTest | GroupType::Fallback) {
                if testing {
                    header = header.child(
                        div()
                            .px_2()
                            .py_0p5()
                            .text_size(px(MICRO))
                            .font_family(MONO_FONT)
                            .text_color(rgb(TEXT_MUTED))
                            .child("Testing…"),
                    );
                } else {
                    header = header.child(
                        action_chip(("group-test", gi), "Test Now", ACCENT).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.group_test_now(gi, cx);
                            },
                        )),
                    );
                }
            }

            // Inline rename: ✎ arms the edit row below the header.
            let current_name_for_rename = group.name.clone();
            header = header.child(
                action_chip(("group-rename", gi), "✎", TEXT_SECONDARY).on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.editing_group_index = Some(gi);
                        let name = current_name_for_rename.clone();
                        this.group_rename_input
                            .update(cx, |s, cx| s.set_value(name, window, cx));
                        cx.notify();
                    },
                )),
            );

            // Delete: two-step confirm (design-system §5.2, same pattern as
            // the Nodes page destructive actions).
            header = header.child(
                action_chip(
                    ("group-delete", gi),
                    if delete_armed {
                        "Confirm delete?"
                    } else {
                        "✕"
                    },
                    DANGER,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.group_delete_armed == Some(gi) && confirm_armed(&GROUP_DELETE_ARMED_AT)
                    {
                        GROUP_DELETE_ARMED_AT.store(0, Ordering::Relaxed);
                        this.group_delete(gi, cx);
                    } else {
                        this.group_delete_armed = Some(gi);
                        this.arm_destructive_confirm(&GROUP_DELETE_ARMED_AT, cx);
                    }
                })),
            );

            let desc = match group.gtype {
                GroupType::UrlTest => {
                    "Test Now measures manually, then auto-locks the lowest-latency member"
                }
                GroupType::Select => "Pick the outbound node manually",
                GroupType::Fallback => "Tries members in order, stops at the first usable one",
            };

            // --- Member chips (wrap): name + latency, current highlighted (✓),
            //     per-chip × removes the member. Members whose fingerprint no
            //     longer resolves are silently omitted. ---
            let mut members = div().flex().flex_row().flex_wrap().gap_1p5();
            let mut resolved_count = 0usize;
            for fp in &group.members {
                let Some(idx) = self.nodes.iter().position(|n| node_fingerprint(n) == *fp) else {
                    continue; // stale fingerprint: keep in config, skip in UI
                };
                resolved_count += 1;
                let node = &self.nodes[idx];
                let latency = if self.latency_testing.contains(&idx) {
                    None // probe in flight: show "…" via the testing marker below
                } else {
                    node.latency_ms
                };
                let is_current = current_fp.as_deref() == Some(fp.as_str());
                let fp_use = fp.clone();
                let fp_remove = fp.clone();

                let mut chip = div()
                    .id(SharedString::from(format!("group-{}-member-{}", gi, fp)))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1p5()
                    .px_3()
                    .py_2()
                    .rounded(px(8.0))
                    .border_1()
                    .cursor_pointer()
                    .text_size(px(SMALL));

                chip = if is_current {
                    chip.border_color(rgba(with_alpha(ACCENT, 0x66)))
                        .bg(rgba(with_alpha(ACCENT, 0x14)))
                        .text_color(rgb(TEXT_ACCENT))
                } else {
                    chip.border_color(rgba(with_alpha(BORDER, 0x99)))
                        .text_color(rgb(TEXT_SECONDARY))
                        .hover(|s| s.bg(rgb(BG_HOVER)).text_color(rgb(TEXT_PRIMARY)))
                };

                chip = chip.child(node.name.clone());
                if self.latency_testing.contains(&idx) {
                    chip = chip.child(
                        div()
                            .text_size(px(TINY))
                            .font_family(MONO_FONT)
                            .text_color(rgb(TEXT_MUTED))
                            .child("…"),
                    );
                } else {
                    chip = match latency {
                        Some(ms) => chip.child(
                            div()
                                .text_size(px(TINY))
                                .font_family(MONO_FONT)
                                .text_color(rgb(latency_color(ms as u64)))
                                .child(format!("{}ms", ms)),
                        ),
                        None => {
                            // Failed probe → red unreachable marker instead of
                            // the neutral "—" (which means "not tested yet").
                            if self.latency_failed.contains(&idx) {
                                chip.child(
                                    div()
                                        .text_size(px(TINY))
                                        .font_family(MONO_FONT)
                                        .text_color(rgb(DANGER))
                                        .child("✗"),
                                )
                            } else {
                                chip.child(
                                    div()
                                        .text_size(px(TINY))
                                        .font_family(MONO_FONT)
                                        .text_color(rgb(TEXT_MUTED))
                                        .child("—"),
                                )
                            }
                        }
                    };
                }
                if is_current {
                    chip = chip.child(
                        div()
                            .text_size(px(TINY))
                            .font_family(MONO_FONT)
                            .text_color(rgb(TEXT_ACCENT))
                            .child("✓"),
                    );
                }

                // Per-chip × removes the member; stop the mouse-down from
                // reaching the chip so removal doesn't also activate the node.
                chip = chip.child(
                    div()
                        .id(SharedString::from(format!("group-{}-rm-{}", gi, fp)))
                        .px_1()
                        .rounded(px(3.0))
                        .cursor_pointer()
                        .text_size(px(TINY))
                        .font_family(MONO_FONT)
                        .text_color(rgb(TEXT_MUTED))
                        .hover(|s| s.text_color(rgb(DANGER)))
                        .child("×")
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.group_remove_member(gi, &fp_remove, cx);
                        })),
                );

                chip = chip.on_click(cx.listener(move |this, _, _, cx| {
                    this.group_use_member(gi, &fp_use, cx);
                }));
                members = members.child(chip);
            }

            let mut card_el = card().child(header).child(
                div()
                    .text_size(px(MICRO))
                    .text_color(rgb(TEXT_MUTED))
                    .child(desc),
            );

            if resolved_count == 0 {
                card_el = card_el.child(
                    div()
                        .text_size(px(MICRO))
                        .text_color(rgb(TEXT_MUTED))
                        .child("No members yet — click + Nodes to add nodes."),
                );
            } else {
                card_el = card_el.child(members);
            }

            // --- Inline rename row (armed by the ✎ header chip) ---
            if editing {
                card_el = card_el.child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .items_center()
                        .child(
                            div().flex_1().child(
                                gpui_component::input::Input::new(&self.group_rename_input)
                                    .xsmall()
                                    .w_full(),
                            ),
                        )
                        .child(
                            Button::new(("group-rename-save", gi))
                                .xsmall()
                                .label("Save".to_string())
                                .tooltip("Confirm rename")
                                .primary()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.group_rename_confirm(gi, cx);
                                })),
                        ),
                );
            }

            // --- "Add Nodes" expander: not-yet-member nodes as add-chips ---
            if add_open {
                let mut add_row = div().flex().flex_row().flex_wrap().gap_1p5();
                let mut any = false;
                for (idx, node) in self.nodes.iter().enumerate() {
                    let fp = node_fingerprint(node);
                    if group.members.contains(&fp) {
                        continue;
                    }
                    any = true;
                    add_row = add_row.child(
                        div()
                            .id(SharedString::from(format!("group-{}-add-{}", gi, idx)))
                            .px_2()
                            .py_1()
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(rgba(with_alpha(BORDER, 0x99)))
                            .cursor_pointer()
                            .text_size(px(MICRO))
                            .text_color(rgb(TEXT_MUTED))
                            .hover(|s| {
                                s.bg(rgba(with_alpha(ACCENT, 0x14)))
                                    .border_color(rgba(with_alpha(ACCENT, 0x40)))
                                    .text_color(rgb(TEXT_ACCENT))
                            })
                            .child(format!("+ {}", node.name))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.group_add_member(gi, idx, cx);
                            })),
                    );
                }
                card_el = card_el.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(TINY))
                                .text_color(rgb(TEXT_MUTED))
                                .child("Click to add to group:"),
                        )
                        .child(if any {
                            add_row
                        } else {
                            div().child(
                                div()
                                    .text_size(px(MICRO))
                                    .text_color(rgb(TEXT_MUTED))
                                    .child("No nodes available to add (all nodes are already in this group)."),
                            )
                        }),
                );
            }

            page = page.child(card_el);
        }

        page.child(
            div().text_size(px(TINY)).text_color(rgb(TEXT_MUTED)).child(
                "Groups persist to gui-state.yaml; members are matched by node fingerprint.",
            ),
        )
    }
}

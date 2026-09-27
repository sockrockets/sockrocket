// === Nodes View ===
// Split impl block of `AppState` (moved from app.rs; behavior unchanged).

use crate::app::*;
use crate::components::qr::render_qr;
use crate::theme::*;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::Sizable as _;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{Icon, Size as ComponentSize};
use sockrocket_core::SubscriptionFormat;
use sockrocket_core::config::model::{Node, ProxyProtocol};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

// === Row hover actions (v3) ===
// Row action buttons are revealed with GPUI group_hover styles (pure style
// refinement on hover — no per-hover cx.notify repaint of the whole tree).

/// v3 subscription bar collapse state (starts collapsed like the prototype).
static SUBS_BAR_OPEN: AtomicBool = AtomicBool::new(false);

/// v3 filter chip: bordered micro pill; active = cyan tint, inactive = muted.
fn v3_chip(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
) -> Stateful<Div> {
    let base = div()
        .id(id)
        .px_2p5()
        .py(px(4.0))
        .rounded(px(6.0))
        .border_1()
        .text_size(px(MICRO))
        .cursor_pointer();
    if active {
        base.bg(rgba(with_alpha(ACCENT, 0x1a)))
            .border_color(rgba(with_alpha(ACCENT, 0x33)))
            .text_color(rgb(ACCENT))
            .child(label.into())
    } else {
        base.border_color(rgba(with_alpha(BORDER, 0x99)))
            .text_color(rgb(TEXT_MUTED))
            .hover(|s| {
                s.text_color(rgb(TEXT_SECONDARY))
                    .border_color(rgba(with_alpha(TEXT_MUTED, 0x4d)))
            })
            .child(label.into())
    }
}

// === Destructive-action two-step confirm (design-system §5.2) ===
// Pure UI state: epoch millis when the action was armed, 0 = disarmed.
// Module-level statics keep this out of the business state in `AppState`.

static DELETE_ALL_ARMED_AT: AtomicU64 = AtomicU64::new(0);
static DELETE_SELECTED_ARMED_AT: AtomicU64 = AtomicU64::new(0);

/// How long a destructive action stays armed before it auto-reverts.
const CONFIRM_WINDOW_MS: u64 = 3_000;

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Node list filter/sort cache: (filter text, protocol filter, tag filter,
/// nodes generation, filtered+sorted indices).
type NodeFilterCache = (
    String,
    Option<&'static str>,
    Option<String>,
    u64,
    std::rc::Rc<Vec<usize>>,
);

thread_local! {
    /// Filtered + latency-sorted node index list, keyed on (filter text,
    /// protocol filter, tag filter, nodes generation). Latency batch tests
    /// repaint the Nodes page constantly; without this cache every repaint
    /// re-ran the whole filter pass (4 `to_lowercase()` per node) + sort.
    static NODE_FILTER_CACHE: std::cell::RefCell<NodeFilterCache> = std::cell::RefCell::new((
        String::new(),
        None,
        None,
        u64::MAX,
        std::rc::Rc::new(Vec::new()),
    ));
}

/// Shared with the Config page ("Clear Nodes" two-step confirm).
pub(crate) fn confirm_armed(slot: &AtomicU64) -> bool {
    let armed_at = slot.load(Ordering::Relaxed);
    armed_at != 0 && now_millis().saturating_sub(armed_at) < CONFIRM_WINDOW_MS
}

/// 1px hairline divider (design-system §3: nested sections use a divider, not a card).
fn hairline() -> Div {
    div().w_full().h(px(1.0)).bg(rgba(with_alpha(BORDER, 0x99)))
}

/// v2 cyan-tint button (px-3 py-1.5 rounded-lg, ACCENT 0x1a bg + 0x33
/// border + cyan text); used for "Test All" / "Test Latency" / "Connect".
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

impl AppState {
    /// Arm a destructive action for its 3 s confirm window and schedule the
    /// auto-revert repaint. UI-only; the actual action runs on the 2nd click.
    /// Shared with the Config page ("Clear Nodes").
    pub(crate) fn arm_destructive_confirm(&self, slot: &'static AtomicU64, cx: &mut Context<Self>) {
        slot.store(now_millis(), Ordering::Relaxed);
        cx.notify();
        let handle = self.tokio_handle.clone();
        cx.spawn(async move |weak, cx| {
            // Must run inside the tokio runtime (via handle.spawn) because
            // gpui's own executor does not provide a tokio reactor.
            handle
                .spawn(async {
                    tokio::time::sleep(std::time::Duration::from_millis(CONFIRM_WINDOW_MS + 50))
                        .await;
                })
                .await
                .ok();
            if !confirm_armed(slot) {
                slot.store(0, Ordering::Relaxed);
            }
            if let Ok(()) = weak.update(cx, |_, cx| cx.notify()) {}
        })
        .detach();
    }

    #[allow(clippy::let_and_return)]
    pub(crate) fn render_nodes(&mut self, cx: &mut Context<Self>) -> Div {
        let node_count = self.nodes.len();

        // Read and apply text filter
        let filter_text = self
            .node_filter_input
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        if filter_text != self.node_filter {
            self.node_filter = filter_text.clone();
        }

        // Collect distinct protocols present in the node list for chips
        let mut present_protocols: Vec<&'static str> = {
            let mut seen = std::collections::HashSet::new();
            self.nodes
                .iter()
                .map(|n| protocol_short_name(&n.protocol))
                .filter(|s| seen.insert(*s))
                .collect()
        };
        // Stable order: SS, VMess, VLESS, Trojan, TUIC, Hy2
        let protocol_order = ["ss", "vmess", "vless", "trojan", "tuic", "hy2"];
        present_protocols.sort_by_key(|p| protocol_order.iter().position(|o| o == p).unwrap_or(99));

        let proto_filter = self.protocol_filter;
        let tag_filter = self.tag_filter.clone();
        let nodes_generation = self.nodes_generation;
        let filtered_indices: std::rc::Rc<Vec<usize>> = NODE_FILTER_CACHE.with(|c| {
            let mut cache = c.borrow_mut();
            if cache.0 != filter_text
                || cache.1 != proto_filter
                || cache.2 != tag_filter
                || cache.3 != nodes_generation
            {
                let mut indices: Vec<usize> = (0..node_count)
                    .filter(|&i| {
                        let n = &self.nodes[i];
                        // Protocol chip filter
                        let proto_match = match proto_filter {
                            None => true,
                            Some(pf) => protocol_short_name(&n.protocol) == pf,
                        };
                        // Tag filter
                        let tag_match = match &tag_filter {
                            None => true,
                            Some(tf) => n.tags.iter().any(|t| t == tf),
                        };
                        // Text filter
                        let text_match = filter_text.is_empty()
                            || n.name.to_lowercase().contains(&filter_text)
                            || n.server.to_lowercase().contains(&filter_text)
                            || protocol_short_name(&n.protocol)
                                .to_lowercase()
                                .contains(&filter_text)
                            || n.tags
                                .iter()
                                .any(|t| t.to_lowercase().contains(&filter_text));
                        proto_match && tag_match && text_match
                    })
                    .collect();
                // Sort: tested nodes by latency ascending, untested last
                indices.sort_by(|&a, &b| {
                    match (self.nodes[a].latency_ms, self.nodes[b].latency_ms) {
                        (Some(a_ms), Some(b_ms)) => a_ms.cmp(&b_ms),
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (None, None) => std::cmp::Ordering::Equal,
                    }
                });
                *cache = (
                    filter_text.clone(),
                    proto_filter,
                    tag_filter.clone(),
                    nodes_generation,
                    std::rc::Rc::new(indices),
                );
            }
            cache.4.clone()
        });
        let _shown_count = filtered_indices.len();
        let node_filter_input = self.node_filter_input.clone();

        // Collect all unique tags across all nodes for filter chips
        let all_tags: Vec<String> = {
            let mut seen = std::collections::HashSet::new();
            let mut tags: Vec<String> = self
                .nodes
                .iter()
                .flat_map(|n| n.tags.iter().cloned())
                .filter(|t| seen.insert(t.clone()))
                .collect();
            tags.sort();
            tags
        };
        let cur_tag_filter = self.tag_filter.clone();

        let delete_all_armed = confirm_armed(&DELETE_ALL_ARMED_AT);
        let delete_selected_armed = confirm_armed(&DELETE_SELECTED_ARMED_AT);

        let content = div()
            .flex()
            .flex_col()
            .gap_3()
            .h_full()
            .min_h_0()
            // --- Subscription bar (v3): collapsible glass panel ---
            .child(self.render_subs_bar(cx))
            .when(self.show_share_panel, |d| {
                d.child(self.render_share_panel(cx))
            })
            // --- Batch action bar (v3): visible when nodes are checked ---
            .when(!self.selected_node_indices.is_empty(), |d| {
                let sel_count = self.selected_node_indices.len();
                let fi = filtered_indices.clone();
                d.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_2()
                        .rounded(px(8.0))
                        .bg(rgba(with_alpha(ACCENT, 0x0d)))
                        .border_1()
                        .border_color(rgba(with_alpha(ACCENT, 0x33)))
                        .child(
                            div()
                                .text_size(px(SMALL))
                                .font_family(MONO_FONT)
                                .text_color(rgb(ACCENT))
                                .child(format!("{} selected", sel_count)),
                        )
                        .child(div().flex_1())
                        .child(
                            Button::new("batch-test-btn").xsmall()
                                .label("Test".to_string())
                                .tooltip("Test latency for selected nodes")
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    // Test only the selected nodes, not the whole list.
                                    let mut indices: Vec<usize> = this
                                        .selected_node_indices
                                        .iter()
                                        .copied()
                                        .filter(|&i| i < this.nodes.len())
                                        .collect();
                                    indices.sort_unstable();
                                    this.pending_latency_batch = indices.len();
                                    this.auto_select_best = false;
                                    this.auto_select_scope = None;
                                    for i in indices {
                                        this.test_node_latency(i, cx);
                                    }
                                })),
                        )
                        .child(
                            Button::new("copy-share-btn").xsmall()
                                .label("Export".to_string())
                                .tooltip("Copy share URIs for selected nodes to clipboard")
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let links: Vec<String> = this.selected_node_indices
                                        .iter()
                                        .filter(|&&i| i < this.nodes.len())
                                        .map(|&i| sockrocket_core::config::v2ray::node_to_share_uri(&this.nodes[i]))
                                        .collect();
                                    let text = links.join("\n");
                                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                                })),
                        )
                        .child(
                            Button::new("select-all-btn").xsmall()
                                .label("Select All".to_string())
                                .tooltip("Select all visible nodes")
                                .ghost()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    for &i in fi.iter() {
                                        this.selected_node_indices.insert(i);
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("delete-selected-btn").xsmall()
                                .label(if delete_selected_armed {
                                    "Confirm?".to_string()
                                } else {
                                    format!("Delete ({})", sel_count)
                                })
                                .tooltip(if delete_selected_armed {
                                    "Click again within 3 s to delete the selected nodes"
                                } else {
                                    "Delete selected nodes"
                                })
                                .danger()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if confirm_armed(&DELETE_SELECTED_ARMED_AT) {
                                        DELETE_SELECTED_ARMED_AT.store(0, Ordering::Relaxed);
                                        // Stop the proxy first if the active node is
                                        // being deleted (same rule as delete_node).
                                        if this.proxy_running
                                            && this
                                                .active_proxy_node
                                                .is_some_and(|a| this.selected_node_indices.contains(&a))
                                        {
                                            this.stop_proxy(cx);
                                        }
                                        let mut indices: Vec<usize> = this.selected_node_indices.drain().collect();
                                        indices.sort_unstable_by(|a, b| b.cmp(a));
                                        for idx in indices {
                                            if idx < this.nodes.len() {
                                                this.nodes.remove(idx);
                                            }
                                        }
                                        this.selected_node = None;
                                        this.active_proxy_node = None;
                                        // Index-keyed latency state is invalid after
                                        // the reshuffle.
                                        this.latency_testing.clear();
                                        this.latency_failed.clear();
                                        this.latency_fail_reason.clear();
                                        this.pending_latency_batch = 0;
                                        this.auto_select_best = false;
                                        this.auto_select_scope = None;
                                        this.schedule_persist(cx);
                                        cx.notify();
                                    } else {
                                        this.arm_destructive_confirm(&DELETE_SELECTED_ARMED_AT, cx);
                                    }
                                })),
                        )
                        .child(
                            Button::new("deselect-all-btn").xsmall()
                                .label("Clear".to_string())
                                .tooltip("Clear selection")
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.selected_node_indices.clear();
                                    cx.notify();
                                })),
                        ),
                )
            })
            // --- Toolbar action feedback: alert strip, no popups (design-system §5.3) ---
            .when(!self.nodes_action_status.is_empty(), |d| {
                d.child(alert_strip(
                    status_alert_kind(&self.nodes_action_status),
                    self.nodes_action_status.clone(),
                ))
            })
            .child({
                // Filter chips (v3 bordered chips): protocol + tag
                let mut row = div()
                    .flex()
                    .flex_row()
                    .gap_1p5()
                    .flex_wrap()
                    .items_center();
                if !present_protocols.is_empty() {
                    let all_active = proto_filter.is_none();
                    row = row.child(
                        v3_chip("proto-all", "All", all_active).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.protocol_filter = None;
                                cx.notify();
                            },
                        )),
                    );
                    for proto_key in &present_protocols {
                        let key: &'static str = proto_key;
                        let active = proto_filter == Some(key);
                        let display = match key {
                            "ss" => "SS",
                            "vmess" => "VM",
                            "vless" => "VL",
                            "trojan" => "TR",
                            "tuic" => "TUIC",
                            "hy2" => "HY",
                            _ => key,
                        };
                        let btn_id = format!("proto-{}", key);
                        row = row.child(
                            v3_chip(SharedString::from(btn_id), display, active).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.protocol_filter = Some(key);
                                    cx.notify();
                                }),
                            ),
                        );
                    }
                }
                // Tag filter chips (only if any tags exist)
                if !all_tags.is_empty() {
                    row = row.child(
                        div()
                            .w(px(1.0))
                            .h(px(16.0))
                            .bg(rgba(with_alpha(BORDER, 0x99)))
                            .mx_1(),
                    );
                    let no_tag = cur_tag_filter.is_none();
                    row = row.child(
                        v3_chip("tag-all", "All Tags", no_tag).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.tag_filter = None;
                                cx.notify();
                            },
                        )),
                    );
                    for tag in &all_tags {
                        let tag_clone = tag.clone();
                        let active = cur_tag_filter.as_deref() == Some(tag.as_str());
                        let btn_id = format!("tag-{}", tag);
                        row = row.child(
                            v3_chip(SharedString::from(btn_id), SharedString::from(format!("#{}", tag)), active)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.tag_filter = Some(tag_clone.clone());
                                    cx.notify();
                                })),
                        );
                    }
                }

                // v2 body: horizontal flex — left column (toolbar + chips +
                // node table) flex-1, right column 240px detail panel.
                div()
                    .flex()
                    .flex_row()
                    .gap_4()
                    .flex_1()
                    .min_h_0()
                    // Left column: toolbar + filter chips + node table
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .gap_3()
                            // --- Toolbar (v2): search + Test All (cyan tint) + Sort + Dedup + Share ---
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(160.0))
                                            .max_w(px(280.0))
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap_2()
                                            .px_3()
                                            .py(px(6.0))
                                            .rounded(px(8.0))
                                            .bg(rgb(BG_HOVER))
                                            .border_1()
                                            .border_color(rgba(with_alpha(BORDER, 0x99)))
                                            .child(
                                                Icon::empty()
                                                    .path("icons/search.svg")
                                                    .with_size(ComponentSize::Size(px(14.0)))
                                                    .text_color(rgb(TEXT_MUTED)),
                                            )
                                            .child(
                                                gpui_component::input::Input::new(&node_filter_input)
                                                    .xsmall()
                                                    .appearance(false)
                                                    .w_full(),
                                            ),
                                    )
                                    .child(
                                        tint_button(
                                            "test-all-btn",
                                            if self.latency_testing.is_empty() {
                                                "Test All"
                                            } else {
                                                "Testing…"
                                            },
                                        )
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.auto_select_best = false;
                                                this.test_all_latency(cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("sort-latency-btn").xsmall()
                                            .label("Sort ↓".to_string())
                                            .tooltip("Sort nodes by latency (fastest first)")
                                            .ghost()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.sort_nodes_by_latency(cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("dedup-nodes-btn").xsmall()
                                            .label("Dedup".to_string())
                                            .tooltip("Remove duplicate nodes (same server + credentials)")
                                            .ghost()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.dedup_nodes_action(cx);
                                            })),
                                    )
                                    .child(div().flex_1())
                                    .child(
                                        Button::new("share-panel-btn").xsmall()
                                            .label("Share / Export".to_string())
                                            .tooltip("Export a subscription or share nodes on the LAN")
                                            .ghost()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.show_share_panel = !this.show_share_panel;
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new("delete-all-btn").xsmall()
                                            .label(if delete_all_armed {
                                                "Confirm?".to_string()
                                            } else {
                                                "Delete All".to_string()
                                            })
                                            .tooltip(if delete_all_armed {
                                                "Click again within 3 s to delete all nodes"
                                            } else {
                                                "Delete all nodes"
                                            })
                                            .when(delete_all_armed, |b| b.danger())
                                            .when(!delete_all_armed, |b| b.ghost())
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                if confirm_armed(&DELETE_ALL_ARMED_AT) {
                                                    DELETE_ALL_ARMED_AT.store(0, Ordering::Relaxed);
                                                    this.delete_all_nodes(cx);
                                                } else {
                                                    this.arm_destructive_confirm(&DELETE_ALL_ARMED_AT, cx);
                                                }
                                            })),
                                    ),
                            )
                            .child(row)
                            .child({
                                // v2: table in a glass panel; header fixed, list scrolls
                                let hcell = |text: &str| {
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_size(px(MICRO))
                                        .font_family(MONO_FONT)
                                        .text_color(rgb(TEXT_MUTED))
                                        .child(text.to_string())
                                };
                                let header = div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .px_3()
                                    .py_2()
                                    .border_b_1()
                                    .border_color(rgba(with_alpha(BORDER, 0x99)))
                                    .child(div().w(px(20.0)).flex_shrink_0())
                                    .child(hcell("NODE"))
                                    .child(
                                        div()
                                            .w(px(40.0))
                                            .flex_shrink_0()
                                            .text_size(px(MICRO))
                                            .font_family(MONO_FONT)
                                            .text_color(rgb(TEXT_MUTED))
                                            .text_center()
                                            .child("TYPE"),
                                    )
                                    .child(
                                        div()
                                            .w(px(64.0))
                                            .flex_shrink_0()
                                            .text_size(px(MICRO))
                                            .font_family(MONO_FONT)
                                            .text_color(rgb(TEXT_MUTED))
                                            .text_right()
                                            .child("LATENCY"),
                                    )
                                    .child(div().w(px(48.0)).flex_shrink_0());

                                let body = if node_count == 0 {
                                    empty_state(
                                        "No nodes yet",
                                        Some(
                                            Button::new("empty-add-subscription-btn").xsmall()
                                                .label("Add Subscription".to_string())
                                                .tooltip("Go to the Config tab to import a subscription")
                                                .primary()
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.active_view = ActiveView::Config;
                                                    this.config_tab = ConfigTab::ImportSubscription;
                                                    cx.notify();
                                                }))
                                                .into_any_element(),
                                        ),
                                    )
                                    .into_any_element()
                                } else if filtered_indices.is_empty() {
                                    empty_state("No nodes match the current filter", None)
                                        .into_any_element()
                                } else {
                                    // Virtualized rendering: only the visible
                                    // rows are built each frame. The previous
                                    // version rendered *every* filtered node
                                    // into a full element subtree on every
                                    // frame — with 165 nodes that's 165
                                    // `render_node_item` calls per frame,
                                    // which measurably stuttered scrolling.
                                    // `uniform_list` renders just the visible
                                    // range (rows are uniform height) and
                                    // provides its own scroll handling, so the
                                    // outer wrapper must NOT also scroll.
                                    let indices = filtered_indices.clone();
                                    uniform_list(
                                        "node-uniform-list",
                                        indices.len(),
                                        cx.processor(
                                            move |this, range: std::ops::Range<usize>, _window, cx| {
                                                let indices = indices.clone();
                                                range
                                                    .map(|row| {
                                                        let node_index = indices[row];
                                                        this.render_node_item(node_index, cx)
                                                            .into_any_element()
                                                    })
                                                    .collect::<Vec<_>>()
                                            },
                                        ),
                                    )
                                    .flex_1()
                                    .min_h_0()
                                    .into_any_element()
                                };

                                card()
                                    .p_0()
                                    .gap_0()
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_hidden()
                                    .when(node_count > 0, |d| d.child(header))
                                    .child(
                                        div()
                                            .id("node-table-body")
                                            // Must be a flex column, not the
                                            // default block: flex_1 only
                                            // propagates a definite height to
                                            // the uniform_list through a flex
                                            // chain; a block parent leaves the
                                            // list at its intrinsic height
                                            // (one row), rendering it blank.
                                            .flex()
                                            .flex_col()
                                            .flex_1()
                                            .min_h_0()
                                            // No overflow_y_scroll here: when
                                            // the body is a uniform_list it
                                            // scrolls itself; a nested scroller
                                            // would clip it to one row.
                                            .child(body),
                                    )
                            }),
                    )
                    // Right column: detail panel (v2: w-60 shrink-0)
                    .child(
                        div()
                            .w(px(240.0))
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .child(
                                if let Some(index) = self.selected_node {
                                    if let Some(node) = self.nodes.get(index).cloned() {
                                        self.render_selected_node_details(&node, index, cx)
                                            .into_any_element()
                                    } else {
                                        div().into_any_element()
                                    }
                                } else {
                                    card()
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .items_center()
                                                .py_8()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .text_size(px(SMALL))
                                                        .text_color(rgb(TEXT_SECONDARY))
                                                        .child("No node selected"),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(SMALL))
                                                        .text_color(rgb(TEXT_MUTED))
                                                        .child("Click a node to view details"),
                                                ),
                                        )
                                        .into_any_element()
                                },
                            ),
                    )
            });

        content
    }

    /// Export & LAN share panel (toggled from the Nodes toolbar).
    pub(crate) fn render_share_panel(&mut self, cx: &mut Context<Self>) -> Div {
        let export_format = self.export_format;
        let export_status = self.export_status.clone();
        let scope_label = self.export_scope_label();
        let lan_on = self.lan_share_on;
        let lan_format = self.lan_share_format;
        let lan_status = self.lan_share_status.clone();
        let lan_url = self.lan_share_url();
        let lan_token = self.lan_share_token.clone();
        let lan_ip_missing = self.lan_ip.is_none();
        let port_input = self.lan_share_port_input.clone();

        card()
            // --- Export subscription ---
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .flex_wrap()
                            .gap_2()
                            .child(section_label("Export Subscription"))
                            .child(
                                div()
                                    .text_size(px(MICRO))
                                    .font_family(MONO_FONT)
                                    .text_color(rgb(TEXT_MUTED))
                                    .child(format!("({})", scope_label)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                tab_button(
                                    "export-fmt-v2ray",
                                    "V2Ray base64",
                                    export_format == SubscriptionFormat::V2ray,
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.export_format = SubscriptionFormat::V2ray;
                                    cx.notify();
                                })),
                            )
                            .child(
                                tab_button(
                                    "export-fmt-clash",
                                    "Clash YAML",
                                    export_format == SubscriptionFormat::Clash,
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.export_format = SubscriptionFormat::Clash;
                                    cx.notify();
                                })),
                            )
                            .child(
                                Button::new("export-copy-btn").xsmall()
                                    .label("Copy".to_string())
                                    .tooltip("Copy the subscription to the clipboard")
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.copy_export_to_clipboard(cx);
                                    })),
                            )
                            .child(
                                Button::new("export-save-btn").xsmall()
                                    .label("Save to File".to_string())
                                    .tooltip("Save next to the app config file; full path is shown below")
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.save_export_to_file(cx);
                                    })),
                            ),
                    )
                    .when(!export_status.is_empty(), |d| {
                        d.child(alert_strip(status_alert_kind(&export_status), export_status))
                    }),
            )
            // --- Divider ---
            .child(hairline())
            // --- LAN share ---
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .flex_wrap()
                            .gap_2()
                            .child(section_label("LAN Share"))
                            .child(
                                Button::new("lan-share-toggle-btn").xsmall()
                                    .label(if lan_on { "Stop" } else { "Start" }.to_string())
                                    .tooltip("Serve selected nodes (or all, when none are selected) as a subscription on the LAN")
                                    .when(lan_on, |b| b.danger())
                                    .when(!lan_on, |b| b.primary())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_lan_share(cx);
                                    })),
                            )
                            .child(
                                div()
                                    .text_size(px(SMALL))
                                    .text_color(rgb(TEXT_MUTED))
                                    .child("Port"),
                            )
                            .child(
                                div().w(px(72.0)).child(
                                    gpui_component::input::Input::new(&port_input).xsmall().w_full(),
                                ),
                            )
                            .child(
                                tab_button(
                                    "lan-fmt-clash",
                                    "Clash",
                                    lan_format == SubscriptionFormat::Clash,
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.set_lan_share_format(SubscriptionFormat::Clash, cx);
                                })),
                            )
                            .child(
                                tab_button(
                                    "lan-fmt-v2ray",
                                    "V2Ray",
                                    lan_format == SubscriptionFormat::V2ray,
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.set_lan_share_format(SubscriptionFormat::V2ray, cx);
                                })),
                            ),
                    )
                    .when(!lan_status.is_empty(), |d| {
                        d.child(alert_strip(status_alert_kind(&lan_status), lan_status))
                    })
                    .when_some(lan_url, |d, url| {
                        let qr = render_qr(&url, 3.0);
                        let url_copy = url.clone();
                        d.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .text_size(px(MICRO))
                                                .font_family(MONO_FONT)
                                                .text_color(rgb(ACCENT))
                                                .child(url),
                                        )
                                        .child(
                                            Button::new("lan-copy-url-btn").xsmall()
                                                .label("Copy URL".to_string())
                                                .tooltip("Copy the subscription URL")
                                                .ghost()
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    cx.write_to_clipboard(
                                                        ClipboardItem::new_string(url_copy.clone()),
                                                    );
                                                    this.lan_share_status =
                                                        "✓ URL copied to clipboard".to_string();
                                                    cx.notify();
                                                })),
                                        ),
                                )
                                .child(kv_row("Token", lan_token))
                                .when(lan_ip_missing, |d| {
                                    d.child(alert_strip(
                                        AlertKind::Warning,
                                        "No LAN IP detected — URL uses 127.0.0.1 (this machine only)",
                                    ))
                                })
                                .when_some(qr, |d, qr| {
                                    d.child(
                                        div()
                                            .pt_1()
                                            .child(qr)
                                            .child(
                                                div()
                                                    .pt_1()
                                                    .text_size(px(MICRO))
                                                    .font_family(MONO_FONT)
                                                    .text_color(rgb(TEXT_MUTED))
                                                    .child("Scan with a phone to subscribe"),
                                            ),
                                    )
                                }),
                        )
                    }),
            )
    }

    pub(crate) fn render_node_item(
        &mut self,
        index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let node = &self.nodes[index];
        let is_selected = self.selected_node == Some(index);
        let is_active = self.proxy_running && self.active_proxy_node == Some(index);
        let name = node.name.clone();
        let proto_short = protocol_short_name(&node.protocol);
        let is_testing = self.latency_testing.contains(&index);
        let is_failed = self.latency_failed.contains(&index);
        // Classified failure reason (timeout / server unreachable / …) for the badge
        // tooltip; a failed node must never show a latency number.
        let fail_reason = self
            .latency_fail_reason
            .get(&index)
            .map(|k| k.badge_label())
            .unwrap_or("Unknown error");
        let latency_color = node
            .latency_ms
            .map(|ms| rgb(latency_color(ms as u64)))
            .unwrap_or(rgb(TEXT_MUTED));

        let bg = if is_active {
            rgba(with_alpha(ACCENT, 0x14))
        } else if is_selected {
            rgba(with_alpha(BG_ACTIVE_ROW, 0x99))
        } else {
            rgba(0x00000000u32)
        };

        // Row actions are revealed on hover via group_hover styles — no
        // cx.notify round-trip per mouse enter/leave.
        let row_group = SharedString::from(format!("node-row-{}", index));

        div()
            .id(SharedString::from(format!("node-{}", index)))
            .group(row_group.clone())
            // uniform_list lays items out shrink-to-fit, so the row must
            // claim the full width explicitly — otherwise the TYPE/LATENCY
            // cells hug the name and drift out of alignment with the header.
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(rgba(with_alpha(BORDER, 0x66)))
            .bg(bg)
            .cursor_pointer()
            .hover(|s| s.bg(rgb(BG_HOVER)))
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                this.select_node(index, cx);
                // Double-click switches the proxy onto this node (v2 UX).
                if event.click_count() == 2 {
                    this.switch_to_node(index, cx);
                }
            }))
            // Column 1: selection checkbox (v3: w-5 centered)
            .child(
                div()
                    .w(px(20.0))
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .justify_center()
                    .child({
                        let is_checked = self.selected_node_indices.contains(&index);
                        let checkbox_bg = if is_checked {
                            rgb(ACCENT)
                        } else {
                            rgba(0x00000000u32)
                        };
                        let checkbox_border = if is_checked {
                            rgb(ACCENT)
                        } else {
                            rgba(with_alpha(BORDER, 0x99))
                        };
                        div()
                            .id(SharedString::from(format!("node-check-{}", index)))
                            .w(px(13.0))
                            .h(px(13.0))
                            .rounded(px(3.0))
                            .flex_shrink_0()
                            .cursor_pointer()
                            .border_1()
                            .border_color(checkbox_border)
                            .bg(checkbox_bg)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.selected_node_indices.contains(&index) {
                                    this.selected_node_indices.remove(&index);
                                } else {
                                    this.selected_node_indices.insert(index);
                                }
                                cx.notify();
                            }))
                    }),
            )
            // Column 2: node name (+ "in use" badge when active; v3)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .min_w_0()
                            .text_size(px(SMALL))
                            .font(localized_font())
                            .font_weight(if is_active {
                                FontWeight::MEDIUM
                            } else {
                                FontWeight::NORMAL
                            })
                            .text_color(rgb(if is_active { ACCENT } else { TEXT_PRIMARY }))
                            .overflow_x_hidden()
                            .child(name),
                    )
                    .when(is_active, |d| d.child(mini_badge("in use", ACCENT))),
            )
            // Column 4: protocol badge (v2: colored mini_badge, w-10 centered)
            .child(
                div()
                    .w(px(40.0))
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .justify_center()
                    .child(mini_badge(proto_short, proto_badge_color(proto_short))),
            )
            // Column 5: latency (v3: w-16 right-aligned, value + small "ms")
            .child(
                div()
                    .w(px(64.0))
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .items_baseline()
                    .justify_end()
                    .gap_1()
                    .child(if is_testing {
                        // 1 Hz blink driven by the UI heartbeat (which repaints
                        // the Nodes page while latency tests are in-flight).
                        // The previous infinite `with_animation` pulse forced a
                        // full-window repaint at display refresh rate for the
                        // entire duration of a batch latency test.
                        let blink_on = (now_millis() / 1000).is_multiple_of(2);
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .w(px(5.0))
                                    .h(px(5.0))
                                    .rounded_full()
                                    .bg(rgb(ACCENT))
                                    .opacity(if blink_on { 1.0 } else { 0.25 }),
                            )
                            .child(
                                div()
                                    .text_size(px(MICRO))
                                    .font_family(MONO_FONT)
                                    .text_color(rgb(TEXT_MUTED))
                                    .child("…"),
                            )
                            .into_any_element()
                    } else {
                        match node.latency_ms {
                            Some(ms) => div()
                                .flex()
                                .flex_row()
                                .items_baseline()
                                .gap(px(2.0))
                                .child(
                                    div()
                                        .text_size(px(SMALL))
                                        .font_family(MONO_FONT)
                                        .text_color(latency_color)
                                        .child(format!("{}", ms)),
                                )
                                .child(
                                    div()
                                        .text_size(px(MICRO))
                                        .font_family(MONO_FONT)
                                        .text_color(rgb(TEXT_MUTED))
                                        .child("ms"),
                                )
                                .into_any_element(),
                            None => {
                                if is_failed {
                                    // Failed probe: red "Unreachable" badge in
                                    // place of any latency figure; tooltip
                                    // carries the classified reason.
                                    div()
                                        .id(SharedString::from(format!("node-fail-{}", index)))
                                        .child(mini_badge("Unreachable", DANGER))
                                        .tooltip(move |window, cx| {
                                            gpui_component::tooltip::Tooltip::new(format!(
                                                "Unreachable: {}",
                                                fail_reason
                                            ))
                                            .build(window, cx)
                                        })
                                        .into_any_element()
                                } else {
                                    div()
                                        .text_size(px(MICRO))
                                        .font_family(MONO_FONT)
                                        .text_color(latency_color)
                                        .child("---")
                                        .into_any_element()
                                }
                            }
                        }
                    }),
            )
            // Row actions (v2: revealed on hover — use / copy URI / delete, SVG icons)
            .child(
                div()
                    .w(px(48.0))
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap_1()
                    .invisible()
                    .group_hover(row_group, |s| s.visible())
                    .child(
                        div()
                            .id(SharedString::from(format!("node-use-{}", index)))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .child(
                                Icon::empty()
                                    .path("icons/bolt.svg")
                                    .with_size(ComponentSize::Size(px(12.0)))
                                    .text_color(rgb(TEXT_MUTED)),
                            )
                            .hover(|s| s.text_color(rgb(ACCENT)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.use_node_by_index(index, cx);
                            })),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("node-copy-{}", index)))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .child(
                                Icon::empty()
                                    .path("icons/copy.svg")
                                    .with_size(ComponentSize::Size(px(12.0)))
                                    .text_color(rgb(TEXT_MUTED)),
                            )
                            .hover(|s| s.text_color(rgb(ACCENT)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(node) = this.nodes.get(index) {
                                    let uri =
                                        sockrocket_core::config::v2ray::node_to_share_uri(node);
                                    cx.write_to_clipboard(ClipboardItem::new_string(uri));
                                }
                            })),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("node-del-{}", index)))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .child(
                                Icon::empty()
                                    .path("icons/x.svg")
                                    .with_size(ComponentSize::Size(px(12.0)))
                                    .text_color(rgb(TEXT_MUTED)),
                            )
                            .hover(|s| s.text_color(rgb(DANGER)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.delete_node(index, cx);
                            })),
                    ),
            )
    }

    /// v3 collapsible subscription bar at the top of the Nodes page.
    /// Header shows sub/node counts; expanding lists each subscription with
    /// node count, last-refresh time and a refresh button.
    pub(crate) fn render_subs_bar(&mut self, cx: &mut Context<Self>) -> Div {
        let sub_count = self.subscriptions.len();
        let node_count = self.nodes.len();
        let open = SUBS_BAR_OPEN.load(Ordering::Relaxed);

        let mut panel = card().p_0().gap_0().child(
            div()
                .id("subs-bar-toggle")
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .px(px(14.0))
                .py_2p5()
                .cursor_pointer()
                .rounded(px(12.0))
                .hover(|s| s.bg(rgba(with_alpha(BG_HOVER, 0x66))))
                .on_click(cx.listener(|_, _, _, cx| {
                    let open = SUBS_BAR_OPEN.load(Ordering::Relaxed);
                    SUBS_BAR_OPEN.store(!open, Ordering::Relaxed);
                    cx.notify();
                }))
                .child(
                    div()
                        .text_size(px(SMALL))
                        .text_color(rgb(TEXT_MUTED))
                        .child(if open { "▾" } else { "▸" }),
                )
                .child(
                    div()
                        .text_size(px(SMALL))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(TEXT_PRIMARY))
                        .child("Subscriptions"),
                )
                .child(
                    div()
                        .text_size(px(TINY))
                        .font_family(MONO_FONT)
                        .text_color(rgb(TEXT_MUTED))
                        .child(format!("{} subs · {} nodes", sub_count, node_count)),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .id("subs-bar-manage")
                        .cursor_pointer()
                        .text_size(px(TINY))
                        .text_color(rgb(ACCENT))
                        .hover(|s| s.text_color(rgb(TEXT_ACCENT)))
                        .child("Manage →")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.active_view = ActiveView::Config;
                            this.config_tab = ConfigTab::ImportSubscription;
                            cx.notify();
                        })),
                ),
        );

        if open {
            let mut body = div()
                .flex()
                .flex_col()
                .gap_2()
                .border_t_1()
                .border_color(rgba(with_alpha(BORDER, 0x99)))
                .px(px(14.0))
                .py_2p5();
            if sub_count == 0 {
                body = body.child(
                    div()
                        .text_size(px(SMALL))
                        .text_color(rgb(TEXT_MUTED))
                        .child("No subscriptions — add one on the Config page."),
                );
            }
            for (i, sub) in self.subscriptions.iter().enumerate() {
                let last_updated = Self::format_last_updated(sub.last_updated);
                let name = sub.name.clone();
                let sub_nodes = self
                    .nodes
                    .iter()
                    .filter(|n| n.extra.get("sub_url") == Some(&sub.url))
                    .count();
                let is_refreshing = self.refreshing_subscriptions.contains(&i);
                body = body.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2p5()
                        .px_2p5()
                        .py(px(6.0))
                        .rounded(px(8.0))
                        .bg(rgba(with_alpha(BG_HOVER, 0x80)))
                        .border_1()
                        .border_color(rgba(with_alpha(BORDER, 0x99)))
                        .child(status_dot(if is_refreshing { ACCENT } else { SUCCESS }))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(1.0))
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .text_size(px(SMALL))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(TEXT_PRIMARY))
                                        .overflow_x_hidden()
                                        .child(name),
                                )
                                .child(
                                    div()
                                        .text_size(px(TINY))
                                        .font_family(MONO_FONT)
                                        .text_color(rgb(TEXT_MUTED))
                                        .child(format!("{} nodes · {}", sub_nodes, last_updated)),
                                ),
                        )
                        .child(
                            Button::new(("subs-bar-refresh", i))
                                .xsmall()
                                .label("Refresh".to_string())
                                .tooltip("Fetch new nodes from this subscription now")
                                .ghost()
                                .loading(is_refreshing)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.refresh_subscription(i, cx);
                                })),
                        ),
                );
            }
            panel = panel.child(body);
        }

        panel
    }

    pub(crate) fn render_subscription_list(&mut self, cx: &mut Context<Self>) -> Div {
        let sub_count = self.subscriptions.len();
        let section = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(section_label("Subscriptions"))
                    .child(
                        div()
                            .text_size(px(MICRO))
                            .font_family(MONO_FONT)
                            .text_color(rgb(TEXT_MUTED))
                            .child(format!("{}", sub_count)),
                    ),
            )
            .child(hairline());

        if sub_count == 0 {
            return section.child(
                div()
                    .text_size(px(SMALL))
                    .text_color(rgb(TEXT_MUTED))
                    .child("No subscriptions saved. Import one above."),
            );
        }

        let mut rows = div().flex().flex_col();
        for (i, sub) in self.subscriptions.iter().enumerate() {
            let last_updated = Self::format_last_updated(sub.last_updated);
            let interval_label = match sub.refresh_interval_hours {
                0 => "Off".to_string(),
                1 => "1h".to_string(),
                h => format!("{}h", h),
            };
            let url_display = if sub.url.len() > 48 {
                format!("{}…", &sub.url[..48])
            } else {
                sub.url.clone()
            };
            let name = sub.name.clone();
            let node_count = self
                .nodes
                .iter()
                .filter(|n| n.extra.get("sub_url") == Some(&sub.url))
                .count();
            let is_refreshing = self.refreshing_subscriptions.contains(&i);

            let row = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .px_2()
                .py_2()
                .border_b_1()
                .border_color(rgba(with_alpha(BORDER, 0x99)))
                .hover(|s| s.bg(rgb(BG_HOVER)))
                // Left: name + meta info
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_size(px(SMALL))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(TEXT_PRIMARY))
                                        .child(name),
                                )
                                .child(
                                    div()
                                        .text_size(px(MICRO))
                                        .font_family(MONO_FONT)
                                        .text_color(rgb(TEXT_MUTED))
                                        .child(format!("{} nodes", node_count)),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap_2()
                                .child(
                                    div()
                                        .min_w_0()
                                        .text_size(px(MICRO))
                                        .font_family(MONO_FONT)
                                        .text_color(rgb(TEXT_MUTED))
                                        .overflow_x_hidden()
                                        .child(url_display),
                                )
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .text_size(px(MICRO))
                                        .font_family(MONO_FONT)
                                        .text_color(rgb(TEXT_MUTED))
                                        .child(if is_refreshing {
                                            "· refreshing…".to_string()
                                        } else {
                                            format!("· {}", last_updated)
                                        }),
                                ),
                        ),
                )
                // Right: action buttons
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_1()
                        .items_center()
                        .child(
                            Button::new(("interval-btn", i))
                                .xsmall()
                                .label(format!("Auto: {}", interval_label))
                                .tooltip("Click to cycle: Off → 1h → 2h → 6h → 12h → 24h → Off")
                                .ghost()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.cycle_refresh_interval(i, cx);
                                })),
                        )
                        .child(
                            Button::new(("refresh-sub-btn", i))
                                .xsmall()
                                .label("Refresh".to_string())
                                .tooltip("Fetch new nodes from this subscription now")
                                .ghost()
                                .loading(is_refreshing)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.refresh_subscription(i, cx);
                                })),
                        )
                        .child(
                            Button::new(("delete-sub-btn", i))
                                .xsmall()
                                .label("×".to_string())
                                .tooltip("Remove this subscription and its nodes")
                                .ghost()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.delete_subscription(i, cx);
                                })),
                        ),
                );
            rows = rows.child(row);
        }
        section.child(rows)
    }

    pub(crate) fn render_selected_node_details(
        &mut self,
        node: &Node,
        index: usize,
        cx: &mut Context<Self>,
    ) -> Div {
        let status = if self.proxy_running && self.active_proxy_node == Some(index) {
            "Active"
        } else if self.selected_node == Some(index) {
            "Selected"
        } else {
            "Idle"
        };
        let status_color = if self.proxy_running && self.active_proxy_node == Some(index) {
            SUCCESS
        } else if self.selected_node == Some(index) {
            ACCENT
        } else {
            TEXT_MUTED
        };

        // A node with no latency is either untested or known-unreachable. Say
        // which, so a dead node never looks like one that simply hasn't been
        // measured yet. Failed nodes carry the classified reason (timeout /
        // server unreachable / TLS handshake failed / protocol error) in the status line.
        let latency_str = node
            .latency_ms
            .map(|ms| format!("{} ms", ms))
            .unwrap_or_else(|| {
                if self.latency_failed.contains(&index) {
                    let reason = self
                        .latency_fail_reason
                        .get(&index)
                        .map(|k| k.badge_label())
                        .unwrap_or("Unknown error");
                    format!("Unreachable · {}", reason)
                } else {
                    "Not tested".to_string()
                }
            });

        let latency_color = node
            .latency_ms
            .map(|ms| latency_color(ms as u64))
            .unwrap_or(if self.latency_failed.contains(&index) {
                DANGER
            } else {
                TEXT_MUTED
            });

        // Subscription source (from sub_url tag)
        let sub_source = node.extra.get("sub_url").and_then(|url| {
            self.subscriptions
                .iter()
                .find(|s| &s.url == url)
                .map(|s| s.name.clone())
        });

        // UDP support
        let udp_enabled = match &node.protocol {
            ProxyProtocol::Shadowsocks { udp, .. } => *udp,
            ProxyProtocol::VMess { udp, .. } => *udp,
            ProxyProtocol::VLess { udp, .. } => *udp,
            ProxyProtocol::Tuic { udp, .. } => *udp,
            ProxyProtocol::Trojan { udp, .. } => *udp,
            ProxyProtocol::Hysteria2 { udp, .. } => *udp,
        };

        // Masked UUID/password (first 8 chars + "…")
        let masked_id: Option<String> = match &node.protocol {
            ProxyProtocol::VMess { uuid, .. }
            | ProxyProtocol::VLess { uuid, .. }
            | ProxyProtocol::Tuic { uuid, .. } => {
                Some(format!("{}…", uuid.get(..8).unwrap_or(uuid.as_str())))
            }
            _ => None,
        };

        let is_active = self.proxy_running && self.active_proxy_node == Some(index);
        let is_editing = self.editing_node_index == Some(index);
        let qr_expanded = self.qr_expanded_node == Some(index);
        let node_rename_input = self.node_rename_input.clone();
        let node_tag_input = self.node_tag_input.clone();
        let node_tags = node.tags.clone();
        let proto_short = protocol_short_name(&node.protocol);
        let is_testing_latency = self.latency_testing.contains(&index);

        // TLS summary: green "Enabled"-style value when secured, muted otherwise.
        let security = security_summary(node);
        let tls_on = security != "Default";

        // XTLS flow (VLESS only, real data from the protocol variant).
        let flow: Option<String> = match &node.protocol {
            ProxyProtocol::VLess { flow, .. } => flow.clone(),
            _ => None,
        };

        card()
            .gap_3()
            // v2: section label
            .child(section_label("Node Details"))
            // v2: node name (BODY medium) + sub info (MICRO muted) + status
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(1.0))
                            .child(
                                div()
                                    .text_size(px(BODY))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(TEXT_PRIMARY))
                                    .font(localized_font())
                                    .overflow_x_hidden()
                                    .child(node.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(MICRO))
                                    .font_family(MONO_FONT)
                                    .text_color(rgb(TEXT_MUTED))
                                    .child(format!(
                                        "{} · {}",
                                        protocol_name(&node.protocol),
                                        latency_str
                                    )),
                            ),
                    )
                    .child(status_dot(status_color))
                    .child(
                        div()
                            .text_size(px(SMALL))
                            .text_color(rgb(status_color))
                            .child(status),
                    ),
            )
            .when(is_editing, |card| {
                card.child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .items_center()
                        .child(
                            div().flex_1().child(
                                gpui_component::input::Input::new(&node_rename_input).xsmall().w_full(),
                            ),
                        )
                        .child(
                            Button::new("rename-confirm-btn")
                                .xsmall()
                                .label("Save".to_string())
                                .tooltip("Confirm rename")
                                .primary()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let new_name =
                                        this.node_rename_input.read(cx).value().trim().to_string();
                                    if !new_name.is_empty() {
                                        if let Some(n) = this.nodes.get_mut(index) {
                                            n.name = new_name;
                                        }
                                        this.schedule_persist(cx);
                                    }
                                    this.editing_node_index = None;
                                    cx.notify();
                                })),
                        ),
                )
            })
            // v2 key/value detail list
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0()
                    // Protocol: value rendered as a colored mini badge.
                    .child(
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
                                    .child("Protocol"),
                            )
                            .child(mini_badge(proto_short, proto_badge_color(proto_short))),
                    )
                    .child(kv_row("Server", node.server.clone()))
                    .child(kv_row("Port", node.port.to_string()))
                    .child(
                        // Latency keeps its semantic color, so it uses a kv-style
                        // row with a custom value color instead of `kv_row`.
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
                                    .child("Latency"),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .text_size(px(MICRO))
                                    .font_family(MONO_FONT)
                                    .text_color(rgb(latency_color))
                                    .child(latency_str),
                            ),
                    )
                    .when_some(flow, |d, f| d.child(kv_row("Flow", f)))
                    .child(kv_row("Transport", transport_summary(node.transport.as_ref())))
                    // TLS: green value when enabled, muted otherwise (v2).
                    .child(
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
                                    .child("TLS"),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .text_size(px(MICRO))
                                    .font_family(MONO_FONT)
                                    .text_color(rgb(if tls_on { SUCCESS } else { TEXT_MUTED }))
                                    .child(if tls_on { security } else { "None".to_string() }),
                            ),
                    )
                    .child(kv_row("Auth", auth_summary(&node.protocol)))
                    .when_some(masked_id, |d, id| d.child(kv_row("UUID", id)))
                    .child(kv_row("UDP", if udp_enabled { "Enabled" } else { "Disabled" }))
                    .when_some(sub_source, |d, name| {
                        d.child(kv_row("Subscription", name))
                    }),
            )
            // v2 actions: Test Latency (cyan tint) / Copy URI / Show QR / Connect.
            .child(hairline())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .child(if is_active {
                        // Full-width danger-tint disconnect.
                        div()
                            .id("deactivate-btn")
                            .w_full()
                            .flex()
                            .flex_row()
                            .justify_center()
                            .px_3()
                            .py(px(6.0))
                            .rounded(px(8.0))
                            .cursor_pointer()
                            .border_1()
                            .bg(rgba(with_alpha(DANGER, 0x1a)))
                            .border_color(rgba(with_alpha(DANGER, 0x33)))
                            .text_size(px(SMALL))
                            .text_color(rgb(DANGER))
                            .hover(|s| s.bg(rgba(with_alpha(DANGER, 0x33))))
                            .child("Disconnect")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.stop_proxy(cx);
                            }))
                            .into_any_element()
                    } else {
                        tint_button("activate-btn", "Connect")
                            .w_full()
                            .justify_center()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected_node = Some(index);
                                this.start_proxy(cx);
                            }))
                            .into_any_element()
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_1p5()
                            .child(
                                div().flex_1().child(
                                    tint_button(
                                        "test-lat-btn",
                                        if is_testing_latency { "Testing…" } else { "Test Latency" },
                                    )
                                    .w_full()
                                    .justify_center()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.test_node_latency(index, cx);
                                    })),
                                ),
                            )
                            .child(
                                div().flex_1().child(
                                    Button::new("copy-uri-btn")
                                        .xsmall()
                                        .label("Copy URI".to_string())
                                        .tooltip("Copy this node's share URI to the clipboard")
                                        .ghost()
                                        .w_full()
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Some(node) = this.nodes.get(index) {
                                                let uri =
                                                    sockrocket_core::config::v2ray::node_to_share_uri(node);
                                                cx.write_to_clipboard(ClipboardItem::new_string(uri));
                                            }
                                        })),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_1p5()
                            .child(
                                div().flex_1().child(
                                    Button::new("node-qr-btn")
                                        .xsmall()
                                        .label("Show QR".to_string())
                                        .tooltip("Show this node's share URI as a QR code")
                                        .when(qr_expanded, |b| b.primary())
                                        .when(!qr_expanded, |b| b.ghost())
                                        .w_full()
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.qr_expanded_node =
                                                if this.qr_expanded_node == Some(index) {
                                                    None
                                                } else {
                                                    Some(index)
                                                };
                                            cx.notify();
                                        })),
                                ),
                            )
                            .child(
                                div().flex_1().child(if is_editing {
                                    Button::new("rename-cancel-btn")
                                        .xsmall()
                                        .label("Cancel".to_string())
                                        .tooltip("Cancel rename")
                                        .ghost()
                                        .w_full()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.editing_node_index = None;
                                            cx.notify();
                                        }))
                                        .into_any_element()
                                } else {
                                    Button::new("rename-btn")
                                        .xsmall()
                                        .label("Rename".to_string())
                                        .tooltip("Rename this node")
                                        .ghost()
                                        .w_full()
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.editing_node_index = Some(index);
                                            let current_name = this
                                                .nodes
                                                .get(index)
                                                .map(|n| n.name.clone())
                                                .unwrap_or_default();
                                            this.node_rename_input.update(cx, |state, cx| {
                                                state.set_value(current_name, window, cx);
                                            });
                                            cx.notify();
                                        }))
                                        .into_any_element()
                                }),
                            ),
                    ),
            )
            // Tags section
            .child(hairline())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(section_label("Tags"))
                    .when(!node_tags.is_empty(), |d| {
                        let mut tag_row = div().flex().flex_row().gap_1().flex_wrap();
                        for tag in &node_tags {
                            let tag_clone = tag.clone();
                            tag_row = tag_row.child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_0p5()
                                    .px_1()
                                    .py(px(1.0))
                                    .rounded(px(3.0))
                                    .border_1()
                                    .border_color(rgba(with_alpha(BORDER, 0x99)))
                                    .child(
                                        div()
                                            .text_size(px(MICRO))
                                            .font_family(MONO_FONT)
                                            .text_color(rgb(TEXT_SECONDARY))
                                            .child(format!("#{}", tag)),
                                    )
                                    .child(
                                        Button::new(SharedString::from(format!(
                                            "del-tag-{}",
                                            tag_clone
                                        )))
                                        .xsmall()
                                        .label("×".to_string())
                                        .ghost()
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Some(n) = this.nodes.get_mut(index) {
                                                n.tags.retain(|t| t != &tag_clone);
                                            }
                                            this.schedule_persist(cx);
                                            cx.notify();
                                        })),
                                    ),
                            );
                        }
                        d.child(tag_row)
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_1()
                            .items_center()
                            .child(div().flex_1().min_w_0().child(
                                gpui_component::input::Input::new(&node_tag_input).xsmall().w_full(),
                            ))
                            .child(
                                Button::new("add-tag-btn")
                                    .xsmall()
                                    .label("Add".to_string())
                                    .tooltip("Add tag to this node")
                                    .ghost()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        let tag = this
                                            .node_tag_input
                                            .read(cx)
                                            .value()
                                            .trim()
                                            .to_string();
                                        if tag.is_empty() {
                                            return;
                                        }
                                        if let Some(n) = this.nodes.get_mut(index)
                                            && !n.tags.contains(&tag)
                                        {
                                            n.tags.push(tag);
                                        }
                                        this.node_tag_input.update(cx, |s, cx| {
                                            s.set_value("", window, cx);
                                        });
                                        this.schedule_persist(cx);
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .pt_1()
                            .text_size(px(MICRO))
                            .font_family(MONO_FONT)
                            .text_color(rgb(TEXT_MUTED))
                            .child(protocol_note(&node.protocol)),
                    ),
            )
            // Collapsible QR section
            .when(qr_expanded, |card| {
                let share_uri = sockrocket_core::config::v2ray::node_to_share_uri(node);
                let qr = render_qr(&share_uri, 4.0);
                let uri_copy = share_uri.clone();
                card.child(hairline())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_2()
                            .when_some(qr, |d, qr| d.child(qr))
                            .child(
                                div()
                                    .w_full()
                                    .text_size(px(MICRO))
                                    .font_family(MONO_FONT)
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .overflow_x_hidden()
                                    .child(share_uri),
                            )
                            .child(
                                Button::new("node-qr-copy-btn")
                                    .xsmall()
                                    .label("Copy URI".to_string())
                                    .tooltip("Copy share URI to clipboard")
                                    .ghost()
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            uri_copy.clone(),
                                        ));
                                    })),
                            ),
                    )
            })
    }
}

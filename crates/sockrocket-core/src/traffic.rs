//! Per-node traffic accounting for the current process and a rolling history.
//!
//! Session counters live in memory and reset when the process starts. Monthly
//! totals are written next to the config so the last six months survive
//! restarts. Keys are [`crate::node_public_id`] values so the history file
//! never stores protocol credentials.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Keep the current calendar month plus the previous five (six months total).
pub const TRAFFIC_HISTORY_MONTHS: u32 = 6;

/// Attribute for Direct mode / no selected node.
pub const DIRECT_TRAFFIC_ID: &str = "direct";

// A single background writer keeps flash I/O off proxy read/write polls.
const PERSIST_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrafficCounters {
    #[serde(default)]
    pub up: u64,
    #[serde(default)]
    pub down: u64,
}

impl TrafficCounters {
    pub fn total(&self) -> u64 {
        self.up.saturating_add(self.down)
    }

    fn add(&mut self, up: u64, down: u64) {
        self.up = self.up.saturating_add(up);
        self.down = self.down.saturating_add(down);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeTrafficSnapshot {
    pub id: String,
    pub name: String,
    pub session: TrafficCounters,
    /// Sum of monthly buckets still retained (includes the current month).
    pub history: TrafficCounters,
}

/// Per-node monthly breakdown for a fixed window (oldest month first).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeMonthlyTraffic {
    pub id: String,
    pub name: String,
    /// `(YYYY-MM, counters)` for each month in the window, including empty months.
    pub months: Vec<(String, TrafficCounters)>,
    pub total: TrafficCounters,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct MonthEntry {
    #[serde(default)]
    up: u64,
    #[serde(default)]
    down: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NodeHistory {
    #[serde(default)]
    name: String,
    /// YYYY-MM → counters. Older files used daily YYYY-MM-DD under `days`;
    /// those keys are folded into months on load.
    #[serde(default, alias = "days")]
    months: BTreeMap<String, MonthEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrafficFile {
    #[serde(default = "traffic_file_version")]
    version: u32,
    #[serde(default)]
    session_started: u64,
    #[serde(default)]
    session: BTreeMap<String, SessionEntry>,
    #[serde(default)]
    history: BTreeMap<String, NodeHistory>,
    #[serde(default)]
    subscriptions: BTreeMap<String, SubHistory>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionEntry {
    #[serde(default)]
    name: String,
    #[serde(default)]
    up: u64,
    #[serde(default)]
    down: u64,
}

/// One node or the direct bucket, plus the subscription that owned the node.
///
/// `subscription_id` is a hash of the subscription URL. The URL itself is not
/// stored: subscription addresses often contain access tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrafficTarget {
    pub id: String,
    pub name: String,
    pub subscription_id: Option<String>,
    pub subscription_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SubHistory {
    #[serde(default)]
    name: String,
    #[serde(default)]
    months: BTreeMap<String, MonthEntry>,
}

fn traffic_file_version() -> u32 {
    3
}

/// Opaque id for a subscription URL. Safe to persist; not reversible to the URL.
pub fn subscription_public_id(url: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(url.as_bytes()))
}

/// Label safe to store and show. Subscription URLs and query secrets stay out of the ledger.
pub fn public_traffic_label(name: &str, fallback: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() || label_carries_secret(trimmed) {
        return fallback.to_string();
    }
    trimmed.chars().take(80).collect()
}

fn label_carries_secret(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("://")
        || lower.contains("token=")
        || lower.contains("password=")
        || lower.contains("passwd=")
        || lower.contains("secret=")
        || lower.contains("auth=")
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn month_key() -> String {
    let secs = unix_now() as i64;
    let days = secs.div_euclid(86_400);
    let (y, m, _) = civil_from_days(days);
    format!("{y:04}-{m:02}")
}

/// Calendar month keys covering the current month and the previous `keep - 1`
/// months, oldest first (e.g. keep=3 → `[YYYY-MM-2, YYYY-MM-1, YYYY-MM]`).
pub fn recent_month_keys(keep: u32) -> Vec<String> {
    let keep = keep.max(1);
    let (y, m, _) = civil_from_days(unix_now() as i64 / 86_400);
    let mut out = Vec::with_capacity(keep as usize);
    for i in (0..keep).rev() {
        let (cy, cm) = shift_month(y, m, -(i as i32));
        out.push(format!("{cy:04}-{cm:02}"));
    }
    out
}

/// Algorithm from Howard Hinnant's public-domain civil-from-days.
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64 + era * 400) as i32;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

fn shift_month(y: i32, m: u32, delta: i32) -> (i32, u32) {
    let idx = y * 12 + (m as i32 - 1) + delta;
    let ny = idx.div_euclid(12);
    let nm = (idx.rem_euclid(12) + 1) as u32;
    (ny, nm)
}

fn month_prefix(key: &str) -> Option<String> {
    // Accept YYYY-MM or legacy YYYY-MM-DD.
    if key.len() >= 7 && key.as_bytes().get(4) == Some(&b'-') {
        let prefix = &key[..7];
        if prefix.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
            return Some(prefix.to_string());
        }
    }
    None
}

fn fold_daily_into_months(node: &mut NodeHistory) {
    let mut folded: BTreeMap<String, MonthEntry> = BTreeMap::new();
    for (key, entry) in std::mem::take(&mut node.months) {
        let Some(month) = month_prefix(&key) else {
            continue;
        };
        let bucket = folded.entry(month).or_insert(MonthEntry { up: 0, down: 0 });
        bucket.up = bucket.up.saturating_add(entry.up);
        bucket.down = bucket.down.saturating_add(entry.down);
    }
    node.months = folded;
}

fn prune_history(history: &mut BTreeMap<String, NodeHistory>, keep_months: u32) {
    let keep = keep_months.max(1);
    let (y, m, _) = civil_from_days(unix_now() as i64 / 86_400);
    let (cy, cm) = shift_month(y, m, 1 - keep as i32);
    let cutoff = format!("{cy:04}-{cm:02}");
    history.retain(|_, node| {
        fold_daily_into_months(node);
        node.months
            .retain(|month, _| month.as_str() >= cutoff.as_str());
        !node.months.is_empty() || !node.name.is_empty()
    });
}

fn prune_subscriptions(subscriptions: &mut BTreeMap<String, SubHistory>, keep_months: u32) {
    let keep = keep_months.max(1);
    let (y, m, _) = civil_from_days(unix_now() as i64 / 86_400);
    let (cy, cm) = shift_month(y, m, 1 - keep as i32);
    let cutoff = format!("{cy:04}-{cm:02}");
    subscriptions.retain(|_, sub| {
        sub.months
            .retain(|month, _| month.as_str() >= cutoff.as_str());
        !sub.months.is_empty() || !sub.name.is_empty()
    });
}

/// Shared ledger attached to [`crate::ProxyStats`].
pub struct TrafficLedger {
    current_id: Mutex<Option<String>>,
    current_name: Mutex<Option<String>>,
    current_sub_id: Mutex<Option<String>>,
    current_sub_name: Mutex<Option<String>>,
    session: Mutex<BTreeMap<String, SessionEntry>>,
    history: Mutex<BTreeMap<String, NodeHistory>>,
    subscriptions: Mutex<BTreeMap<String, SubHistory>>,
    session_started: u64,
    path: Option<PathBuf>,
    generation: AtomicU64,
    persisted_generation: AtomicU64,
    persist_lock: Mutex<()>,
}

impl std::fmt::Debug for TrafficLedger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrafficLedger")
            .field("session_started", &self.session_started)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl TrafficLedger {
    /// Load durable history from `path` (if present) and start a fresh session.
    pub fn load_or_new(path: impl Into<PathBuf>) -> Arc<Self> {
        let path = path.into();
        let mut history = BTreeMap::new();
        let mut subscriptions = BTreeMap::new();
        if let Ok(text) = std::fs::read_to_string(&path)
            && let Ok(mut file) = serde_yaml::from_str::<TrafficFile>(&text)
        {
            history = file.history;
            prune_history(&mut history, TRAFFIC_HISTORY_MONTHS);
            prune_subscriptions(&mut file.subscriptions, TRAFFIC_HISTORY_MONTHS);
            subscriptions = file.subscriptions;
        }
        let ledger = Arc::new(Self {
            current_id: Mutex::new(None),
            current_name: Mutex::new(None),
            current_sub_id: Mutex::new(None),
            current_sub_name: Mutex::new(None),
            session: Mutex::new(BTreeMap::new()),
            history: Mutex::new(history),
            subscriptions: Mutex::new(subscriptions),
            session_started: unix_now(),
            path: Some(path),
            generation: AtomicU64::new(1),
            persisted_generation: AtomicU64::new(0),
            persist_lock: Mutex::new(()),
        });
        // Replace the previous process session immediately so readers do not
        // keep showing the last run until the next byte sample.
        if let Err(error) = ledger.flush() {
            tracing::debug!("traffic ledger startup flush failed: {error:#}");
        }
        let weak = Arc::downgrade(&ledger);
        if let Err(error) = std::thread::Builder::new()
            .name("sockrocket-traffic-save".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(PERSIST_INTERVAL);
                    let Some(ledger) = weak.upgrade() else { break };
                    if ledger.generation.load(Ordering::Acquire)
                        != ledger.persisted_generation.load(Ordering::Acquire)
                        && let Err(error) = ledger.flush()
                    {
                        tracing::debug!("traffic ledger persist skipped/failed: {error:#}");
                    }
                }
            })
        {
            tracing::warn!("traffic ledger background writer unavailable: {error}");
        }
        ledger
    }

    /// In-memory ledger only (tests / short-lived tools).
    pub fn memory_only() -> Arc<Self> {
        Arc::new(Self {
            current_id: Mutex::new(None),
            current_name: Mutex::new(None),
            current_sub_id: Mutex::new(None),
            current_sub_name: Mutex::new(None),
            session: Mutex::new(BTreeMap::new()),
            history: Mutex::new(BTreeMap::new()),
            subscriptions: Mutex::new(BTreeMap::new()),
            session_started: unix_now(),
            path: None,
            generation: AtomicU64::new(0),
            persisted_generation: AtomicU64::new(0),
            persist_lock: Mutex::new(()),
        })
    }

    /// Point subsequent byte samples at this node. Pass `None` for Direct.
    pub fn set_current_node(&self, id: Option<String>, name: Option<String>) {
        let id = id.filter(|value| !value.is_empty());
        let name = name.filter(|value| !value.is_empty());
        if let Ok(mut slot) = self.current_id.lock() {
            *slot = id.or_else(|| Some(DIRECT_TRAFFIC_ID.to_string()));
        }
        if let Ok(mut slot) = self.current_name.lock() {
            *slot = name
                .map(|value| public_traffic_label(&value, "节点"))
                .or_else(|| Some("直连".to_string()));
        }
        // The previous node's subscription must not follow a different node.
        self.set_current_subscription(None, None);
    }

    /// Attach the subscription that owns the current node. `None` clears it.
    pub fn set_current_subscription(&self, id: Option<String>, name: Option<String>) {
        let id = id.filter(|value| !value.is_empty());
        let name = name.filter(|value| !value.is_empty());
        if let Ok(mut slot) = self.current_sub_id.lock() {
            *slot = id;
        }
        if let Ok(mut slot) = self.current_sub_name.lock() {
            *slot = name;
        }
    }

    /// Accumulate bytes against the current node (session + this month's history).
    pub fn record(&self, up: u64, down: u64) {
        let id = self
            .current_id
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
            .unwrap_or_else(|| DIRECT_TRAFFIC_ID.to_string());
        let name = self
            .current_name
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
            .unwrap_or_else(|| "直连".to_string());
        let sub_id = self
            .current_sub_id
            .lock()
            .ok()
            .and_then(|guard| guard.clone());
        let sub_name = self
            .current_sub_name
            .lock()
            .ok()
            .and_then(|guard| guard.clone());
        self.record_parts(&id, &name, sub_id.as_deref(), sub_name.as_deref(), up, down);
    }

    /// Accumulate bytes for a specific routed node, including its subscription.
    pub fn record_target(&self, target: &TrafficTarget, up: u64, down: u64) {
        self.record_parts(
            &target.id,
            &target.name,
            target.subscription_id.as_deref(),
            target.subscription_name.as_deref(),
            up,
            down,
        );
    }

    fn record_parts(
        &self,
        id: &str,
        name: &str,
        sub_id: Option<&str>,
        sub_name: Option<&str>,
        up: u64,
        down: u64,
    ) {
        if up == 0 && down == 0 {
            return;
        }
        let id = if id.is_empty() { DIRECT_TRAFFIC_ID } else { id };
        let name = public_traffic_label(
            name,
            if id == DIRECT_TRAFFIC_ID {
                "直连"
            } else {
                "节点"
            },
        );
        let sub_name = sub_name.map(|value| public_traffic_label(value, "订阅"));
        if let Ok(mut session) = self.session.lock() {
            let entry = session
                .entry(id.to_string())
                .or_insert_with(|| SessionEntry {
                    name: name.to_string(),
                    up: 0,
                    down: 0,
                });
            entry.name = name.to_string();
            entry.up = entry.up.saturating_add(up);
            entry.down = entry.down.saturating_add(down);
        }
        let month = month_key();
        if let Ok(mut history) = self.history.lock() {
            let node = history
                .entry(id.to_string())
                .or_insert_with(|| NodeHistory {
                    name: name.to_string(),
                    months: BTreeMap::new(),
                });
            node.name = name.to_string();
            let bucket = node
                .months
                .entry(month.clone())
                .or_insert(MonthEntry { up: 0, down: 0 });
            bucket.up = bucket.up.saturating_add(up);
            bucket.down = bucket.down.saturating_add(down);
        }
        if let Some(sub_id) = sub_id.filter(|value| !value.is_empty())
            && let Ok(mut subscriptions) = self.subscriptions.lock()
        {
            let sub = subscriptions
                .entry(sub_id.to_string())
                .or_insert_with(|| SubHistory {
                    name: sub_name.clone().unwrap_or_else(|| "订阅".to_string()),
                    months: BTreeMap::new(),
                });
            if let Some(sub_name) = sub_name.filter(|value| !value.is_empty()) {
                sub.name = sub_name;
            }
            let bucket = sub
                .months
                .entry(month)
                .or_insert(MonthEntry { up: 0, down: 0 });
            bucket.up = bucket.up.saturating_add(up);
            bucket.down = bucket.down.saturating_add(down);
        }
        self.generation.fetch_add(1, Ordering::Release);
    }

    pub fn session_started(&self) -> u64 {
        self.session_started
    }

    /// Per-node snapshot: this process session plus retained history totals.
    pub fn snapshots(&self) -> Vec<NodeTrafficSnapshot> {
        let session = self.session.lock().map(|g| g.clone()).unwrap_or_default();
        let history = self.history.lock().map(|g| g.clone()).unwrap_or_default();
        let mut ids: BTreeMap<String, String> = BTreeMap::new();
        for (id, entry) in &session {
            ids.insert(id.clone(), entry.name.clone());
        }
        for (id, entry) in &history {
            ids.entry(id.clone()).or_insert_with(|| entry.name.clone());
        }
        let mut out = Vec::with_capacity(ids.len());
        for (id, name) in ids {
            let session_counters = session
                .get(&id)
                .map(|e| TrafficCounters {
                    up: e.up,
                    down: e.down,
                })
                .unwrap_or_default();
            let mut history_counters = TrafficCounters::default();
            if let Some(node) = history.get(&id) {
                for month in node.months.values() {
                    history_counters.add(month.up, month.down);
                }
            }
            if session_counters.total() == 0 && history_counters.total() == 0 {
                continue;
            }
            out.push(NodeTrafficSnapshot {
                id,
                name,
                session: session_counters,
                history: history_counters,
            });
        }
        out.sort_by(|a, b| {
            b.session
                .total()
                .cmp(&a.session.total())
                .then_with(|| b.history.total().cmp(&a.history.total()))
                .then_with(|| a.name.cmp(&b.name))
        });
        out
    }

    /// Monthly series for one node, oldest first, within the retention window.
    pub fn monthly_series(&self, id: &str) -> Vec<(String, TrafficCounters)> {
        let Ok(history) = self.history.lock() else {
            return Vec::new();
        };
        let Some(node) = history.get(id) else {
            return Vec::new();
        };
        node.months
            .iter()
            .map(|(month, entry)| {
                (
                    month.clone(),
                    TrafficCounters {
                        up: entry.up,
                        down: entry.down,
                    },
                )
            })
            .collect()
    }

    pub fn flush(&self) -> Result<()> {
        let Some(path) = self.path.as_ref() else {
            return Ok(());
        };
        let _persist = self.persist_lock.lock().unwrap_or_else(|e| e.into_inner());
        let generation = self.generation.load(Ordering::Acquire);
        let session = self.session.lock().map(|g| g.clone()).unwrap_or_default();
        let mut history = self.history.lock().map(|g| g.clone()).unwrap_or_default();
        let mut subscriptions = self
            .subscriptions
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default();
        prune_history(&mut history, TRAFFIC_HISTORY_MONTHS);
        prune_subscriptions(&mut subscriptions, TRAFFIC_HISTORY_MONTHS);
        let file = TrafficFile {
            version: traffic_file_version(),
            session_started: self.session_started,
            session,
            history,
            subscriptions,
        };
        let text = serde_yaml::to_string(&file)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::config::storage::atomic_write_private(path, &text, true)?;
        self.persisted_generation
            .store(generation, Ordering::Release);
        Ok(())
    }

    /// Read a previously written file without starting a live session (CGI).
    pub fn read_file(path: &Path) -> Result<Vec<NodeTrafficSnapshot>> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut file: TrafficFile = serde_yaml::from_str(&text)?;
        prune_history(&mut file.history, TRAFFIC_HISTORY_MONTHS);
        let mut ids: BTreeMap<String, String> = BTreeMap::new();
        for (id, entry) in &file.session {
            ids.insert(id.clone(), entry.name.clone());
        }
        for (id, entry) in &file.history {
            ids.entry(id.clone()).or_insert_with(|| entry.name.clone());
        }
        let mut out = Vec::new();
        for (id, name) in ids {
            let session = file
                .session
                .get(&id)
                .map(|e| TrafficCounters {
                    up: e.up,
                    down: e.down,
                })
                .unwrap_or_default();
            let mut history = TrafficCounters::default();
            if let Some(node) = file.history.get(&id) {
                for month in node.months.values() {
                    history.add(month.up, month.down);
                }
            }
            if session.total() == 0 && history.total() == 0 {
                continue;
            }
            out.push(NodeTrafficSnapshot {
                id,
                name,
                session,
                history,
            });
        }
        out.sort_by(|a, b| {
            b.session
                .total()
                .cmp(&a.session.total())
                .then_with(|| b.history.total().cmp(&a.history.total()))
        });
        Ok(out)
    }

    pub fn read_session_started(path: &Path) -> Option<u64> {
        let text = std::fs::read_to_string(path).ok()?;
        let file: TrafficFile = serde_yaml::from_str(&text).ok()?;
        (file.session_started > 0).then_some(file.session_started)
    }

    /// Per-node monthly totals for the last `keep_months` calendar months.
    ///
    /// Empty months are included so UIs can render a fixed column set. Nodes
    /// with zero bytes across the window are omitted.
    pub fn read_recent_months(path: &Path, keep_months: u32) -> Result<Vec<NodeMonthlyTraffic>> {
        let keys = recent_month_keys(keep_months);
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut file: TrafficFile = serde_yaml::from_str(&text)?;
        prune_history(&mut file.history, TRAFFIC_HISTORY_MONTHS);
        let mut out = Vec::new();
        for (id, node) in file.history {
            let mut total = TrafficCounters::default();
            let mut months = Vec::with_capacity(keys.len());
            for key in &keys {
                let counters = node
                    .months
                    .get(key)
                    .map(|e| TrafficCounters {
                        up: e.up,
                        down: e.down,
                    })
                    .unwrap_or_default();
                total.add(counters.up, counters.down);
                months.push((key.clone(), counters));
            }
            if total.total() == 0 {
                continue;
            }
            out.push(NodeMonthlyTraffic {
                id,
                name: node.name,
                months,
                total,
            });
        }
        out.sort_by(|a, b| {
            b.total
                .total()
                .cmp(&a.total.total())
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(out)
    }

    /// Per-subscription monthly totals for the last `keep_months` calendar months.
    pub fn read_recent_subscriptions(
        path: &Path,
        keep_months: u32,
    ) -> Result<Vec<NodeMonthlyTraffic>> {
        let keys = recent_month_keys(keep_months);
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut file: TrafficFile = serde_yaml::from_str(&text)?;
        prune_subscriptions(&mut file.subscriptions, TRAFFIC_HISTORY_MONTHS);
        let mut out = Vec::new();
        for (id, sub) in file.subscriptions {
            let mut total = TrafficCounters::default();
            let mut months = Vec::with_capacity(keys.len());
            for key in &keys {
                let counters = sub
                    .months
                    .get(key)
                    .map(|e| TrafficCounters {
                        up: e.up,
                        down: e.down,
                    })
                    .unwrap_or_default();
                total.add(counters.up, counters.down);
                months.push((key.clone(), counters));
            }
            if total.total() == 0 {
                continue;
            }
            out.push(NodeMonthlyTraffic {
                id,
                name: sub.name,
                months,
                total,
            });
        }
        out.sort_by(|a, b| {
            b.total
                .total()
                .cmp(&a.total.total())
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(out)
    }
}

/// Add each node's monthly totals into the subscription that currently owns it.
///
/// The side ledger can miss bytes recorded before a node was tied to a
/// subscription. The subscriptions page promises the sum of member nodes, so
/// that table is derived from node history plus the current config.
///
/// `membership` is `(node_public_id, subscription_public_id)`.
/// `subscription_names` is `(subscription_public_id, display_name)`.
/// Direct and nodes that do not belong to a listed subscription are omitted.
pub fn sum_subscription_members(
    nodes: &[NodeMonthlyTraffic],
    membership: &[(String, String)],
    subscription_names: &[(String, String)],
) -> Vec<NodeMonthlyTraffic> {
    let names: BTreeMap<&str, &str> = subscription_names
        .iter()
        .map(|(id, name)| (id.as_str(), name.as_str()))
        .collect();
    let owners: BTreeMap<&str, &str> = membership
        .iter()
        .filter(|(_, sub_id)| names.contains_key(sub_id.as_str()))
        .map(|(node_id, sub_id)| (node_id.as_str(), sub_id.as_str()))
        .collect();
    let mut totals: BTreeMap<&str, BTreeMap<String, TrafficCounters>> = BTreeMap::new();
    for node in nodes {
        let Some(sub_id) = owners.get(node.id.as_str()).copied() else {
            continue;
        };
        let months = totals.entry(sub_id).or_default();
        for (key, counters) in &node.months {
            months
                .entry(key.clone())
                .or_default()
                .add(counters.up, counters.down);
        }
    }
    let mut out = Vec::new();
    for (sub_id, months) in totals {
        let mut total = TrafficCounters::default();
        let months: Vec<(String, TrafficCounters)> = months
            .into_iter()
            .map(|(key, counters)| {
                total.add(counters.up, counters.down);
                (key, counters)
            })
            .collect();
        if total.total() == 0 {
            continue;
        }
        let name = public_traffic_label(names.get(sub_id).copied().unwrap_or(""), "订阅");
        out.push(NodeMonthlyTraffic {
            id: sub_id.to_string(),
            name,
            months,
            total,
        });
    }
    out.sort_by(|a, b| {
        b.total
            .total()
            .cmp(&a.total.total())
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

impl Drop for TrafficLedger {
    fn drop(&mut self) {
        if self.generation.load(Ordering::Acquire)
            != self.persisted_generation.load(Ordering::Acquire)
        {
            let _ = self.flush();
        }
    }
}

tokio::task_local! {
    static TRAFFIC_SLOT: std::sync::Arc<std::sync::Mutex<Option<TrafficTarget>>>;
}

struct TrafficCatalog {
    nodes: std::collections::HashMap<String, TrafficTarget>,
    active: Option<TrafficTarget>,
}

fn traffic_catalog() -> &'static std::sync::RwLock<TrafficCatalog> {
    static CATALOG: std::sync::OnceLock<std::sync::RwLock<TrafficCatalog>> =
        std::sync::OnceLock::new();
    CATALOG.get_or_init(|| {
        std::sync::RwLock::new(TrafficCatalog {
            nodes: std::collections::HashMap::new(),
            active: None,
        })
    })
}

/// Remember node and subscription labels for the running config.
pub fn publish_traffic_catalog(
    nodes: &[crate::config::model::Node],
    subscriptions: &[crate::config::model::Subscription],
    active_index: Option<usize>,
    direct_mode: bool,
) {
    let names: std::collections::HashMap<&str, &str> = subscriptions
        .iter()
        .map(|sub| (sub.url.as_str(), sub.name.as_str()))
        .collect();
    let mut built = std::collections::HashMap::new();
    for node in nodes {
        let mut target = target_for_node(node);
        if let Some(url) = node
            .extra
            .get(crate::SUB_URL_EXTRA_KEY)
            .map(String::as_str)
            .filter(|url| !url.is_empty())
        {
            target.subscription_name = names
                .get(url)
                .map(|name| (*name).to_string())
                .filter(|name| !name.is_empty())
                .or(target.subscription_name);
        }
        built.insert(target.id.clone(), target);
    }
    let active = if direct_mode {
        None
    } else {
        active_index
            .and_then(|index| nodes.get(index))
            .and_then(|node| {
                let id = crate::node_public_id(&crate::node_fingerprint(node));
                built.get(&id).cloned()
            })
    };
    if let Ok(mut catalog) = traffic_catalog().write() {
        catalog.nodes = built;
        catalog.active = active;
    }
}

/// Point the fallback (global / unscoped) bucket at a catalogued node.
pub fn set_catalog_active(public_id: &str, name: &str) -> TrafficTarget {
    let target = traffic_catalog()
        .read()
        .ok()
        .and_then(|catalog| catalog.nodes.get(public_id).cloned())
        .unwrap_or(TrafficTarget {
            id: public_id.to_string(),
            name: name.to_string(),
            subscription_id: None,
            subscription_name: None,
        });
    if let Ok(mut catalog) = traffic_catalog().write() {
        let stored = catalog
            .nodes
            .get(public_id)
            .cloned()
            .unwrap_or_else(|| target.clone());
        catalog.active = Some(stored.clone());
        return stored;
    }
    target
}

pub fn active_traffic_target() -> Option<TrafficTarget> {
    traffic_catalog()
        .read()
        .ok()
        .and_then(|catalog| catalog.active.clone())
}

pub fn direct_traffic_target() -> TrafficTarget {
    TrafficTarget {
        id: DIRECT_TRAFFIC_ID.to_string(),
        name: "直连".to_string(),
        subscription_id: None,
        subscription_name: None,
    }
}

pub fn target_for_node(node: &crate::config::model::Node) -> TrafficTarget {
    let url = node
        .extra
        .get(crate::SUB_URL_EXTRA_KEY)
        .map(String::as_str)
        .filter(|url| !url.is_empty());
    TrafficTarget {
        id: crate::node_public_id(&crate::node_fingerprint(node)),
        name: node.name.clone(),
        subscription_id: url.map(subscription_public_id),
        subscription_name: None,
    }
}

pub fn note_node(node: &crate::config::model::Node) {
    let id = crate::node_public_id(&crate::node_fingerprint(node));
    let target = traffic_catalog()
        .read()
        .ok()
        .and_then(|catalog| catalog.nodes.get(&id).cloned())
        .unwrap_or_else(|| target_for_node(node));
    note_traffic(target);
}

pub fn note_traffic(target: TrafficTarget) {
    let _ = TRAFFIC_SLOT.try_with(|slot| {
        if let Ok(mut guard) = slot.lock() {
            *guard = Some(target);
        }
    });
}

pub fn current_traffic_target() -> Option<TrafficTarget> {
    TRAFFIC_SLOT
        .try_with(|slot| slot.lock().ok().and_then(|guard| guard.clone()))
        .ok()
        .flatten()
}

// Avoid another async frame around the already scoped future.
pub fn with_traffic_scope<F: std::future::Future>(
    future: F,
) -> impl std::future::Future<Output = F::Output> {
    TRAFFIC_SLOT.scope(std::sync::Arc::new(std::sync::Mutex::new(None)), future)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_traffic_does_not_write_on_the_proxy_path() {
        let dir = std::env::temp_dir().join(format!("sockrocket-traffic-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("traffic.yaml");
        let ledger = TrafficLedger::load_or_new(&path);
        let before = std::fs::read(&path).unwrap();
        ledger.record(25, 75);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        ledger.flush().unwrap();
        assert_eq!(
            TrafficLedger::read_file(&path).unwrap()[0].history.total(),
            100
        );
        drop(ledger);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn records_session_and_history_then_survives_reopen() {
        let dir = std::env::temp_dir().join(format!("sockrocket-traffic-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("traffic.yaml");
        let ledger = TrafficLedger::load_or_new(&path);
        ledger.set_current_node(Some("abc123".into()), Some("Hong Kong".into()));
        ledger.record(100, 400);
        ledger.set_current_node(Some("def456".into()), Some("Tokyo".into()));
        ledger.record(50, 50);
        ledger.flush().unwrap();

        let snapshots = ledger.snapshots();
        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].id, "abc123");
        assert_eq!(snapshots[0].session.total(), 500);
        assert_eq!(snapshots[0].history.total(), 500);

        let series = ledger.monthly_series("abc123");
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].0, month_key());
        assert_eq!(series[0].1.total(), 500);

        let reopened = TrafficLedger::load_or_new(&path);
        // Fresh process: session empty, history kept.
        let again = reopened.snapshots();
        assert_eq!(again.len(), 2);
        assert_eq!(
            again
                .iter()
                .find(|s| s.id == "abc123")
                .unwrap()
                .session
                .total(),
            0
        );
        assert_eq!(
            again
                .iter()
                .find(|s| s.id == "abc123")
                .unwrap()
                .history
                .total(),
            500
        );
        assert_eq!(
            again
                .iter()
                .find(|s| s.id == "def456")
                .unwrap()
                .history
                .total(),
            100
        );

        let file_view = TrafficLedger::read_file(&path).unwrap();
        assert_eq!(file_view.len(), 2);

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn prunes_months_older_than_retention() {
        let ledger = TrafficLedger::memory_only();
        {
            let mut history = ledger.history.lock().unwrap();
            history.insert(
                "n1".into(),
                NodeHistory {
                    name: "Old".into(),
                    months: BTreeMap::from([
                        ("2000-01".into(), MonthEntry { up: 9, down: 9 }),
                        (month_key(), MonthEntry { up: 1, down: 2 }),
                    ]),
                },
            );
            prune_history(&mut history, TRAFFIC_HISTORY_MONTHS);
            assert!(!history["n1"].months.contains_key("2000-01"));
            assert!(history["n1"].months.contains_key(&month_key()));
        }
    }

    #[test]
    fn folds_legacy_daily_keys_into_months() {
        let ledger = TrafficLedger::memory_only();
        {
            let mut history = ledger.history.lock().unwrap();
            let month = month_key();
            history.insert(
                "n1".into(),
                NodeHistory {
                    name: "Legacy".into(),
                    months: BTreeMap::from([
                        (format!("{month}-01"), MonthEntry { up: 10, down: 20 }),
                        (format!("{month}-15"), MonthEntry { up: 5, down: 5 }),
                        ("1999-12-31".into(), MonthEntry { up: 1, down: 1 }),
                    ]),
                },
            );
            prune_history(&mut history, TRAFFIC_HISTORY_MONTHS);
            assert_eq!(history["n1"].months.len(), 1);
            let bucket = &history["n1"].months[&month];
            assert_eq!(bucket.up, 15);
            assert_eq!(bucket.down, 25);
        }
    }

    #[test]
    fn direct_bucket_used_when_no_node_selected() {
        let ledger = TrafficLedger::memory_only();
        ledger.set_current_node(None, None);
        ledger.record(10, 20);
        let snaps = ledger.snapshots();
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].id, DIRECT_TRAFFIC_ID);
        assert_eq!(snaps[0].session.total(), 30);
    }

    #[test]
    fn recent_months_window_is_oldest_first() {
        let keys = recent_month_keys(3);
        assert_eq!(keys.len(), 3);
        assert_eq!(keys[2], month_key());
        assert!(keys[0] < keys[1] && keys[1] < keys[2]);
    }

    #[test]
    fn read_recent_months_sums_last_three() {
        let dir =
            std::env::temp_dir().join(format!("sockrocket-traffic-m-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("traffic.yaml");
        let ledger = TrafficLedger::load_or_new(&path);
        ledger.set_current_node(Some("n1".into()), Some("Node One".into()));
        ledger.record(100, 200);
        {
            let mut history = ledger.history.lock().unwrap();
            let keys = recent_month_keys(3);
            let node = history.get_mut("n1").unwrap();
            node.months
                .insert(keys[0].clone(), MonthEntry { up: 10, down: 20 });
            node.months
                .insert(keys[1].clone(), MonthEntry { up: 30, down: 40 });
        }
        ledger.flush().unwrap();
        let rows = TrafficLedger::read_recent_months(&path, 3).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].months.len(), 3);
        assert_eq!(rows[0].months[0].1.total(), 30);
        assert_eq!(rows[0].months[1].1.total(), 70);
        assert_eq!(rows[0].months[2].1.total(), 300);
        assert_eq!(rows[0].total.total(), 400);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn subscription_table_sums_current_members_and_skips_direct() {
        let node = |id: &str, up: u64, down: u64| NodeMonthlyTraffic {
            id: id.into(),
            name: id.into(),
            months: vec![("2026-10".into(), TrafficCounters { up, down })],
            total: TrafficCounters { up, down },
        };
        let rows = vec![
            node("alpha", 100, 200),
            node("beta", 5, 5),
            node("direct", 9, 9),
            node("solo", 7, 1),
        ];
        let summed = sum_subscription_members(
            &rows,
            &[
                ("alpha".into(), "sub-a".into()),
                ("beta".into(), "sub-a".into()),
                ("solo".into(), "sub-b".into()),
            ],
            &[
                ("sub-a".into(), "订阅A".into()),
                ("sub-b".into(), "订阅B".into()),
            ],
        );
        assert_eq!(summed.len(), 2);
        assert_eq!(summed[0].name, "订阅A");
        assert_eq!(summed[0].total.total(), 310);
        assert_eq!(summed[1].name, "订阅B");
        assert_eq!(summed[1].total.total(), 8);
        let hidden = sum_subscription_members(
            &rows,
            &[("alpha".into(), "sub-a".into())],
            &[(
                "sub-a".into(),
                "https://provider.example/sub?token=secret".into(),
            )],
        );
        assert_eq!(hidden.len(), 1);
        assert_eq!(hidden[0].name, "订阅");
        assert_eq!(hidden[0].total.total(), 300);
    }

    #[test]
    fn subscription_months_sum_member_nodes_without_the_url() {
        let url = "https://provider.example/sub?token=secret-value";
        let dir =
            std::env::temp_dir().join(format!("sockrocket-traffic-sub-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("traffic.yaml");
        let ledger = TrafficLedger::load_or_new(&path);
        let sub_id = subscription_public_id(url);
        let target = |id: &str, name: &str| TrafficTarget {
            id: id.into(),
            name: name.into(),
            subscription_id: Some(sub_id.clone()),
            subscription_name: Some("Home".into()),
        };
        ledger.record_target(&target("node-a", "Alpha"), 10, 40);
        ledger.record_target(&target("node-b", "Beta"), 5, 5);
        ledger.flush().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("secret-value"));
        assert!(!text.contains(url));
        let subs = TrafficLedger::read_recent_subscriptions(&path, 3).unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].id, sub_id);
        assert_eq!(subs[0].name, "Home");
        assert_eq!(subs[0].total.total(), 60);
        let nodes = TrafficLedger::read_file(&path).unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(
            nodes.iter().map(|node| node.session.total()).sum::<u64>(),
            60
        );
        ledger.record_target(
            &TrafficTarget {
                id: "node-secret".into(),
                name: url.into(),
                subscription_id: Some(sub_id),
                subscription_name: Some(url.into()),
            },
            1,
            1,
        );
        ledger.flush().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("secret-value"));
        assert!(!text.contains("https://"));
        let hidden = TrafficLedger::read_recent_subscriptions(&path, 3).unwrap();
        assert_eq!(hidden[0].name, "订阅");
        let hidden_nodes = TrafficLedger::read_file(&path).unwrap();
        assert!(
            hidden_nodes
                .iter()
                .any(|node| node.id == "node-secret" && node.name == "节点")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}

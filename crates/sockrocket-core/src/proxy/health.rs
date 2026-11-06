//! Periodic health checking of the active proxy node, with optional
//! automatic failover to the lowest-latency reachable node.
//!
//! The monitor snapshots the current [`SwappableOutbound`] and checks its
//! default proxy with HTTPS, independently of domain routing rules. After `failure_threshold`
//! consecutive failures (and with `auto_switch` on) it latency-tests all
//! candidate nodes concurrently and swaps the shared outbound to the best
//! one — the local listeners keep running untouched.
//!
//! On start it runs a short **startup burst** (two probes a few seconds
//! apart) before settling into `interval_secs`, so a dead active node after
//! boot or TUN re-enable fails over within seconds instead of waiting a
//! full interval.
//!
//! The whole mechanism is opt-in via `health_check.enabled` (default off).

use std::sync::Arc;

use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::config::model::{HealthCheckConfig, Node};
use crate::proxy::connector::{SharedOutbound, SwappableOutbound};
use crate::proxy::factory::create_outbound_unwarmed;
use crate::proxy::speedtest::https_health_probe;

/// Timeout for a single health probe of the active node.
const PROBE_TIMEOUT_SECS: u64 = 5;
/// Timeout for testing one candidate node during failover.
const CANDIDATE_TIMEOUT_SECS: u64 = 5;
/// How many probes to run back-to-back at monitor start (boot / restart /
/// TUN re-enable) before settling into `interval_secs`.
const STARTUP_BURST: u32 = 2;
/// Gap between startup-burst probes (seconds). Short enough that a dead
/// active node fails over within a few seconds of start.
const STARTUP_GAP_SECS: u64 = 3;

/// Builds the full outbound for a node, applying the caller's routing/mode
/// policy (rule routing, direct mode, …). Returns `None` when the outbound
/// cannot be constructed.
pub type OutboundFactory = Arc<dyn Fn(&Node) -> Option<SharedOutbound> + Send + Sync>;

/// Everything the monitor needs; a snapshot taken when it is (re)started.
pub struct HealthCheckSetup {
    pub config: HealthCheckConfig,
    /// Candidate nodes for failover.
    pub nodes: Vec<Node>,
    /// Index of the currently active node (informational).
    pub active_node: Option<usize>,
    /// The shared outbound the proxy servers use; failover swaps its inner
    /// connector so listeners stay up.
    pub swappable: Arc<SwappableOutbound>,
    /// Rebuilds a policy-compliant outbound for a given node.
    pub outbound_factory: OutboundFactory,
}

/// Health check progress, for UIs and logs.
#[derive(Debug, Clone, PartialEq)]
pub enum HealthEvent {
    /// Active node probe succeeded.
    ProbeOk { latency_ms: u32 },
    /// Active node probe failed; carries the consecutive failure count.
    ProbeFailed {
        consecutive_failures: u32,
        error: String,
    },
    /// Failover completed: the active outbound now points at this node.
    AutoSwitched {
        node_index: usize,
        node_name: String,
        node_key: String,
        latency_ms: u32,
    },
    /// Failover was attempted but no candidate node was reachable.
    SwitchFailed { reason: String },
}

/// Counts consecutive probe failures and decides when to fail over.
struct FailureTracker {
    threshold: u32,
    consecutive: u32,
    candidate_cursor: usize,
    current: Option<usize>,
    avoided: std::collections::HashMap<usize, tokio::time::Instant>,
    recent: Vec<usize>,
    last_outbound: Option<SharedOutbound>,
}

impl FailureTracker {
    fn new(threshold: u32) -> Self {
        Self {
            threshold: threshold.max(1),
            consecutive: 0,
            candidate_cursor: 0,
            current: None,
            avoided: Default::default(),
            recent: vec![],
            last_outbound: None,
        }
    }

    fn record_success(&mut self) {
        self.consecutive = 0;
    }

    /// Returns `true` once the failure threshold is reached.
    fn record_failure(&mut self) -> bool {
        self.consecutive = self.consecutive.saturating_add(1);
        self.consecutive >= self.threshold
    }

    fn reset(&mut self) {
        self.consecutive = 0;
    }
}

/// Running health check task. Stops cleanly on [`HealthMonitor::stop`] or drop.
///
/// The events channel is owned by [`ProxyService`](super::service::ProxyService)
/// and passed in, NOT created per monitor: monitor restarts (config reloads)
/// must not strand subscribers — a fresh channel per spawn would drop the old
/// sender and silently kill every existing receiver (observed in the field:
/// a config reload at runtime froze sockrocket_health.json updates for hours while
/// probing continued).
pub struct HealthMonitor {
    shutdown_tx: Option<watch::Sender<bool>>,
    handle: Option<JoinHandle<()>>,
}

impl HealthMonitor {
    /// Spawn the monitor loop. Does nothing by itself when
    /// `setup.config.enabled` is false — callers should check beforehand.
    pub fn spawn(setup: HealthCheckSetup, events_tx: watch::Sender<Option<HealthEvent>>) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = tokio::spawn(run(setup, shutdown_rx, events_tx));
        Self {
            shutdown_tx: Some(shutdown_tx),
            handle: Some(handle),
        }
    }

    /// Stop the monitor, aborting any in-flight probe with a short deadline.
    pub async fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        if let Some(h) = self.handle.take() {
            h.abort();
            let _ = tokio::time::timeout(std::time::Duration::from_millis(500), h).await;
        }
    }
}

impl Drop for HealthMonitor {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        if let Some(h) = self.handle.take() {
            h.abort();
        }
    }
}

// Retry only failed checks, and keep the whole operation cancellation-safe.
async fn confirm_probe<F, Fut>(confirm: bool, mut probe: F) -> anyhow::Result<u32>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<u32>>,
{
    let first = probe().await;
    if first.is_ok() || !confirm {
        return first;
    }
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    probe().await
}

/// Run one probe (+ optional failover). Returns `false` when shutdown was
/// signalled mid-probe so the outer loop can exit cleanly.
async fn probe_once(
    setup: &HealthCheckSetup,
    config: &HealthCheckConfig,
    tracker: &mut FailureTracker,
    events_tx: &watch::Sender<Option<HealthEvent>>,
    shutdown_rx: &mut watch::Receiver<bool>,
) -> bool {
    let snapshot = setup.swappable.current();
    if tracker
        .last_outbound
        .as_ref()
        .is_some_and(|old| !Arc::ptr_eq(&old.0, &snapshot.0))
    {
        // A manual switch owns the decision; do not quarantine the old index.
        tracker.current = None;
        tracker.reset();
    }
    tracker.last_outbound = Some(snapshot.clone());
    let probe_outbound = snapshot
        .0
        .default_proxy()
        .unwrap_or_else(|| snapshot.clone());
    let probe = tokio::select! {
        result = confirm_probe(config.auto_switch, || https_health_probe(&probe_outbound, PROBE_TIMEOUT_SECS)) => result,
        _ = shutdown_rx.changed() => return false,
    };
    if !Arc::ptr_eq(&snapshot.0, &setup.swappable.current().0) {
        return true;
    }
    match probe {
        Ok(ms) => {
            tracker.record_success();
            if let Some(index) = tracker.current {
                tracker.recent.retain(|i| *i != index);
                tracker.recent.insert(0, index);
                tracker.recent.truncate(4);
            }
            let _ = events_tx.send(Some(HealthEvent::ProbeOk { latency_ms: ms }));
        }
        Err(e) => {
            let should_switch = tracker.record_failure();
            let consecutive = tracker.consecutive;
            tracing::warn!(
                "health probe failed ({}/{}): {:#}",
                consecutive,
                tracker.threshold,
                e
            );
            let _ = events_tx.send(Some(HealthEvent::ProbeFailed {
                consecutive_failures: consecutive,
                error: format!("{:#}", e),
            }));

            if !(should_switch && config.auto_switch) {
                return true;
            }
            // Reset so a failed failover does not re-trigger on every tick.
            tracker.reset();
            let now = tokio::time::Instant::now();
            tracker.avoided.retain(|_, until| *until > now);
            if let Some(index) = tracker.current {
                tracker
                    .avoided
                    .insert(index, now + std::time::Duration::from_secs(120));
            }

            let best = tokio::select! {
                best = tokio::time::timeout(std::time::Duration::from_secs(30), find_replacement(&setup.nodes, tracker)) => best.ok().flatten(),
                _ = shutdown_rx.changed() => return false,
            };
            match best {
                Some((index, ms)) => {
                    let node = &setup.nodes[index];
                    match (setup.outbound_factory)(node) {
                        Some(outbound) => {
                            if !Arc::ptr_eq(&snapshot.0, &setup.swappable.current().0) {
                                return true;
                            }
                            if !setup.swappable.set_if_current(&snapshot, outbound.clone()) {
                                return true;
                            }
                            tracker.last_outbound = Some(outbound);
                            tracker.current = Some(index);
                            tracker.recent.retain(|i| *i != index);
                            tracker.recent.insert(0, index);
                            tracker.recent.truncate(4);
                            tracing::info!(
                                "health check: auto-switched to node index {} ({} ms)",
                                index,
                                ms
                            );
                            let _ = events_tx.send(Some(HealthEvent::AutoSwitched {
                                node_index: index,
                                node_name: node.name.clone(),
                                node_key: crate::config::dedup::node_fingerprint(node),
                                latency_ms: ms,
                            }));
                        }
                        None => {
                            let _ = events_tx.send(Some(HealthEvent::SwitchFailed {
                                reason: format!("failed to build outbound for '{}'", node.name),
                            }));
                        }
                    }
                }
                None => {
                    tracing::warn!(
                        "health check: failover found no reachable node within this round's probe budget"
                    );
                    let _ = events_tx.send(Some(HealthEvent::SwitchFailed {
                        reason: "no reachable node within this round's probe budget".to_string(),
                    }));
                }
            }
        }
    }
    true
}

async fn run(
    setup: HealthCheckSetup,
    mut shutdown_rx: watch::Receiver<bool>,
    events_tx: watch::Sender<Option<HealthEvent>>,
) {
    let config = setup.config.clone();
    let interval_secs = config.interval_secs.clamp(5, 3600);

    let mut tracker = FailureTracker::new(config.failure_threshold);
    tracker.current = setup.active_node;

    tracing::info!(
        "health check started: startup burst {}×{}s then every {}s, {} consecutive failure(s) trigger failover (auto_switch={})",
        STARTUP_BURST,
        STARTUP_GAP_SECS,
        interval_secs,
        tracker.threshold,
        config.auto_switch
    );

    // Startup burst: probe immediately, then once more after a short gap.
    // Catches a dead active_node within seconds of boot / TUN re-enable
    // instead of waiting a full interval_secs for the second sample.
    for i in 0..STARTUP_BURST {
        if i > 0 {
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(STARTUP_GAP_SECS)) => {}
                _ = shutdown_rx.changed() => {
                    tracing::info!("health check stopped");
                    return;
                }
            }
        }
        if !probe_once(&setup, &config, &mut tracker, &events_tx, &mut shutdown_rx).await {
            tracing::info!("health check stopped");
            return;
        }
    }

    let mut timer = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Consume the immediate first tick — the burst already covered "now".
    timer.tick().await;

    loop {
        tokio::select! {
            _ = timer.tick() => {}
            _ = shutdown_rx.changed() => break,
        }

        if !probe_once(&setup, &config, &mut tracker, &events_tx, &mut shutdown_rx).await {
            break;
        }
    }

    tracing::info!("health check stopped");
}

/// Latency-test all candidate nodes concurrently and return the index and
/// latency of the fastest reachable one.
// Previously successful nodes are revalidated before reuse; no blind switch.
async fn find_replacement(nodes: &[Node], tracker: &mut FailureTracker) -> Option<(usize, u32)> {
    for index in tracker
        .recent
        .iter()
        .copied()
        .filter(|i| !tracker.avoided.contains_key(i))
        .take(1)
        .collect::<Vec<_>>()
    {
        if tracker.avoided.contains_key(&index) {
            continue;
        }
        if let Some(node) = nodes.get(index) {
            if let Ok(outbound) = create_outbound_unwarmed(node) {
                if let Ok(ms) = https_health_probe(&outbound, CANDIDATE_TIMEOUT_SECS).await {
                    return Some((index, ms));
                }
            }
            tracker.avoided.insert(
                index,
                tokio::time::Instant::now() + std::time::Duration::from_secs(120),
            );
        }
    }
    let available = eligible_candidates(nodes, &tracker.avoided);
    let candidates: Vec<_> = available.iter().map(|(_, node)| (*node).clone()).collect();
    let (index, ms) = find_best_node(&candidates, &mut tracker.candidate_cursor).await?;
    Some((available[index].0, ms))
}

fn eligible_candidates<'a>(
    nodes: &'a [Node],
    avoided: &std::collections::HashMap<usize, tokio::time::Instant>,
) -> Vec<(usize, &'a Node)> {
    let excluded: std::collections::HashSet<_> = avoided
        .keys()
        .filter_map(|i| nodes.get(*i))
        .map(crate::config::dedup::node_fingerprint)
        .collect();
    let mut seen = std::collections::HashSet::new();
    let available: Vec<_> = nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| {
            let key = crate::config::dedup::node_fingerprint(node);
            !excluded.contains(&key) && seen.insert(key)
        })
        .collect();
    available
}

async fn find_best_node(nodes: &[Node], cursor: &mut usize) -> Option<(usize, u32)> {
    find_best_node_with(nodes, cursor, |node| async move {
        let outbound = create_outbound_unwarmed(&node).ok()?;
        https_health_probe(&outbound, CANDIDATE_TIMEOUT_SECS)
            .await
            .ok()
    })
    .await
}

async fn find_best_node_with<F, Fut>(
    nodes: &[Node],
    cursor: &mut usize,
    probe: F,
) -> Option<(usize, u32)>
where
    F: Fn(Node) -> Fut,
    Fut: std::future::Future<Output = Option<u32>> + Send + 'static,
{
    let mut set = tokio::task::JoinSet::new();
    let mut ordered: Vec<_> = nodes.iter().cloned().enumerate().collect();
    ordered.sort_by_key(|(_, node)| node.latency_ms.unwrap_or(u32::MAX));
    if ordered.is_empty() {
        return None;
    }
    let count = ordered.len();
    ordered.rotate_left(*cursor % count);
    let mut candidates = ordered.into_iter();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    let mut best: Option<(usize, u32)> = None;
    loop {
        // Keep the number of live probes bounded, not just their socket IO.
        while set.len() < 4 {
            let Some((index, node)) = candidates.next() else {
                break;
            };
            // Advance when scheduled, including probes canceled by the round deadline.
            *cursor = (*cursor + 1) % count;
            let future = probe(node);
            set.spawn(async move { future.await.map(|ms| (index, ms)) });
        }
        let result = tokio::select! {
            result = set.join_next() => result,
            _ = tokio::time::sleep_until(deadline) => { set.abort_all(); break; }
        };
        let Some(result) = result else {
            break;
        };
        if let Ok(Some((index, ms))) = result
            && best.is_none_or(|(_, best_ms)| ms < best_ms)
        {
            best = Some((index, ms));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::ProxyProtocol;

    #[tokio::test(start_paused = true)]
    async fn transient_failure_is_confirmed_without_failover() {
        let mut calls = 0;
        let start = tokio::time::Instant::now();
        let result = confirm_probe(true, || {
            calls += 1;
            std::future::ready(if calls == 1 {
                Err(anyhow::anyhow!("transient"))
            } else {
                Ok(42)
            })
        })
        .await;
        assert_eq!(result.unwrap(), 42);
        assert_eq!(calls, 2);
        assert_eq!(start.elapsed(), std::time::Duration::from_secs(2));
    }

    #[tokio::test(start_paused = true)]
    async fn successful_probe_does_not_add_delay_or_traffic() {
        let mut calls = 0;
        let start = tokio::time::Instant::now();
        assert_eq!(
            confirm_probe(true, || {
                calls += 1;
                std::future::ready(Ok(7))
            })
            .await
            .unwrap(),
            7
        );
        assert_eq!(calls, 1);
        assert_eq!(start.elapsed(), std::time::Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn confirmed_failure_stays_failed() {
        let mut calls = 0;
        assert!(
            confirm_probe(true, || {
                calls += 1;
                std::future::ready(Err(anyhow::anyhow!("down")))
            })
            .await
            .is_err()
        );
        assert_eq!(calls, 2);
    }

    #[test]
    fn stale_failover_cannot_overwrite_manual_switch() {
        let old = SharedOutbound::direct();
        let live = SwappableOutbound::new(old.clone());
        let manual = SharedOutbound::direct();
        live.set(manual.clone());
        assert!(!live.set_if_current(&old, SharedOutbound::direct()));
        assert!(Arc::ptr_eq(&manual.0, &live.current().0));
        assert!(live.set_if_current(&manual, old.clone()));
        assert!(Arc::ptr_eq(&old.0, &live.current().0));
    }

    #[test]
    fn failure_tracker_triggers_at_threshold() {
        let mut tracker = FailureTracker::new(2);
        assert!(!tracker.record_failure());
        assert!(tracker.record_failure());
        tracker.reset();
        assert!(!tracker.record_failure());
        tracker.record_success();
        assert!(!tracker.record_failure());
    }

    #[test]
    fn failure_tracker_threshold_is_at_least_one() {
        let mut tracker = FailureTracker::new(0);
        assert!(tracker.record_failure());
    }

    fn dead_node(name: &str) -> Node {
        Node {
            name: name.to_string(),
            // 127.0.0.1:1 refuses connections immediately — deterministic
            // "unreachable" without relying on external network.
            server: "127.0.0.1".to_string(),
            port: 1,
            protocol: ProxyProtocol::Shadowsocks {
                cipher: "aes-256-gcm".to_string(),
                password: "x".to_string(),
                udp: false,
                shadow_tls: None,
            },
            transport: None,
            latency_ms: None,
            tags: vec![],
            extra: Default::default(),
        }
    }

    #[test]
    fn candidate_filter_excludes_failed_aliases_and_deduplicates() {
        let a = dead_node("a");
        let alias = dead_node("alias");
        let mut b = dead_node("b");
        b.port = 2;
        let nodes = vec![a, alias, b];
        let mut avoided = std::collections::HashMap::new();
        assert_eq!(eligible_candidates(&nodes, &avoided).len(), 2);
        avoided.insert(0, tokio::time::Instant::now());
        let candidates = eligible_candidates(&nodes, &avoided);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].0, 2);
    }

    #[tokio::test]
    async fn find_best_node_returns_none_when_all_unreachable() {
        let nodes = vec![dead_node("a"), dead_node("b")];
        assert_eq!(find_best_node(&nodes, &mut 0).await, None);
    }

    #[tokio::test(start_paused = true)]
    async fn bounded_rounds_eventually_reach_candidates_after_silent_prefix() {
        let nodes: Vec<_> = (0..12).map(|i| dead_node(&i.to_string())).collect();
        let mut cursor = 0;
        let probe = |node: Node| async move {
            if node.name == "8" {
                Some(10)
            } else {
                std::future::pending::<Option<u32>>().await
            }
        };
        assert_eq!(find_best_node_with(&nodes, &mut cursor, probe).await, None);
        assert_eq!(cursor, 4);
        assert_eq!(find_best_node_with(&nodes, &mut cursor, probe).await, None);
        assert_eq!(cursor, 8);
        assert_eq!(
            find_best_node_with(&nodes, &mut cursor, probe).await,
            Some((8, 10))
        );
    }

    #[tokio::test]
    async fn monitor_stops_cleanly() {
        let setup = HealthCheckSetup {
            config: HealthCheckConfig {
                enabled: true,
                interval_secs: 3600,
                failure_threshold: 2,
                auto_switch: true,
            },
            nodes: vec![],
            active_node: None,
            swappable: Arc::new(SwappableOutbound::new(SharedOutbound::direct())),
            outbound_factory: Arc::new(|_| None),
        };
        let (events_tx, _) = watch::channel(None);
        let mut monitor = HealthMonitor::spawn(setup, events_tx);
        monitor.stop().await;
        // Second stop is a no-op; drop must not panic either.
        monitor.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn silent_tunnel_triggers_failover_instead_of_reporting_healthy() {
        use crate::proxy::connector::{BoxProxyStream, Outbound};

        struct SilentTunnel;
        impl Outbound for SilentTunnel {
            fn connect(
                &self,
                _host: &str,
                _port: u16,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = anyhow::Result<BoxProxyStream>> + Send + '_>,
            > {
                Box::pin(async {
                    let (client, mut peer) = tokio::io::duplex(1024);
                    tokio::spawn(async move {
                        let _ = tokio::io::copy(&mut peer, &mut tokio::io::sink()).await;
                    });
                    Ok(Box::new(client) as BoxProxyStream)
                })
            }

            fn name(&self) -> &str {
                "silent tunnel"
            }
        }

        let setup = HealthCheckSetup {
            config: HealthCheckConfig {
                enabled: true,
                interval_secs: 60,
                failure_threshold: 1,
                auto_switch: true,
            },
            nodes: vec![],
            active_node: None,
            swappable: Arc::new(SwappableOutbound::new(SharedOutbound(Arc::new(
                SilentTunnel,
            )))),
            outbound_factory: Arc::new(|_| None),
        };
        let (events_tx, mut events_rx) = watch::channel(None);
        let mut monitor = HealthMonitor::spawn(setup, events_tx);
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                events_rx.changed().await.unwrap();
                let event = events_rx.borrow().clone();
                assert!(!matches!(event, Some(HealthEvent::ProbeOk { .. })));
                if matches!(event, Some(HealthEvent::SwitchFailed { .. })) {
                    break;
                }
            }
        })
        .await
        .expect("a silent tunnel must reach failover within the probe deadline");
        monitor.stop().await;
    }

    /// Regression: restarting the monitor (config reload) must NOT strand
    /// existing subscribers. Events emitted by the replacement monitor have
    /// to arrive on the same channel the pre-restart subscriber holds —
    /// this is what keeps sockrocket_health.json updating after a reload.
    #[tokio::test]
    async fn subscribers_survive_monitor_restart() {
        let make_setup = || HealthCheckSetup {
            config: HealthCheckConfig {
                enabled: true,
                interval_secs: 5,
                failure_threshold: 2,
                auto_switch: false,
            },
            nodes: vec![],
            active_node: None,
            swappable: Arc::new(SwappableOutbound::new(SharedOutbound::direct())),
            outbound_factory: Arc::new(|_| None),
        };
        let (events_tx, mut events_rx) = watch::channel(None);
        let mut monitor = HealthMonitor::spawn(make_setup(), events_tx.clone());
        monitor.stop().await;
        let _monitor2 = HealthMonitor::spawn(make_setup(), events_tx);

        let event = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                events_rx.changed().await.ok()?;
                if events_rx.borrow().is_some() {
                    return Some(());
                }
            }
        })
        .await;
        assert!(
            event.ok().flatten().is_some(),
            "subscriber must keep receiving events after a monitor restart"
        );
    }
}

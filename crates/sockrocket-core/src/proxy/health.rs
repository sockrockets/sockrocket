//! Periodic health checking of the active proxy node, with optional
//! automatic failover to the lowest-latency reachable node.
//!
//! The monitor probes through the current [`SwappableOutbound`] using the
//! same HTTP latency test as the speed tester. After `failure_threshold`
//! consecutive failures (and with `auto_switch` on) it latency-tests all
//! candidate nodes concurrently and swaps the shared outbound to the best
//! one — the local listeners keep running untouched.
//!
//! The whole mechanism is opt-in via `health_check.enabled` (default off).

use std::sync::Arc;

use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::config::model::{HealthCheckConfig, Node};
use crate::proxy::connector::{SharedOutbound, SwappableOutbound};
use crate::proxy::factory::create_outbound_unwarmed;
use crate::proxy::speedtest::http_latency_test;

/// Timeout for a single health probe of the active node.
const PROBE_TIMEOUT_SECS: u64 = 5;
/// Timeout for testing one candidate node during failover.
const CANDIDATE_TIMEOUT_SECS: u64 = 5;

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
        latency_ms: u32,
    },
    /// Failover was attempted but no candidate node was reachable.
    SwitchFailed { reason: String },
}

/// Counts consecutive probe failures and decides when to fail over.
struct FailureTracker {
    threshold: u32,
    consecutive: u32,
}

impl FailureTracker {
    fn new(threshold: u32) -> Self {
        Self {
            threshold: threshold.max(1),
            consecutive: 0,
        }
    }

    fn record_success(&mut self) {
        self.consecutive = 0;
    }

    /// Returns `true` once the failure threshold is reached.
    fn record_failure(&mut self) -> bool {
        self.consecutive += 1;
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

async fn run(
    setup: HealthCheckSetup,
    mut shutdown_rx: watch::Receiver<bool>,
    events_tx: watch::Sender<Option<HealthEvent>>,
) {
    let config = setup.config;
    let interval_secs = config.interval_secs.clamp(5, 3600);
    let mut timer = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut tracker = FailureTracker::new(config.failure_threshold);
    // Probing through the swappable always exercises the *current* node.
    let probe_outbound = SharedOutbound(setup.swappable.clone());

    tracing::info!(
        "health check started: every {}s, {} consecutive failure(s) trigger failover (auto_switch={})",
        interval_secs,
        tracker.threshold,
        config.auto_switch
    );

    loop {
        tokio::select! {
            _ = timer.tick() => {}
            _ = shutdown_rx.changed() => break,
        }

        let probe = tokio::select! {
            result = http_latency_test(&probe_outbound, PROBE_TIMEOUT_SECS) => result,
            _ = shutdown_rx.changed() => break,
        };

        match probe {
            Ok(ms) => {
                tracker.record_success();
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
                    continue;
                }
                // Reset so a failed failover does not re-trigger on every tick.
                tracker.reset();

                let best = tokio::select! {
                    best = find_best_node(&setup.nodes) => best,
                    _ = shutdown_rx.changed() => break,
                };
                match best {
                    Some((index, ms)) => {
                        let node = &setup.nodes[index];
                        match (setup.outbound_factory)(node) {
                            Some(outbound) => {
                                setup.swappable.set(outbound);
                                tracing::info!(
                                    "health check: auto-switched to '{}' ({} ms)",
                                    node.name,
                                    ms
                                );
                                let _ = events_tx.send(Some(HealthEvent::AutoSwitched {
                                    node_index: index,
                                    node_name: node.name.clone(),
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
                        tracing::warn!("health check: failover found no reachable node");
                        let _ = events_tx.send(Some(HealthEvent::SwitchFailed {
                            reason: "no reachable node among candidates".to_string(),
                        }));
                    }
                }
            }
        }
    }

    tracing::info!("health check stopped");
}

/// Latency-test all candidate nodes concurrently and return the index and
/// latency of the fastest reachable one.
async fn find_best_node(nodes: &[Node]) -> Option<(usize, u32)> {
    let mut set = tokio::task::JoinSet::new();
    for (index, node) in nodes.iter().enumerate() {
        let node = node.clone();
        set.spawn(async move {
            // Unwarmed: a probe does one latency test and drops the outbound;
            // a warming pool would fire a full round of pre-connect
            // handshakes at every candidate server.
            let outbound = create_outbound_unwarmed(&node).ok()?;
            let ms = http_latency_test(&outbound, CANDIDATE_TIMEOUT_SECS)
                .await
                .ok()?;
            Some((index, ms))
        });
    }

    let mut best: Option<(usize, u32)> = None;
    while let Some(result) = set.join_next().await {
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

    #[tokio::test]
    async fn find_best_node_returns_none_when_all_unreachable() {
        let nodes = vec![dead_node("a"), dead_node("b")];
        assert_eq!(find_best_node(&nodes).await, None);
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

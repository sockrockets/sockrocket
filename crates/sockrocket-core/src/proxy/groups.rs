//! Immutable group snapshots. Connections keep their selected outbound across
//! reloads. Missing groups/members fail closed, never use the global node.
use super::connector::{BoxProxyStream, Outbound, SharedOutbound};
use super::datagram::{BoxDatagram, DatagramFuture};
use crate::config::{
    group::{resolve_members, selected_member},
    model::{GroupType, Node, ProxyGroupConfig},
};
use anyhow::Result;
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, OnceLock},
    time::Duration,
};

struct Member {
    node: Node,
    outbound: OnceLock<Result<SharedOutbound, ()>>,
}
impl Member {
    fn get(&self) -> Result<&SharedOutbound> {
        self.outbound
            .get_or_init(|| super::factory::create_outbound_unwarmed(&self.node).map_err(|_| ()))
            .as_ref()
            .map_err(|_| anyhow::anyhow!("Proxy group member configuration is invalid"))
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum GroupKind {
    Select,
    Fallback,
    UrlTest,
}
const GROUP_PROBE_CONCURRENCY: usize = 4;
const GLOBAL_PROBE_CONCURRENCY: usize = 16;
fn probe_budget() -> &'static Arc<tokio::sync::Semaphore> {
    static BUDGET: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    BUDGET.get_or_init(|| Arc::new(tokio::sync::Semaphore::new(GLOBAL_PROBE_CONCURRENCY)))
}

struct GroupOutbound {
    members: Vec<Arc<Member>>,
    kind: GroupKind,
    /// Index of the member to try first. Fallback keeps the last TCP success
    /// and url-test keeps the fastest recent handshake.
    preferred: std::sync::atomic::AtomicUsize,
    /// A saved `current` is used once, then the runtime pick takes over.
    use_saved: std::sync::atomic::AtomicBool,
    sticky_until: std::sync::Mutex<Option<tokio::time::Instant>>,
    selection: tokio::sync::Mutex<()>,
    generation: std::sync::atomic::AtomicUsize,
    next_probe: std::sync::atomic::AtomicUsize,
    probe_slots: Arc<tokio::sync::Semaphore>,
}
impl GroupOutbound {
    fn remember(&self, index: usize) {
        self.preferred
            .store(index, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut until) = self.sticky_until.lock() {
            *until = Some(tokio::time::Instant::now() + Duration::from_secs(300));
        }
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::Release);
    }
    fn sticky_index(&self) -> Option<usize> {
        let len = self.members.len();
        if len == 0 {
            return None;
        }
        let now = tokio::time::Instant::now();
        let fresh = self
            .sticky_until
            .lock()
            .ok()
            .and_then(|until| *until)
            .is_some_and(|until| until > now);
        if fresh
            || self
                .use_saved
                .swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            Some(self.preferred.load(std::sync::atomic::Ordering::Relaxed) % len)
        } else {
            None
        }
    }
    async fn connect_at(
        &self,
        index: usize,
        host: &str,
        port: u16,
        timeout: Duration,
    ) -> Result<BoxProxyStream> {
        let member = &self.members[index];
        let stream =
            tokio::time::timeout(timeout, async { member.get()?.0.connect(host, port).await })
                .await
                .map_err(|_| anyhow::anyhow!("Proxy group member timed out"))?;
        if stream.is_ok() {
            crate::traffic::note_node(&member.node);
        }
        stream
    }
    async fn connect_fastest(&self, host: &str, port: u16) -> Result<BoxProxyStream> {
        let mut tasks = tokio::task::JoinSet::new();
        let mut launched = 0;
        let mut winner = None;
        loop {
            while tasks.len() < GROUP_PROBE_CONCURRENCY && launched < self.members.len() {
                // Continue after prior cancelled/failed scans instead of always
                // spending the entire deadline on the same unreachable prefix.
                let index = self
                    .next_probe
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    % self.members.len();
                let member = self.members[index].clone();
                let host = host.to_owned();
                launched += 1;
                let slots = self.probe_slots.clone();
                tasks.spawn(async move {
                    let _permit = slots.acquire_owned().await?;
                    let stream = tokio::time::timeout(Duration::from_secs(3), async {
                        member.get()?.0.connect(&host, port).await
                    })
                    .await
                    .map_err(|_| anyhow::anyhow!("Proxy group member timed out"))??;
                    Ok::<_, anyhow::Error>((index, stream))
                });
            }
            match tasks.join_next().await {
                Some(Ok(Ok(found))) => {
                    winner = Some(found);
                    break;
                }
                Some(_) => {}
                None => break,
            }
        }
        drop(tasks);
        let (index, stream) = winner
            .ok_or_else(|| anyhow::anyhow!("Proxy group unavailable: no member connected"))?;
        self.remember(index);
        crate::traffic::note_node(&self.members[index].node);
        Ok(stream)
    }
}
impl Outbound for GroupOutbound {
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<BoxProxyStream>> + Send + '_>>
    {
        let host = host.to_owned();
        Box::pin(async move {
            if self.members.is_empty() {
                anyhow::bail!("Proxy group has no selected member");
            }
            tokio::time::timeout(Duration::from_secs(10), async {
                if self.kind == GroupKind::UrlTest {
                    let generation = self.generation.load(std::sync::atomic::Ordering::Acquire);
                    if let Some(index) = self.sticky_index()
                        && let Ok(stream) = self
                            .connect_at(index, &host, port, Duration::from_secs(3))
                            .await
                    {
                        return Ok(stream);
                    }
                    let selection = self.selection.lock().await;
                    // Other requests reuse the chosen node, but each opens its
                    // own stream for its own destination outside the selection lock.
                    if self.generation.load(std::sync::atomic::Ordering::Acquire) != generation
                        && let Some(index) = self.sticky_index()
                    {
                        drop(selection);
                        return self
                            .connect_at(index, &host, port, Duration::from_secs(3))
                            .await;
                    }
                    return self.connect_fastest(&host, port).await;
                }
                let start = if self.kind == GroupKind::Fallback {
                    self.sticky_index().unwrap_or(0)
                } else {
                    0
                };
                let attempts = if self.kind == GroupKind::Fallback {
                    self.members.len()
                } else {
                    1
                };
                let per_try = if self.kind == GroupKind::Fallback {
                    Duration::from_secs(3)
                } else {
                    Duration::from_secs(10)
                };
                for offset in 0..attempts {
                    let index = (start + offset) % self.members.len();
                    if let Ok(stream) = self.connect_at(index, &host, port, per_try).await {
                        if self.kind == GroupKind::Fallback {
                            self.remember(index);
                        }
                        return Ok(stream);
                    }
                }
                anyhow::bail!(
                    "Proxy group unavailable: selected member missing or connection failed"
                )
            })
            .await
            .map_err(|_| anyhow::anyhow!("Proxy group connection timed out"))?
        })
    }
    fn connect_datagram(&self, host: &str, port: u16) -> DatagramFuture<'_, BoxDatagram> {
        let host = host.to_owned();
        Box::pin(async move {
            // UDP session creation is not a reachability test. Fallback stays
            // on the first member. url-test follows the TCP pick and does not
            // migrate an association that is already open.
            if self.members.is_empty() {
                anyhow::bail!("Proxy group has no selected member");
            }
            let index = if self.kind == GroupKind::UrlTest {
                self.preferred.load(std::sync::atomic::Ordering::Relaxed) % self.members.len()
            } else {
                0
            };
            let member = &self.members[index];
            let datagram = tokio::time::timeout(
                Duration::from_secs(10),
                member.get()?.0.connect_datagram(&host, port),
            )
            .await
            .map_err(|_| anyhow::anyhow!("Proxy group UDP setup timed out"))?;
            if datagram.is_ok() {
                crate::traffic::note_node(&member.node);
            }
            datagram
        })
    }
    fn name(&self) -> &str {
        "proxy-group"
    }
}

struct PinnedOutbound(Arc<Member>);

impl Outbound for PinnedOutbound {
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
        let member = Arc::clone(&self.0);
        let host = host.to_string();
        Box::pin(async move {
            crate::traffic::note_node(&member.node);
            member.get()?.0.connect(&host, port).await
        })
    }

    fn connect_datagram(
        &self,
        host: &str,
        port: u16,
    ) -> super::datagram::DatagramFuture<'_, super::datagram::BoxDatagram> {
        let member = Arc::clone(&self.0);
        let host = host.to_string();
        Box::pin(async move {
            crate::traffic::note_node(&member.node);
            member.get()?.0.connect_datagram(&host, port).await
        })
    }

    fn name(&self) -> &str {
        "pinned-node"
    }
}

/// One outbound per node, keyed by [`crate::node_public_id`].
/// Duplicate fingerprints keep the first node. Missing ids fail closed.
pub fn pinned_nodes(nodes: &[Node]) -> HashMap<String, SharedOutbound> {
    let mut out = HashMap::new();
    for node in nodes {
        let id = crate::node_public_id(&crate::node_fingerprint(node));
        out.entry(id).or_insert_with(|| {
            SharedOutbound(Arc::new(PinnedOutbound(Arc::new(Member {
                node: node.clone(),
                outbound: OnceLock::new(),
            }))))
        });
    }
    out
}

pub fn build_groups(
    groups: &[ProxyGroupConfig],
    nodes: &[Node],
) -> HashMap<String, SharedOutbound> {
    let mut shared = HashMap::<usize, Arc<Member>>::new();
    groups
        .iter()
        .map(|group| {
            let indices = match group.gtype {
                GroupType::Fallback | GroupType::UrlTest => resolve_members(group, nodes),
                GroupType::Select => selected_member(group, nodes).into_iter().collect(),
            };
            let preferred = group
                .current
                .as_ref()
                .and_then(|current| {
                    indices.iter().position(|index| {
                        crate::config::dedup::node_fingerprint(&nodes[*index]) == *current
                    })
                })
                .unwrap_or(0);
            let kind = match group.gtype {
                GroupType::Fallback => GroupKind::Fallback,
                GroupType::UrlTest => GroupKind::UrlTest,
                GroupType::Select => GroupKind::Select,
            };
            let members = indices
                .into_iter()
                .map(|i| {
                    shared
                        .entry(i)
                        .or_insert_with(|| {
                            Arc::new(Member {
                                node: nodes[i].clone(),
                                outbound: OnceLock::new(),
                            })
                        })
                        .clone()
                })
                .collect();
            (
                group.name.clone(),
                SharedOutbound(Arc::new(GroupOutbound {
                    members,
                    kind,
                    preferred: std::sync::atomic::AtomicUsize::new(preferred),
                    use_saved: std::sync::atomic::AtomicBool::new(
                        group.current.is_some()
                            && matches!(kind, GroupKind::Fallback | GroupKind::UrlTest),
                    ),
                    sticky_until: std::sync::Mutex::new(None),
                    selection: tokio::sync::Mutex::new(()),
                    generation: std::sync::atomic::AtomicUsize::new(0),
                    next_probe: std::sync::atomic::AtomicUsize::new(0),
                    probe_slots: probe_budget().clone(),
                })),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Probe {
        calls: Arc<AtomicUsize>,
        fail: bool,
        hang: bool,
    }
    impl Outbound for Probe {
        fn connect(
            &self,
            _: &str,
            _: u16,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<BoxProxyStream>> + Send + '_>>
        {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                if self.hang {
                    std::future::pending::<()>().await;
                }
                if self.fail {
                    anyhow::bail!("synthetic failure");
                }
                Ok(Box::new(tokio::io::duplex(64).0) as BoxProxyStream)
            })
        }
        fn connect_datagram(&self, _: &str, _: u16) -> DatagramFuture<'_, BoxDatagram> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                anyhow::bail!("synthetic UDP failure")
            })
        }
        fn name(&self) -> &str {
            "fixture"
        }
    }
    fn node() -> Node {
        serde_json::from_value(serde_json::json!({"name":"Example","server":"proxy.example.invalid","port":443,"protocol":{"type":"vless","uuid":"00000000-0000-0000-0000-000000000000"},"transport":null})).unwrap()
    }
    fn member(fail: bool, hang: bool) -> (Arc<Member>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let cell = OnceLock::new();
        assert!(
            cell.set(Ok(SharedOutbound(Arc::new(Probe {
                calls: calls.clone(),
                fail,
                hang
            }))))
            .is_ok()
        );
        (
            Arc::new(Member {
                node: node(),
                outbound: cell,
            }),
            calls,
        )
    }
    struct CountingProbe {
        calls: Arc<AtomicUsize>,
        active: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
        complete: bool,
    }
    struct Attempt(Arc<AtomicUsize>);
    impl Drop for Attempt {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    impl Outbound for CountingProbe {
        fn connect(
            &self,
            _: &str,
            _: u16,
        ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.peak.fetch_max(active, Ordering::SeqCst);
                let _attempt = Attempt(self.active.clone());
                if !self.complete {
                    std::future::pending::<()>().await;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
                Ok(Box::new(tokio::io::duplex(64).0) as BoxProxyStream)
            })
        }
        fn name(&self) -> &str {
            "counting-fixture"
        }
    }
    fn counting_group(probe: Arc<CountingProbe>) -> Arc<GroupOutbound> {
        let members = (0..64)
            .map(|_| {
                let cell = OnceLock::new();
                cell.set(Ok(SharedOutbound(probe.clone()))).ok().unwrap();
                Arc::new(Member {
                    node: node(),
                    outbound: cell,
                })
            })
            .collect();
        Arc::new(GroupOutbound {
            members,
            kind: GroupKind::UrlTest,
            preferred: AtomicUsize::new(0),
            use_saved: std::sync::atomic::AtomicBool::new(false),
            sticky_until: std::sync::Mutex::new(None),
            selection: tokio::sync::Mutex::new(()),
            generation: AtomicUsize::new(0),
            next_probe: AtomicUsize::new(0),
            probe_slots: probe_budget().clone(),
        })
    }
    fn counting_probe(complete: bool) -> Arc<CountingProbe> {
        Arc::new(CountingProbe {
            calls: Arc::new(AtomicUsize::new(0)),
            active: Arc::new(AtomicUsize::new(0)),
            peak: Arc::new(AtomicUsize::new(0)),
            complete,
        })
    }
    #[tokio::test]
    async fn concurrent_url_test_requests_share_selection_and_keep_individual_streams() {
        let probe = counting_probe(true);
        let group = counting_group(probe.clone());
        let mut tasks = tokio::task::JoinSet::new();
        for i in 0..8 {
            let group = group.clone();
            tasks.spawn(async move { group.connect(&format!("target-{i}.invalid"), 443).await });
        }
        while let Some(result) = tasks.join_next().await {
            result.unwrap().unwrap();
        }
        let calls = probe.calls.load(Ordering::SeqCst);
        assert!(calls >= 8, "every request needs its own outbound stream");
        assert!(
            calls <= GROUP_PROBE_CONCURRENCY + 7,
            "selection was duplicated: {calls}"
        );
    }
    #[tokio::test]
    async fn group_probes_obey_global_and_per_group_limits_and_refund_on_cancel() {
        let probe = counting_probe(false);
        let groups: Vec<_> = (0..8).map(|_| counting_group(probe.clone())).collect();
        let mut tasks = tokio::task::JoinSet::new();
        for group in &groups {
            for _ in 0..8 {
                let group = group.clone();
                tasks.spawn(async move { group.connect("target.invalid", 443).await });
            }
        }
        tokio::time::timeout(Duration::from_secs(2), async {
            while probe.active.load(Ordering::SeqCst) < GLOBAL_PROBE_CONCURRENCY {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        for group in &groups {
            assert!(group.next_probe.load(Ordering::Relaxed) <= GROUP_PROBE_CONCURRENCY);
        }
        assert!(probe.peak.load(Ordering::SeqCst) <= GLOBAL_PROBE_CONCURRENCY);
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        tokio::time::timeout(Duration::from_secs(1), async {
            while probe.active.load(Ordering::SeqCst) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn fallback_is_bounded_and_udp_does_not_change_member() {
        let (first, a) = member(true, true);
        let (second, b) = member(false, false);
        let group = GroupOutbound {
            members: vec![first, second],
            kind: GroupKind::Fallback,
            preferred: std::sync::atomic::AtomicUsize::new(0),
            use_saved: std::sync::atomic::AtomicBool::new(false),
            sticky_until: std::sync::Mutex::new(None),
            selection: tokio::sync::Mutex::new(()),
            generation: std::sync::atomic::AtomicUsize::new(0),
            next_probe: std::sync::atomic::AtomicUsize::new(0),
            // Virtual-time fixtures must not wait on permits held by another
            // test runtime: their clock can advance while that runtime sleeps.
            probe_slots: Arc::new(tokio::sync::Semaphore::new(GLOBAL_PROBE_CONCURRENCY)),
        };
        let start = tokio::time::Instant::now();
        assert!(group.connect("example.invalid", 443).await.is_ok());
        assert_eq!(start.elapsed(), Duration::from_secs(3));
        assert_eq!(a.load(Ordering::SeqCst), 1);
        assert_eq!(b.load(Ordering::SeqCst), 1);
        assert!(
            group
                .connect_datagram("example.invalid", 443)
                .await
                .is_err()
        );
        assert_eq!(a.load(Ordering::SeqCst), 2);
        assert_eq!(b.load(Ordering::SeqCst), 1);
        assert!(group.connect("example.invalid", 443).await.is_ok());
        assert_eq!(a.load(Ordering::SeqCst), 2);
        assert_eq!(b.load(Ordering::SeqCst), 2);
        tokio::time::advance(Duration::from_secs(300)).await;
        assert!(group.connect("example.invalid", 443).await.is_ok());
        assert_eq!(a.load(Ordering::SeqCst), 3);
        assert_eq!(b.load(Ordering::SeqCst), 3);
    }
    #[tokio::test(start_paused = true)]
    async fn url_test_uses_the_fastest_member_until_the_result_expires() {
        let (first, a) = member(false, true);
        let (second, b) = member(false, false);
        let group = GroupOutbound {
            members: vec![first, second],
            kind: GroupKind::UrlTest,
            preferred: std::sync::atomic::AtomicUsize::new(0),
            use_saved: std::sync::atomic::AtomicBool::new(false),
            sticky_until: std::sync::Mutex::new(None),
            selection: tokio::sync::Mutex::new(()),
            generation: std::sync::atomic::AtomicUsize::new(0),
            next_probe: std::sync::atomic::AtomicUsize::new(0),
            // Virtual-time fixtures must not wait on permits held by another
            // test runtime: their clock can advance while that runtime sleeps.
            probe_slots: Arc::new(tokio::sync::Semaphore::new(GLOBAL_PROBE_CONCURRENCY)),
        };
        assert!(group.connect("example.invalid", 443).await.is_ok());
        assert_eq!(a.load(Ordering::SeqCst), 1);
        assert_eq!(b.load(Ordering::SeqCst), 1);
        assert!(group.connect("example.invalid", 443).await.is_ok());
        assert_eq!(a.load(Ordering::SeqCst), 1);
        assert_eq!(b.load(Ordering::SeqCst), 2);
        assert!(
            group
                .connect_datagram("example.invalid", 443)
                .await
                .is_err()
        );
        assert_eq!(b.load(Ordering::SeqCst), 3);
        assert_eq!(a.load(Ordering::SeqCst), 1);
    }
    #[tokio::test(start_paused = true)]
    async fn manual_selection_never_falls_back_and_missing_selection_is_closed() {
        let (first, a) = member(true, false);
        let (second, b) = member(false, false);
        let group = GroupOutbound {
            members: vec![first, second],
            kind: GroupKind::Select,
            preferred: std::sync::atomic::AtomicUsize::new(0),
            use_saved: std::sync::atomic::AtomicBool::new(false),
            sticky_until: std::sync::Mutex::new(None),
            selection: tokio::sync::Mutex::new(()),
            generation: std::sync::atomic::AtomicUsize::new(0),
            next_probe: std::sync::atomic::AtomicUsize::new(0),
            // Virtual-time fixtures must not wait on permits held by another
            // test runtime: their clock can advance while that runtime sleeps.
            probe_slots: Arc::new(tokio::sync::Semaphore::new(GLOBAL_PROBE_CONCURRENCY)),
        };
        assert!(group.connect("example.invalid", 443).await.is_err());
        assert_eq!(a.load(Ordering::SeqCst), 1);
        assert_eq!(b.load(Ordering::SeqCst), 0);
        let n = node();
        let fp = crate::config::dedup::node_fingerprint(&n);
        let config = ProxyGroupConfig {
            name: "Video".into(),
            gtype: GroupType::Select,
            members: vec![fp, "deleted".into()],
            current: Some("deleted".into()),
        };
        let groups = build_groups(&[config], &[n]);
        assert!(
            groups["Video"]
                .0
                .connect("example.invalid", 443)
                .await
                .is_err()
        );
        assert!(
            groups["Video"]
                .0
                .connect_datagram("example.invalid", 443)
                .await
                .is_err()
        );
    }
}

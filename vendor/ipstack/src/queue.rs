//! Byte-accounted packet queues. Ingress overflow happens before TCP ACK.
use crate::packet::NetworkPacket;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, AtomicUsize, Ordering},
    },
    task::{Context, Poll, Waker},
};
use tokio::sync::mpsc;
#[derive(Debug)]
pub(crate) struct Budget {
    used: AtomicUsize,
    limit: usize,
    waiters: Mutex<std::collections::VecDeque<std::sync::Weak<Mutex<Option<Waker>>>>>,
    flows: Mutex<Vec<std::sync::Weak<std::sync::Mutex<crate::stream::tcb::Tcb>>>>,
}
impl Budget {
    pub fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            used: AtomicUsize::new(0),
            limit,
            waiters: Mutex::new(std::collections::VecDeque::new()),
            flows: Mutex::new(Vec::new()),
        })
    }
    pub fn register_flow(&self, flow: &Arc<Mutex<crate::stream::tcb::Tcb>>) {
        let mut flows = self.flows.lock().unwrap();
        flows.retain(|flow| flow.strong_count() > 0);
        flows.push(Arc::downgrade(flow));
    }
    pub fn flow_health(&self) -> ([usize; 10], u64, usize) {
        let mut result = ([0; 10], 0, 0);
        let mut flows = self.flows.lock().unwrap();
        flows.retain(|flow| {
            if let Some(flow) = flow.upgrade() {
                let (state, age, bytes) = flow.lock().unwrap().health();
                result.0[state] += 1;
                result.1 = result.1.max(age);
                result.2 += bytes;
                true
            } else {
                false
            }
        });
        result
    }
    pub fn used(&self) -> usize {
        self.used.load(Ordering::Acquire)
    }
    pub fn limit(&self) -> usize {
        self.limit
    }
    fn take(&self, n: usize, reserve: usize) -> bool {
        self.used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(n)
                    .filter(|next| *next <= self.limit.saturating_sub(reserve))
            })
            .is_ok()
    }
    fn release(&self, n: usize) {
        self.used.fetch_sub(n, Ordering::AcqRel);
        // Wake one blocked producer per released packet in round-robin
        // order. Waking every flow here creates a CPU storm under pressure.
        let ready = {
            let mut waiters = self.waiters.lock().unwrap();
            let mut ready = None;
            for _ in 0..waiters.len() {
                let Some(weak) = waiters.pop_front() else {
                    break;
                };
                if let Some(waiter) = weak.upgrade() {
                    waiters.push_back(weak);
                    ready = waiter.lock().unwrap().take();
                    if ready.is_some() {
                        break;
                    }
                }
            }
            ready
        };
        if let Some(waker) = ready {
            waker.wake();
        }
    }
    fn register(&self, waiter: &Arc<Mutex<Option<Waker>>>) {
        let mut waiters = self.waiters.lock().unwrap();
        waiters.retain(|waiter| waiter.strong_count() > 0);
        waiters.push_back(Arc::downgrade(waiter));
    }
}
#[derive(Debug)]
struct Queued {
    packet: Option<NetworkPacket>,
    local: Arc<Budget>,
    global: Arc<Budget>,
    charge: usize,
}
impl Drop for Queued {
    fn drop(&mut self) {
        self.local.release(self.charge);
        self.global.release(self.charge);
    }
}
#[derive(Debug)]
pub(crate) struct PacketSender {
    tx: mpsc::UnboundedSender<Queued>,
    local_waiter: Arc<Mutex<Option<Waker>>>,
    global_waiter: Arc<Mutex<Option<Waker>>>,
    registered: AtomicU8,
    blocked_on: AtomicU8,
    local: Arc<Budget>,
    global: Arc<Budget>,
}
#[derive(Debug)]
pub(crate) struct PacketReceiver {
    rx: mpsc::UnboundedReceiver<Queued>,
}
#[derive(Debug)]
pub(crate) struct SendError(pub NetworkPacket, pub bool);
impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.1 {
            "packet queue full"
        } else {
            "packet queue closed"
        })
    }
}
impl std::error::Error for SendError {}
pub(crate) fn channel(limit: usize, global: Arc<Budget>) -> (PacketSender, PacketReceiver) {
    let (tx, rx) = mpsc::unbounded_channel();
    (
        PacketSender {
            tx,
            local_waiter: Arc::new(Mutex::new(None)),
            global_waiter: Arc::new(Mutex::new(None)),
            registered: AtomicU8::new(0),
            blocked_on: AtomicU8::new(0),
            local: Budget::new(limit),
            global,
        },
        PacketReceiver { rx },
    )
}
impl Clone for PacketSender {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            local: self.local.clone(),
            global: self.global.clone(),
            local_waiter: Arc::new(Mutex::new(None)),
            global_waiter: Arc::new(Mutex::new(None)),
            registered: AtomicU8::new(0),
            blocked_on: AtomicU8::new(0),
        }
    }
}
impl PacketSender {
    pub fn global(&self) -> Arc<Budget> {
        self.global.clone()
    }
    pub fn same_channel(&self, other: &Self) -> bool {
        self.tx.same_channel(&other.tx)
    }
    pub fn register(&self, cx: &Context<'_>) {
        let blocked = self.blocked_on.load(Ordering::Acquire);
        let (waiter, other, budget) = if blocked == 1 {
            (&self.local_waiter, &self.global_waiter, &self.local)
        } else {
            (&self.global_waiter, &self.local_waiter, &self.global)
        };
        *other.lock().unwrap() = None;
        *waiter.lock().unwrap() = Some(cx.waker().clone());
        if self.registered.fetch_or(blocked, Ordering::AcqRel) & blocked == 0 {
            budget.register(waiter);
        }
    }
    pub fn send(&self, packet: NetworkPacket) -> Result<(), SendError> {
        if self.tx.is_closed() {
            return Err(SendError(packet, false));
        }
        let charge = packet
            .payload
            .as_ref()
            .map_or(0, Vec::len)
            .saturating_add(256);
        // Reserve capacity for ACK/FIN/RST and DNS even under bulk load.
        let priority = match &packet.transport {
            crate::packet::TransportHeader::Tcp(_) => {
                packet.payload.as_ref().is_none_or(Vec::is_empty)
            }
            crate::packet::TransportHeader::Udp(header) => {
                header.source_port == 53 || header.destination_port == 53
            }
            _ => true,
        };
        let local_reserve = if priority { 0 } else { self.local.limit / 8 };
        let global_reserve = if priority { 0 } else { self.global.limit / 8 };
        if !self.local.take(charge, local_reserve) {
            self.blocked_on.store(1, Ordering::Release);
            return Err(SendError(packet, true));
        }
        if !self.global.take(charge, global_reserve) {
            self.blocked_on.store(2, Ordering::Release);
            self.local.release(charge);
            return Err(SendError(packet, true));
        }
        let item = Queued {
            packet: Some(packet),
            local: self.local.clone(),
            global: self.global.clone(),
            charge,
        };
        self.tx.send(item).map_err(|e| {
            let mut item = e.0;
            SendError(item.packet.take().unwrap(), false)
        })
    }
}
impl PacketReceiver {
    pub async fn recv(&mut self) -> Option<NetworkPacket> {
        self.rx.recv().await.map(|mut p| p.packet.take().unwrap())
    }
    pub fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<NetworkPacket>> {
        self.rx
            .poll_recv(cx)
            .map(|p| p.map(|mut p| p.packet.take().unwrap()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn packet() -> NetworkPacket {
        let builder =
            etherparse::PacketBuilder::ipv4([192, 0, 2, 2], [198, 51, 100, 2], 64).udp(1000, 443);
        let mut bytes = Vec::new();
        builder.write(&mut bytes, &[42; 500]).unwrap();
        NetworkPacket::parse(&bytes).unwrap()
    }
    struct WakeCount(AtomicUsize);
    impl std::task::Wake for WakeCount {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    #[tokio::test]
    async fn global_capacity_wakes_producers_one_at_a_time_and_returns_credits() {
        let budget = Budget::new(4096);
        let (first, mut receiver) = channel(8192, budget.clone());
        let (second, _second_receiver) = channel(8192, budget.clone());
        for _ in 0..4 {
            first.send(packet()).unwrap();
        }
        assert!(first.send(packet()).unwrap_err().1);
        assert!(second.send(packet()).unwrap_err().1);
        let count = Arc::new(WakeCount(AtomicUsize::new(0)));
        let waker = std::task::Waker::from(count.clone());
        let cx = Context::from_waker(&waker);
        first.register(&cx);
        second.register(&cx);
        receiver.recv().await.unwrap();
        assert_eq!(count.0.load(Ordering::Relaxed), 1);
        receiver.recv().await.unwrap();
        assert_eq!(count.0.load(Ordering::Relaxed), 2);
        drop(receiver);
        assert_eq!(budget.used(), 0);
    }
    #[test]
    fn cancelled_producers_do_not_retain_task_wakers() {
        let budget = Budget::new(4096);
        let (sender, receiver) = channel(1024, budget.clone());
        sender.send(packet()).unwrap();
        assert!(sender.send(packet()).unwrap_err().1);
        let count = Arc::new(WakeCount(AtomicUsize::new(0)));
        let waker = std::task::Waker::from(count.clone());
        let cx = Context::from_waker(&waker);
        sender.register(&cx);
        assert_eq!(Arc::strong_count(&count), 3);
        drop(sender);
        assert_eq!(Arc::strong_count(&count), 2);
        drop(receiver);
        assert_eq!(budget.used(), 0);
    }
}

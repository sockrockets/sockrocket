//! Aggregate TCP payload accounting, independent of each flow's window.
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::task::Waker;

/// Shared by every TCP flow of one stack. Credits cover receive payload
/// (including data waiting for application reads) and unacknowledged sends.
#[derive(Debug)]
pub struct TcpPayloadBudget {
    used: AtomicUsize,
    limit: usize,
    writers: Mutex<std::collections::VecDeque<Weak<Mutex<Option<Waker>>>>>,
    has_writers: AtomicBool,
}
impl TcpPayloadBudget {
    pub fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            used: AtomicUsize::new(0),
            limit: limit.max(1),
            writers: Mutex::new(std::collections::VecDeque::new()),
            has_writers: AtomicBool::new(false),
        })
    }
    pub fn usage(&self) -> (usize, usize) {
        (self.used.load(Ordering::Acquire), self.limit)
    }
    pub(crate) fn reserve(self: &Arc<Self>, bytes: usize) -> Option<Reservation> {
        self.used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|next| *next <= self.limit)
            })
            .ok()?;
        // Pass remaining credit to another blocked writer, including after
        // a large release which only woke the first writer in the queue.
        if self.usage().0 < self.limit {
            self.wake_one();
        }
        Some(Reservation {
            pool: self.clone(),
            bytes,
        })
    }
    pub(crate) fn release(&self, bytes: usize) {
        if bytes != 0 {
            self.used.fetch_sub(bytes, Ordering::AcqRel);
            self.wake_one();
        }
    }
    pub(crate) fn register(&self, writer: &Arc<Mutex<Option<Waker>>>) {
        let mut writers = self.writers.lock().unwrap();
        writers.retain(|w| w.strong_count() != 0);
        let weak = Arc::downgrade(writer);
        if !writers.iter().any(|w| w.ptr_eq(&weak)) {
            writers.push_back(weak);
        }
        self.has_writers.store(true, Ordering::Release);
    }
    fn wake_one(&self) {
        if !self.has_writers.load(Ordering::Acquire) {
            return;
        }
        let wake = {
            let mut writers = self.writers.lock().unwrap();
            let mut wake = None;
            for _ in 0..writers.len() {
                let Some(weak) = writers.pop_front() else {
                    break;
                };
                if let Some(writer) = weak.upgrade() {
                    wake = writer.lock().unwrap().take();
                    if wake.is_some() {
                        break;
                    }
                }
            }
            self.has_writers
                .store(!writers.is_empty(), Ordering::Release);
            wake
        };
        if let Some(wake) = wake {
            wake.wake();
        }
    }
}
pub(crate) struct Reservation {
    pool: Arc<TcpPayloadBudget>,
    bytes: usize,
}
impl Reservation {
    /// Ownership of used credits transfers to the TCB, which releases them
    /// on ACK/application consumption/error/drop. Failed writes refund all.
    pub(crate) fn commit(mut self, used: usize) {
        assert!(used <= self.bytes);
        self.pool.release(self.bytes - used);
        self.bytes = 0;
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.pool.release(self.bytes);
    }
}

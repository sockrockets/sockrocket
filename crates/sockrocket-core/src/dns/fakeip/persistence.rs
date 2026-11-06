//! One bounded, coalescing writer per persistent Fake-IP pool. Requests only
//! advance a generation; they never enqueue snapshots or wait for disk I/O.
use std::{
    io,
    sync::{Arc, Condvar, Mutex},
    thread::JoinHandle,
    time::{Duration, Instant},
};

#[derive(Default)]
struct State {
    requested: u64,
    persisted: u64,
    attempted: u64,
    force: bool,
    stopping: bool,
    last_attempt: Option<Instant>,
    error: Option<(io::ErrorKind, String)>,
}
#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

pub(super) struct CoalescingWriter {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}
impl CoalescingWriter {
    pub(super) fn new(
        interval: Duration,
        write: impl Fn() -> io::Result<()> + Send + 'static,
    ) -> io::Result<Self> {
        let shared = Arc::new(Shared::default());
        let background = shared.clone();
        let worker = std::thread::Builder::new()
            .name("fakeip-writer".into())
            .spawn(move || run(background, interval, write))?;
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }

    pub(super) fn schedule(&self) {
        let mut state = self.shared.state.lock().unwrap();
        state.requested += 1;
        self.shared.changed.notify_all();
    }

    /// Barrier: success means a snapshot at least as recent as this request
    /// was committed. Errors reach the caller; background retries stay bounded.
    pub(super) fn flush(&self) -> io::Result<()> {
        let mut state = self.shared.state.lock().unwrap();
        state.requested += 1;
        let target = state.requested;
        state.force = true;
        self.shared.changed.notify_all();
        while state.attempted < target {
            state = self.shared.changed.wait(state).unwrap();
        }
        if state.persisted >= target {
            Ok(())
        } else {
            let (kind, message) = state.error.as_ref().expect("failed attempt has an error");
            Err(io::Error::new(*kind, message.clone()))
        }
    }
}
impl Drop for CoalescingWriter {
    fn drop(&mut self) {
        {
            let mut state = self.shared.state.lock().unwrap();
            state.stopping = true;
            self.shared.changed.notify_all();
        }
        // The worker owns only the snapshot source, never the pool/writer.
        // Join drains trailing changes and prevents stale writes after drop.
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run(shared: Arc<Shared>, interval: Duration, write: impl Fn() -> io::Result<()>) {
    loop {
        let generation = {
            let mut state = shared.state.lock().unwrap();
            loop {
                if state.stopping && state.attempted >= state.requested {
                    return;
                }
                if state.requested <= state.persisted {
                    state = shared.changed.wait(state).unwrap();
                    continue;
                }
                if !state.force && !state.stopping {
                    if let Some(remaining) = state
                        .last_attempt
                        .and_then(|at| interval.checked_sub(at.elapsed()))
                    {
                        state = shared.changed.wait_timeout(state, remaining).unwrap().0;
                        continue;
                    }
                }
                state.force = false;
                break state.requested;
            }
        };
        // Disk I/O never holds the scheduler lock. A poisoned snapshot source
        // must report a failed flush rather than strand every flush waiter.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(&write))
            .unwrap_or_else(|_| Err(io::Error::other("fake-IP snapshot writer panicked")));
        if let Err(error) = &result {
            tracing::warn!("fake-IP background persist failed: {error}");
        }
        let mut state = shared.state.lock().unwrap();
        state.attempted = generation;
        state.last_attempt = Some(Instant::now());
        match result {
            Ok(()) => {
                state.persisted = generation;
                state.error = None;
            }
            Err(error) => {
                state.error = Some((error.kind(), error.to_string()));
            }
        }
        shared.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    };

    #[test]
    fn slow_disk_does_not_block_scheduling_and_bursts_coalesce() {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let writer = CoalescingWriter::new(Duration::from_secs(3600), move || {
            if count.fetch_add(1, Ordering::SeqCst) == 0 {
                entered_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            }
            Ok(())
        })
        .unwrap();
        writer.schedule();
        entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        for _ in 0..10_000 {
            writer.schedule();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        release_tx.send(()).unwrap();
        writer.flush().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn trailing_change_is_saved_without_another_request() {
        let (tx, rx) = mpsc::channel();
        let writer = CoalescingWriter::new(Duration::from_millis(20), move || {
            tx.send(Instant::now()).unwrap();
            Ok(())
        })
        .unwrap();
        writer.flush().unwrap();
        let first = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        writer.schedule();
        let second = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(second.duration_since(first) >= Duration::from_millis(20));
    }

    #[test]
    fn failed_write_reports_error_and_retries_without_new_traffic() {
        let fail = Arc::new(AtomicBool::new(true));
        let failure = fail.clone();
        let (tx, rx) = mpsc::channel();
        let writer = CoalescingWriter::new(Duration::from_millis(20), move || {
            if failure.load(Ordering::SeqCst) {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            tx.send(()).unwrap();
            Ok(())
        })
        .unwrap();
        assert_eq!(
            writer.flush().unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        fail.store(false, Ordering::SeqCst);
        rx.recv_timeout(Duration::from_secs(3)).unwrap();
        writer.flush().unwrap();
    }

    #[test]
    fn drop_drains_trailing_changes_and_stops_the_worker() {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let writer = CoalescingWriter::new(Duration::from_secs(3600), move || {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
        writer.flush().unwrap();
        writer.schedule();
        drop(writer);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(Arc::strong_count(&calls), 1);
    }

    #[test]
    fn panicking_writer_reports_failure_instead_of_stranding_flush() {
        let writer = CoalescingWriter::new(Duration::from_secs(3600), || {
            panic!("injected snapshot failure");
        })
        .unwrap();
        assert_eq!(writer.flush().unwrap_err().kind(), io::ErrorKind::Other);
    }
}

//! One coalescing writer; acknowledgement follows the private atomic commit.
use sockrocket_core::config::{model::AppConfig, watch::ConfigWatchHandle};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
struct Job {
    generation: u64,
    config: AppConfig,
    watch: Option<ConfigWatchHandle>,
}
#[derive(Default)]
struct State {
    next: u64,
    done: u64,
    pending: Option<Job>,
    error: Option<String>,
}
struct Writer {
    state: Mutex<State>,
    changed: Condvar,
}
fn writer() -> &'static Arc<Writer> {
    static WRITER: OnceLock<Arc<Writer>> = OnceLock::new();
    WRITER.get_or_init(|| {
        let writer = Arc::new(Writer {
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        });
        let worker = writer.clone();
        std::thread::Builder::new()
            .name("configuration-writer".into())
            .spawn(move || {
                loop {
                    let job = {
                        let mut state = worker.state.lock().unwrap();
                        while state.pending.is_none() {
                            state = worker.changed.wait(state).unwrap();
                        }
                        state.pending.take().unwrap()
                    };
                    let error = crate::app::save_gui_state(&job.config, job.watch.as_ref())
                        .err()
                        .map(|_| {
                            "Configuration could not be saved. Keep the app open and retry."
                                .to_owned()
                        });
                    let mut state = worker.state.lock().unwrap();
                    state.done = job.generation;
                    state.error = error;
                    worker.changed.notify_all();
                }
            })
            .expect("configuration writer thread");
        writer
    })
}
pub fn enqueue(config: AppConfig, watch: Option<ConfigWatchHandle>) -> u64 {
    let writer = writer();
    let mut state = writer.state.lock().unwrap();
    state.next += 1;
    let generation = state.next;
    // A newer complete snapshot supersedes an older snapshot still waiting.
    state.pending = Some(Job {
        generation,
        config,
        watch,
    });
    writer.changed.notify_all();
    generation
}
pub fn wait(generation: u64) -> anyhow::Result<()> {
    let writer = writer();
    let state = writer.state.lock().unwrap();
    let (state, _) = writer
        .changed
        .wait_timeout_while(state, std::time::Duration::from_secs(10), |state| {
            state.done < generation
        })
        .unwrap();
    if state.done < generation {
        anyhow::bail!("Timed out waiting for durable configuration save");
    }
    if let Some(error) = &state.error {
        anyhow::bail!("{}", error);
    }
    Ok(())
}

/// Normal-quit barrier for the most recently queued complete snapshot.
pub fn flush() -> anyhow::Result<()> {
    let generation = writer().state.lock().unwrap().next;
    if generation == 0 {
        return Ok(());
    }
    wait(generation)
}

pub fn is_idle() -> bool {
    let state = writer().state.lock().unwrap();
    state.done >= state.next
}

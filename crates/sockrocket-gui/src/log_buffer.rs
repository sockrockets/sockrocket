use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex};

use gpui::SharedString;
use tracing::Level;
use tracing_subscriber::Layer;

/// Maximum number of log entries to keep in the ring buffer.
const MAX_LOG_ENTRIES: usize = 500;

/// A single captured log entry.
#[derive(Clone)]
pub struct LogEntry {
    pub level: Level,
    pub target: String,
    pub message: String,
    /// Precomputed "HH:MM:SS" local capture time. The entry time never
    /// changes, so the chrono format runs once at capture time instead of
    /// per row per render on the Logs page. `SharedString` makes per-row
    /// reads free.
    time_hms: SharedString,
}

impl LogEntry {
    pub fn new(level: Level, target: String, message: String, at_unix: u64) -> Self {
        let time_hms = chrono::DateTime::from_timestamp(at_unix as i64, 0)
            .map(|dt| {
                let local: chrono::DateTime<chrono::Local> = dt.into();
                local.format("%H:%M:%S").to_string()
            })
            .unwrap_or_default();
        Self {
            level,
            target,
            message,
            time_hms: SharedString::from(time_hms),
        }
    }

    /// "HH:MM:SS" local time for the log table (cached at capture time).
    pub fn time_hms(&self) -> SharedString {
        self.time_hms.clone()
    }
}

impl fmt::Display for LogEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let lvl = match self.level {
            Level::ERROR => "ERROR",
            Level::WARN => "WARN ",
            Level::INFO => "INFO ",
            Level::DEBUG => "DEBUG",
            Level::TRACE => "TRACE",
        };
        write!(
            f,
            "{} [{}] {}: {}",
            self.time_hms(),
            lvl,
            self.target,
            self.message
        )
    }
}

/// Shared log buffer accessible by the GUI. `generation` is bumped on every
/// mutation so readers (the Logs page) can cache a reversed snapshot instead
/// of cloning the whole ring on every frame.
pub struct LogBuffer {
    pub entries: VecDeque<LogEntry>,
    pub generation: u64,
}

/// Shared log buffer accessible by the GUI.
pub type SharedLogBuffer = Arc<Mutex<LogBuffer>>;

/// Create a new shared log buffer.
pub fn new_log_buffer() -> SharedLogBuffer {
    Arc::new(Mutex::new(LogBuffer {
        entries: VecDeque::with_capacity(MAX_LOG_ENTRIES),
        generation: 0,
    }))
}

/// A tracing layer that captures events into a shared ring buffer.
pub struct LogCaptureLayer {
    buffer: SharedLogBuffer,
}

impl LogCaptureLayer {
    pub fn new(buffer: SharedLogBuffer) -> Self {
        Self { buffer }
    }
}

impl<S> Layer<S> for LogCaptureLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let metadata = event.metadata();
        let level = *metadata.level();

        // Only capture INFO and above for the GUI log viewer
        if level > Level::DEBUG {
            return;
        }

        let target = metadata.target().to_string();

        // Extract message from the event fields
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        let message = visitor.0;

        if let Ok(mut buf) = self.buffer.lock() {
            if buf.entries.len() >= MAX_LOG_ENTRIES {
                buf.entries.pop_front();
            }
            buf.entries.push_back(LogEntry::new(
                level,
                target,
                message,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            ));
            buf.generation = buf.generation.wrapping_add(1);
        }
    }
}

/// Visitor that extracts the `message` field from tracing events.
struct MessageVisitor(String);

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{:?}", value);
        } else if self.0.is_empty() {
            self.0 = format!("{} = {:?}", field.name(), value);
        } else {
            self.0
                .push_str(&format!(", {} = {:?}", field.name(), value));
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.0 = value.to_string();
        } else if self.0.is_empty() {
            self.0 = format!("{} = {}", field.name(), value);
        } else {
            self.0.push_str(&format!(", {} = {}", field.name(), value));
        }
    }
}

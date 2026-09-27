//! Config file hot-reload via simple mtime polling (no `notify` dependency).
//!
//! The watcher polls the file every few seconds; when the mtime changes it
//! re-reads and re-parses the YAML. A parsed config is emitted together with
//! the list of fields that require a service restart to take effect
//! (listen address / ports). Parse failures are reported as errors and the
//! previously accepted config is kept — a bad edit can never take down a
//! running service.
//!
//! Writers (e.g. the GUI persisting its own state) suppress the resulting
//! no-op reload by calling [`ConfigWatchHandle::note_saved`] with the content
//! hash before/after writing.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use super::model::AppConfig;

/// Compute a stable hash of serialized config content.
///
/// Used to distinguish real changes from mtime-only touches and to let the
/// writer suppress reload events for its own saves.
pub fn config_content_hash(content: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

/// A successfully reloaded config.
#[derive(Debug, Clone)]
pub struct ReloadedConfig {
    /// The newly parsed config.
    pub config: AppConfig,
    /// Fields that changed relative to the running baseline and only take
    /// effect after a service restart (e.g. `listen_addr`, `socks_port`).
    pub requires_restart: Vec<String>,
}

/// Events emitted by [`ConfigWatcher`].
#[derive(Debug, Clone)]
pub enum ConfigWatchEvent {
    /// The file changed and parsed successfully.
    Reloaded(ReloadedConfig),
    /// The file changed but failed to parse; the old config stays active.
    ParseError(String),
}

struct WatcherState {
    /// Hash of the last accepted (or externally saved) content.
    content_hash: u64,
    /// The listen settings of the running service; changes against this
    /// baseline are reported as `requires_restart`.
    restart_baseline: (String, u16, u16),
}

/// Cloneable handle for writers to suppress self-triggered reloads.
#[derive(Clone)]
pub struct ConfigWatchHandle {
    state: Arc<Mutex<WatcherState>>,
}

impl ConfigWatchHandle {
    /// Record externally saved content so the watcher does not emit a reload
    /// event for it, and adopt `config`'s listen settings as the new
    /// restart baseline.
    pub fn note_saved(&self, config: &AppConfig, content_hash: u64) {
        let mut state = self.state.lock().unwrap();
        state.content_hash = content_hash;
        state.restart_baseline = (
            config.listen_addr.clone(),
            config.socks_port,
            config.http_port,
        );
    }
}

/// Watches a config file for changes via mtime polling.
pub struct ConfigWatcher {
    shutdown_tx: watch::Sender<bool>,
    handle: JoinHandle<()>,
    events_rx: mpsc::UnboundedReceiver<ConfigWatchEvent>,
    state: Arc<Mutex<WatcherState>>,
}

impl ConfigWatcher {
    /// Start watching `path`, polling every `poll_interval`.
    ///
    /// `current` is the config the service is currently running with; it
    /// seeds both the content hash (so no event fires for the on-disk state
    /// at startup) and the restart baseline.
    pub fn spawn(path: impl Into<PathBuf>, current: &AppConfig, poll_interval: Duration) -> Self {
        let path = path.into();
        let content_hash = std::fs::read_to_string(&path)
            .map(|c| config_content_hash(&c))
            .unwrap_or(0);
        let state = Arc::new(Mutex::new(WatcherState {
            content_hash,
            restart_baseline: (
                current.listen_addr.clone(),
                current.socks_port,
                current.http_port,
            ),
        }));

        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let task_state = state.clone();

        let handle = tokio::spawn(async move {
            let mut last_mtime = file_mtime(&path);
            let poll = poll_interval.max(Duration::from_millis(500));
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(poll) => {}
                    _ = shutdown_rx.changed() => break,
                }
                let mtime = file_mtime(&path);
                if mtime == last_mtime {
                    continue;
                }
                last_mtime = mtime;

                let content = match std::fs::read_to_string(&path) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::debug!("config watcher: cannot read {}: {}", path.display(), e);
                        continue;
                    }
                };
                let hash = config_content_hash(&content);
                {
                    let state = task_state.lock().unwrap();
                    if hash == state.content_hash {
                        // Writer's own save, or a content-free touch.
                        continue;
                    }
                }

                match serde_yaml::from_str::<AppConfig>(&content) {
                    Ok(config) => {
                        let requires_restart = {
                            let mut state = task_state.lock().unwrap();
                            state.content_hash = hash;
                            let baseline = state.restart_baseline.clone();
                            restart_fields(&baseline, &config)
                        };
                        tracing::info!(
                            "config file changed, reloaded (requires restart: {:?})",
                            requires_restart
                        );
                        let _ = events_tx.send(ConfigWatchEvent::Reloaded(ReloadedConfig {
                            config,
                            requires_restart,
                        }));
                    }
                    Err(e) => {
                        // Keep the previous config; never let a bad edit
                        // affect the running service.
                        tracing::error!(
                            "config file changed but failed to parse, keeping previous config: {}",
                            e
                        );
                        let _ = events_tx.send(ConfigWatchEvent::ParseError(format!("{:#}", e)));
                    }
                }
            }
        });

        Self {
            shutdown_tx,
            handle,
            events_rx,
            state,
        }
    }

    /// Handle for writers to suppress self-triggered reload events.
    pub fn handle(&self) -> ConfigWatchHandle {
        ConfigWatchHandle {
            state: self.state.clone(),
        }
    }

    /// Wait for the next watch event. Returns `None` after shutdown.
    pub async fn next(&mut self) -> Option<ConfigWatchEvent> {
        self.events_rx.recv().await
    }

    /// Non-blocking variant of [`Self::next`], mainly for tests.
    pub fn try_next(&mut self) -> Option<ConfigWatchEvent> {
        self.events_rx.try_recv().ok()
    }

    /// Stop the watcher task.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

impl Drop for ConfigWatcher {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(true);
        self.handle.abort();
    }
}

fn file_mtime(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Compare the restart-relevant listen fields against the running baseline.
fn restart_fields(baseline: &(String, u16, u16), config: &AppConfig) -> Vec<String> {
    let mut fields = Vec::new();
    if config.listen_addr != baseline.0 {
        fields.push("listen_addr".to_string());
    }
    if config.socks_port != baseline.1 {
        fields.push("socks_port".to_string());
    }
    if config.http_port != baseline.2 {
        fields.push("http_port".to_string());
    }
    fields
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::Node;

    fn temp_config_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "sockrocket-watch-test-{}-{}.yaml",
            std::process::id(),
            tag
        ))
    }

    fn write_config(path: &std::path::Path, config: &AppConfig) -> u64 {
        let content = serde_yaml::to_string(config).unwrap();
        std::fs::write(path, &content).unwrap();
        config_content_hash(&content)
    }

    fn node(name: &str) -> Node {
        use crate::config::model::ProxyProtocol;
        Node {
            name: name.to_string(),
            server: "127.0.0.1".to_string(),
            port: 443,
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
    fn content_hash_is_stable() {
        assert_eq!(config_content_hash("abc"), config_content_hash("abc"));
        assert_ne!(config_content_hash("abc"), config_content_hash("abd"));
    }

    #[tokio::test]
    async fn watcher_emits_reload_and_parse_error() {
        let path = temp_config_path("reload");
        let mut config = AppConfig {
            socks_port: 21080,
            http_port: 21087,
            ..AppConfig::default()
        };
        write_config(&path, &config);

        let mut watcher = ConfigWatcher::spawn(&path, &config, Duration::from_millis(200));

        // 1. Runtime-changeable edit: add a node.
        // Sleep so the mtime actually advances (coarse fs timestamps).
        tokio::time::sleep(Duration::from_millis(1100)).await;
        config.nodes.push(node("a"));
        write_config(&path, &config);
        let event = tokio::time::timeout(Duration::from_secs(10), watcher.next())
            .await
            .unwrap()
            .unwrap();
        match event {
            ConfigWatchEvent::Reloaded(reloaded) => {
                assert_eq!(reloaded.config.nodes.len(), 1);
                assert!(reloaded.requires_restart.is_empty());
            }
            other => panic!("expected Reloaded, got {:?}", other),
        }

        // 2. Restart-requiring edit: change the SOCKS port.
        tokio::time::sleep(Duration::from_millis(1100)).await;
        config.socks_port = 21081;
        write_config(&path, &config);
        let event = tokio::time::timeout(Duration::from_secs(10), watcher.next())
            .await
            .unwrap()
            .unwrap();
        match event {
            ConfigWatchEvent::Reloaded(reloaded) => {
                assert_eq!(reloaded.requires_restart, vec!["socks_port".to_string()]);
            }
            other => panic!("expected Reloaded, got {:?}", other),
        }

        // 3. Broken YAML: ParseError, old config kept.
        tokio::time::sleep(Duration::from_millis(1100)).await;
        std::fs::write(&path, "nodes: [unclosed\n  : : :").unwrap();
        let event = tokio::time::timeout(Duration::from_secs(10), watcher.next())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(event, ConfigWatchEvent::ParseError(_)));

        watcher.shutdown();
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn watcher_suppresses_writer_saves() {
        let path = temp_config_path("suppress");
        let mut config = AppConfig::default();
        write_config(&path, &config);

        let mut watcher = ConfigWatcher::spawn(&path, &config, Duration::from_millis(200));
        let handle = watcher.handle();

        // Simulate the GUI saving: note the hash, then write the file.
        tokio::time::sleep(Duration::from_millis(1100)).await;
        config.nodes.push(node("b"));
        let content = serde_yaml::to_string(&config).unwrap();
        handle.note_saved(&config, config_content_hash(&content));
        std::fs::write(&path, &content).unwrap();

        // Give the poller several chances to observe the file.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(watcher.try_next().is_none(), "own save must not reload");

        watcher.shutdown();
        let _ = std::fs::remove_file(&path);
    }
}

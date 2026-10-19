use sockrocket_core::config::{model::AppConfig, watch::ConfigWatchHandle};
mod app {
    use super::*;
    pub fn path() -> &'static std::path::PathBuf {
        static PATH: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
        PATH.get_or_init(|| {
            std::env::temp_dir().join(format!(
                "sockrocket-worker-review-{}.yaml",
                std::process::id()
            ))
        })
    }
    pub fn save_gui_state(
        config: &AppConfig,
        _watch: Option<&ConfigWatchHandle>,
    ) -> anyhow::Result<()> {
        if config.locale == "fail" {
            anyhow::bail!("synthetic save failure");
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
        sockrocket_core::config::storage::atomic_write_private(
            path(),
            &serde_yaml::to_string(config)?,
            false,
        )
    }
}
#[path = "../../sockrocket-gui/src/persistence.rs"]
mod persistence;
#[test]
fn coalesced_snapshots_are_durable_in_order_and_quit_barrier_waits() {
    let mut config = AppConfig::default();
    let mut generation = 0;
    for index in 0..1000 {
        config.locale = format!("snapshot-{index}");
        generation = persistence::enqueue(config.clone(), None);
    }
    persistence::wait(generation).unwrap();
    let saved: AppConfig =
        serde_yaml::from_str(&std::fs::read_to_string(app::path()).unwrap()).unwrap();
    assert_eq!(saved.locale, "snapshot-999");
    config.locale = "fail".into();
    let failed = persistence::enqueue(config.clone(), None);
    assert!(persistence::wait(failed).is_err());
    config.locale = "quit-snapshot".into();
    persistence::enqueue(config, None);
    persistence::flush().unwrap();
    assert!(persistence::is_idle());
    let saved: AppConfig =
        serde_yaml::from_str(&std::fs::read_to_string(app::path()).unwrap()).unwrap();
    assert_eq!(saved.locale, "quit-snapshot");
    std::fs::remove_file(app::path()).unwrap();
}

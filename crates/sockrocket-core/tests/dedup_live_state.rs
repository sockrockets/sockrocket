//! One-off cleanup: dedup nodes in the live GUI state file.
//! Run: cargo test -p sockrocket-core --test dedup_live_state dedup -- --ignored --nocapture
//! Requires the GUI to be stopped first (it holds the file).

use sockrocket_core::config::dedup::dedup_nodes;
use sockrocket_core::config::model::AppConfig;

#[test]
#[ignore = "mutates the live gui-state.yaml; run deliberately"]
fn dedup() {
    let path = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .expect("HOME")
        .join(".config/sockrocket/gui-state.yaml");
    let content = std::fs::read_to_string(&path).expect("read gui-state.yaml");
    let mut config: AppConfig = serde_yaml::from_str(&content).expect("parse gui-state.yaml");
    let before = config.nodes.len();
    let removed = dedup_nodes(&mut config.nodes);
    let after = config.nodes.len();
    println!("nodes: {} -> {} (removed {})", before, after, removed);
    if removed > 0 {
        let out = serde_yaml::to_string(&config).expect("serialize");
        std::fs::write(&path, out).expect("write gui-state.yaml");
        println!("written to {}", path.display());
    } else {
        println!("no duplicates, file untouched");
    }
}

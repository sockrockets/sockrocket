//! Shared helpers for loading secrets from `.local/` (gitignored).

use std::collections::HashMap;
use std::path::PathBuf;

pub fn local_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("SOCKROCKET_LOCAL_DIR") {
        return PathBuf::from(dir);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.local")
}

/// Load KEY=VALUE pairs from `.local/<name>`, then overlay process env.
pub fn load_env_file(name: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let path = local_dir().join(name);
    if let Ok(text) = std::fs::read_to_string(&path) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                map.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    for (k, v) in std::env::vars() {
        map.insert(k, v);
    }
    map
}

pub fn require(map: &HashMap<String, String>, key: &str) -> String {
    map.get(key)
        .cloned()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            panic!(
                "missing {key}; copy .local.example/ to .local/ and fill values \
                 (or export {key})"
            )
        })
}

pub fn optional(map: &HashMap<String, String>, key: &str) -> Option<String> {
    map.get(key).cloned().filter(|s| !s.is_empty())
}

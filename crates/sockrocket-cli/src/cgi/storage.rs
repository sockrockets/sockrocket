//! Cross-process configuration transactions and durable atomic replacement.
use super::*;

pub(super) struct MutationGuard {
    _guard: sockrocket_core::config::storage::MutationGuard,
}
impl MutationGuard {
    pub fn acquire() -> Result<Self> {
        let guard = sockrocket_core::config::storage::MutationGuard::acquire(&conf_path())?;
        anyhow::ensure!(!sockrocket_dir().join(".update-transaction").exists(),
            "软件更新中，暂不能修改配置，请稍后重试");
        Ok(Self { _guard: guard })
    }
}

pub(super) fn atomic_write(text: &str) -> Result<()> {
    sockrocket_core::config::storage::atomic_write_private(&conf_path(), text, true)
}

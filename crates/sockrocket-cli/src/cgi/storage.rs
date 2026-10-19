//! Cross-process configuration transactions and durable atomic replacement.
use super::*;

pub(super) struct MutationGuard {
    _guard: sockrocket_core::config::storage::MutationGuard,
}
impl MutationGuard {
    pub fn acquire() -> Result<Self> {
        Ok(Self {
            _guard: sockrocket_core::config::storage::MutationGuard::acquire(&conf_path())?,
        })
    }
}

pub(super) fn atomic_write(text: &str) -> Result<()> {
    sockrocket_core::config::storage::atomic_write_private(&conf_path(), text, true)
}

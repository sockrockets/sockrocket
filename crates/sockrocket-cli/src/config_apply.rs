//! One observable lifecycle for every configuration reload, including failures.
//! Revisions are process-local counters and contain no configuration secrets.
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(super) struct ApplyStatus {
    pub observed_revision: u64,
    pub runtime_revision: u64,
    pub state: Phase,
    pub requires_restart: Vec<String>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Phase {
    #[default]
    Applied,
    Validating,
    Rejected,
    RestartRequired,
    PolicyPending,
    PolicyFailed,
}

impl ApplyStatus {
    fn begin(&mut self) -> u64 {
        self.observed_revision += 1;
        self.state = Phase::Validating;
        self.observed_revision
    }
    fn publish(&mut self, revision: u64, restart: Vec<String>, policy: bool) {
        if revision != self.observed_revision {
            return;
        }
        self.runtime_revision = revision;
        self.requires_restart = restart;
        self.state = if policy {
            Phase::PolicyPending
        } else if self.requires_restart.is_empty() {
            Phase::Applied
        } else {
            Phase::RestartRequired
        };
    }
    fn finish(&mut self, revision: u64, success: bool, published: bool) {
        if revision != self.observed_revision {
            return;
        }
        self.state = if !success {
            if published {
                Phase::PolicyFailed
            } else {
                Phase::Rejected
            }
        } else if self.requires_restart.is_empty() {
            Phase::Applied
        } else {
            Phase::RestartRequired
        };
    }
}

pub(super) struct ApplyAttempt {
    revision: u64,
    restart: Vec<String>,
    published: bool,
    finished: bool,
}
impl ApplyAttempt {
    pub fn begin(restart: &[String]) -> Self {
        let mut revision = 0;
        super::update_health_state(|s| revision = s.config_apply.begin());
        Self {
            revision,
            restart: restart
                .iter()
                .filter(|field| field.as_str() != "ipv6_mode")
                .cloned()
                .collect(),
            published: false,
            finished: false,
        }
    }
    pub fn runtime_applied(&mut self, policy_pending: bool) {
        self.published = true;
        super::update_health_state(|s| {
            s.config_apply
                .publish(self.revision, self.restart.clone(), policy_pending)
        });
        self.finished = !policy_pending;
    }
    pub fn policy_finished(mut self, success: bool) {
        super::update_health_state(|s| s.config_apply.finish(self.revision, success, true));
        self.finished = true;
    }
}
impl Drop for ApplyAttempt {
    fn drop(&mut self) {
        if !self.finished {
            super::update_health_state(|s| {
                s.config_apply.finish(self.revision, false, self.published)
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejected_revision_retains_previous_runtime_and_restart_requirement() {
        let mut s = ApplyStatus::default();
        let first = s.begin();
        s.publish(first, vec!["socks_port".into()], false);
        assert_eq!(s.state, Phase::RestartRequired);
        let bad = s.begin();
        s.finish(bad, false, false);
        assert_eq!(s.state, Phase::Rejected);
        assert_eq!(s.runtime_revision, first);
        assert_eq!(s.requires_restart, vec!["socks_port"]);
    }
    #[test]
    fn late_policy_completion_cannot_acknowledge_a_newer_configuration() {
        let mut s = ApplyStatus::default();
        let old = s.begin();
        s.publish(old, vec![], true);
        assert_eq!(s.state, Phase::PolicyPending);
        let new = s.begin();
        s.publish(new, vec![], true);
        s.finish(old, true, true);
        assert_eq!(s.state, Phase::PolicyPending);
        s.finish(new, false, true);
        assert_eq!(s.state, Phase::PolicyFailed);
        assert_eq!(s.runtime_revision, new);
    }
}

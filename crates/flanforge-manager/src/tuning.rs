use std::time::Duration;

/// How much finished history stays resident. Unfinished work is kept on top of
/// this bound, never counted against it.
pub(super) const IN_MEMORY_ALLOCATION_LIMIT: usize = 1_000;

/// How long teardown waits between the cleanup attempts it retries.
const CLEANUP_RETRY_DELAY: Duration = Duration::from_secs(1);

/// The knobs production always leaves at their defaults, injectable so a test
/// can prove a rule without seeding a full history window or spending the
/// wall clock the rule costs in production.
#[derive(Clone, Copy, Debug)]
pub(super) struct ManagerTuning {
    pub(super) history_limit: usize,
    pub(super) cleanup_retry_delay: Duration,
}

impl Default for ManagerTuning {
    fn default() -> Self {
        Self {
            history_limit: IN_MEMORY_ALLOCATION_LIMIT,
            cleanup_retry_delay: CLEANUP_RETRY_DELAY,
        }
    }
}

#[cfg(test)]
impl ManagerTuning {
    /// The production retry count and ordering with a negligible pause between
    /// attempts, so a test asserts the same attempts in a fraction of a second.
    pub(crate) fn fast_cleanup_retries() -> Self {
        Self {
            cleanup_retry_delay: Duration::from_millis(1),
            ..Self::default()
        }
    }
}

use std::time::Duration;

/// How long one teardown may take: the allowance configuration gives it, and
/// the shutdown grace it must not outlive once a stop is running.
///
/// Every teardown path shares this — an allocation's cleanup, a reaper
/// deletion, and an interrupted staging image's removal — so a backend has one
/// clock to read rather than a timeout per caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CleanupBudget {
    allowance: Duration,
    deadline: Option<tokio::time::Instant>,
}

impl CleanupBudget {
    /// The configured allowance, with no deadline over it.
    #[must_use]
    pub const fn allow(allowance: Duration) -> Self {
        Self {
            allowance,
            deadline: None,
        }
    }

    /// The same allowance, capped by a deadline it must not outlive.
    #[must_use]
    pub const fn until(self, deadline: tokio::time::Instant) -> Self {
        Self {
            allowance: self.allowance,
            deadline: Some(deadline),
        }
    }

    /// The same budget with its allowance raised to a backend's floor. A
    /// backend that cannot stop, undefine, and delete volumes in the
    /// configured time says so here; a shutdown deadline still caps the result.
    #[must_use]
    pub fn at_least(self, floor: Duration) -> Self {
        Self {
            allowance: self.allowance.max(floor),
            deadline: self.deadline,
        }
    }

    /// Does a shutdown deadline cap this teardown?
    #[must_use]
    pub const fn is_bounded(&self) -> bool {
        self.deadline.is_some()
    }

    /// The whole configured allowance, before any deadline.
    #[must_use]
    pub const fn allowance(&self) -> Duration {
        self.allowance
    }

    /// What the deadline still allows, or `None` when nothing bounds it.
    #[must_use]
    pub fn remaining(&self) -> Option<Duration> {
        self.deadline
            .map(|deadline| deadline.saturating_duration_since(tokio::time::Instant::now()))
    }

    /// `duration`, shortened to what the deadline still allows. For a step
    /// that needs something other than the whole allowance.
    #[must_use]
    pub fn limit(&self, duration: Duration) -> Duration {
        self.remaining()
            .map_or(duration, |remaining| remaining.min(duration))
    }

    /// The timeout one teardown gets if it starts now.
    #[must_use]
    pub fn timeout(&self) -> Duration {
        self.limit(self.allowance)
    }
}

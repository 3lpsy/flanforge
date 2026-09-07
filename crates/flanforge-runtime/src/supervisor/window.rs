use std::time::Duration;

use flanforge_core::{Allocation, AllocationState, Profile};

use flanforge_forgejo::{ForgejoError, RunnerStatus};
use flanforge_manager::WorkerError;

use crate::channel::GuestExit;

/// How long a runner Forgejo has already released may keep shutting down
/// before it is killed, so an interrupted job still gets to finish. It stays a
/// constant because no profile budget describes a host-side shutdown wait, and
/// it holds a VM slot; the supervised lifetime caps it either way.
pub(crate) const RELEASED_RUNNER_GRACE: Duration = Duration::from_mins(1);

/// The floor under the idle budget supervision inherits, so a job bound at the
/// very end of the idle TTL still gets a bounded chance to be accepted. It is a
/// constant for the same reason `RELEASED_RUNNER_GRACE` is — no profile budget
/// describes it — and the idle TTL itself caps it, so no allocation waits for a
/// job longer than the one budget the operator configured.
const POST_BINDING_GRACE: Duration = Duration::from_mins(1);

/// How far the supervised runner has been observed to get. It only moves
/// forward, so a late or repeated status cannot ask the allocation to go back
/// to a state it has already left.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ObservedPhase {
    Waiting,
    Ready,
    Running,
}

/// Deadline bookkeeping for one supervised guest runner.
#[derive(Debug)]
pub struct SupervisionWindow {
    idle_timeout: Duration,
    job_timeout: Duration,
    idle_deadline: tokio::time::Instant,
    supervised_deadline: tokio::time::Instant,
    job_deadline: Option<tokio::time::Instant>,
    phase: ObservedPhase,
    observation_failures: u32,
}

impl SupervisionWindow {
    /// `idle_deadline` is what waiting for the job left of the idle TTL, so one
    /// idle budget spans both waits rather than one per wait.
    #[must_use]
    pub fn new(profile: &Profile, idle_deadline: tokio::time::Instant) -> Self {
        let now = tokio::time::Instant::now();
        let idle_timeout = Duration::from_secs(profile.idle_timeout_seconds);
        let job_timeout = Duration::from_secs(profile.job_timeout_seconds);
        Self {
            idle_timeout,
            job_timeout,
            idle_deadline: idle_deadline.max(now + idle_timeout.min(POST_BINDING_GRACE)),
            // An unobservable runner is still bounded by the configured job TTL.
            supervised_deadline: now + job_timeout,
            job_deadline: None,
            phase: ObservedPhase::Waiting,
            observation_failures: 0,
        }
    }

    #[must_use]
    pub fn is_running(&self) -> bool {
        self.phase == ObservedPhase::Running
    }

    /// The states one observed status should report, in order. A status is
    /// information, not an instruction: an observation that would move the
    /// allocation backwards reports nothing.
    pub fn advance(&mut self, status: RunnerStatus) -> &'static [AllocationState] {
        match (status, self.phase) {
            (RunnerStatus::Idle, ObservedPhase::Waiting) => {
                self.phase = ObservedPhase::Ready;
                &[AllocationState::Ready]
            }
            (RunnerStatus::Active, ObservedPhase::Waiting) => {
                self.start_job();
                &[AllocationState::Ready, AllocationState::Running]
            }
            (RunnerStatus::Active, ObservedPhase::Ready) => {
                self.start_job();
                &[AllocationState::Running]
            }
            _ => &[],
        }
    }

    fn start_job(&mut self) {
        self.phase = ObservedPhase::Running;
        self.observation_failures = 0;
        self.job_deadline = Some(tokio::time::Instant::now() + self.job_timeout);
    }

    pub fn observed(&mut self) {
        self.observation_failures = 0;
    }

    /// Records a failed observation. The idle window is held open unless
    /// Forgejo refused the request: an unreachable or unreadable Forgejo is not
    /// evidence of an idle runner, but a rejection never becomes a job.
    pub fn unobserved(&mut self, error: ForgejoError) -> u32 {
        self.observation_failures = self.observation_failures.saturating_add(1);
        if !error.is_rejection() {
            self.idle_deadline = tokio::time::Instant::now() + self.idle_timeout;
        }
        self.observation_failures
    }

    /// How long a released runner may still shut down: the grace period, or
    /// the supervised lifetime it can never outlive, whichever comes first.
    pub(super) fn released_deadline(&self, now: tokio::time::Instant) -> tokio::time::Instant {
        (now + RELEASED_RUNNER_GRACE).min(self.supervised_deadline)
    }

    /// A lost observation is not an exit. The process may still be running, and
    /// treating it as one would run the retention pre-capture gate over a live
    /// job. Supervision fails instead, which skips retention and lets cleanup
    /// destroy the guest.
    pub(super) fn ensure_exit_is_a_verdict(
        &self,
        exit: GuestExit,
        allocation: &Allocation,
    ) -> Result<(), WorkerError> {
        if exit.is_verdict() {
            return self.ensure_exit_completed_a_job(exit, allocation);
        }
        tracing::warn!(allocation_id = %allocation.id, "guest runner became unobservable");
        Err(WorkerError::new("guest runner became unobservable"))
    }

    /// The one rule for an exited guest runner. A runner that got as far as a
    /// job owns no verdict on it — the job's own result belongs to Forgejo —
    /// so only one that never reached a job can fail the allocation here.
    fn ensure_exit_completed_a_job(
        &self,
        exit: GuestExit,
        allocation: &Allocation,
    ) -> Result<(), WorkerError> {
        if exit.is_success() {
            tracing::info!(allocation_id = %allocation.id, "guest runner exited successfully");
            return Ok(());
        }
        if self.is_running() {
            tracing::warn!(allocation_id = %allocation.id, exit_code = exit.code(), "guest runner exited unsuccessfully after accepting its job");
            return Ok(());
        }
        tracing::warn!(allocation_id = %allocation.id, exit_code = exit.code(), "guest runner exited before accepting a job");
        Err(WorkerError::new(
            "guest runner exited before completing a job",
        ))
    }

    #[must_use]
    pub fn overrun(&self, now: tokio::time::Instant) -> Option<&'static str> {
        if now >= self.supervised_deadline {
            return Some("guest runner exceeded its supervised lifetime");
        }
        if self.job_deadline.is_some_and(|deadline| now >= deadline) {
            return Some("guest job exceeded its timeout");
        }
        if !self.is_running() && now >= self.idle_deadline {
            return Some("guest runner did not receive a job before its idle timeout");
        }
        None
    }
}

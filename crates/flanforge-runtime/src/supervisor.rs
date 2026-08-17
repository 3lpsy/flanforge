use std::{process::ExitStatus, time::Duration};

use flanforge_core::{Allocation, AllocationState, Profile};
use tokio::process::Child;
use tokio_util::sync::CancellationToken;

use flanforge_forgejo::{ForgejoError, RunnerStatus};
use flanforge_manager::{AllocationReporter, WorkerError};

use super::worker::FlanForgeWorker;

/// How long a runner Forgejo has already released may keep shutting down
/// before it is killed, so an interrupted job still gets to finish. It stays a
/// constant because no profile budget describes a host-side shutdown wait, and
/// it holds a VM slot; the supervised lifetime caps it either way.
const RELEASED_RUNNER_GRACE: Duration = Duration::from_mins(1);

/// Deadline bookkeeping for one supervised guest runner.
pub(super) struct SupervisionWindow {
    idle_timeout: Duration,
    job_timeout: Duration,
    idle_deadline: tokio::time::Instant,
    supervised_deadline: tokio::time::Instant,
    job_deadline: Option<tokio::time::Instant>,
    observation_failures: u32,
}

impl SupervisionWindow {
    pub(super) fn new(profile: &Profile) -> Self {
        let now = tokio::time::Instant::now();
        let idle_timeout = Duration::from_secs(profile.idle_timeout_seconds);
        let job_timeout = Duration::from_secs(profile.job_timeout_seconds);
        Self {
            idle_timeout,
            job_timeout,
            idle_deadline: now + idle_timeout,
            // An unobservable runner is still bounded by the configured job TTL.
            supervised_deadline: now + job_timeout,
            job_deadline: None,
            observation_failures: 0,
        }
    }

    pub(super) fn is_running(&self) -> bool {
        self.job_deadline.is_some()
    }

    pub(super) fn start_job(&mut self) {
        self.observation_failures = 0;
        self.job_deadline = Some(tokio::time::Instant::now() + self.job_timeout);
    }

    pub(super) fn observed(&mut self) {
        self.observation_failures = 0;
    }

    /// Records a failed observation. The idle window is held open unless
    /// Forgejo refused the request: an unreachable or unreadable Forgejo is not
    /// evidence of an idle runner, but a rejection never becomes a job.
    pub(super) fn unobserved(&mut self, error: ForgejoError) -> u32 {
        self.observation_failures = self.observation_failures.saturating_add(1);
        if !error.is_rejection() {
            self.idle_deadline = tokio::time::Instant::now() + self.idle_timeout;
        }
        self.observation_failures
    }

    /// How long a released runner may still shut down: the grace period, or
    /// the supervised lifetime it can never outlive, whichever comes first.
    fn released_deadline(&self, now: tokio::time::Instant) -> tokio::time::Instant {
        (now + RELEASED_RUNNER_GRACE).min(self.supervised_deadline)
    }

    /// The one rule for an exited guest runner. A runner that got as far as a
    /// job owns no verdict on it — the job's own result belongs to Forgejo —
    /// so only one that never reached a job can fail the allocation here.
    fn ensure_exit_completed_a_job(
        &self,
        status: ExitStatus,
        allocation: &Allocation,
    ) -> Result<(), WorkerError> {
        if status.success() {
            tracing::info!(allocation_id = %allocation.id, "guest runner exited successfully");
            return Ok(());
        }
        if self.is_running() {
            tracing::warn!(allocation_id = %allocation.id, exit_code = status.code(), "guest runner exited unsuccessfully after accepting its job");
            return Ok(());
        }
        tracing::warn!(allocation_id = %allocation.id, exit_code = status.code(), "guest runner exited before accepting a job");
        Err(WorkerError::new(
            "guest runner exited before completing a job",
        ))
    }

    pub(super) fn overrun(&self, now: tokio::time::Instant) -> Option<&'static str> {
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

impl FlanForgeWorker {
    pub(super) async fn supervise(
        &self,
        mut child: Child,
        allocation: &Allocation,
        profile: &Profile,
        reporter: &AllocationReporter,
        cancellation: &CancellationToken,
    ) -> Result<(), WorkerError> {
        let mut window = SupervisionWindow::new(profile);
        let mut is_ready = false;
        loop {
            if cancellation.is_cancelled() {
                let _ = child.kill().await;
                tracing::warn!(allocation_id = %allocation.id, "guest runner cancelled");
                return Err(WorkerError::new("allocation was cancelled"));
            }
            if let Some(status) = child
                .try_wait()
                .map_err(|_| WorkerError::new("cannot inspect guest runner"))?
            {
                return window.ensure_exit_completed_a_job(status, allocation);
            }
            match self
                .forgejo
                .runner_status(
                    &allocation.request.repository,
                    allocation.runner_id.unwrap_or_default(),
                )
                .await
            {
                Ok(status) => {
                    window.observed();
                    match status {
                        RunnerStatus::Idle if !is_ready => {
                            reporter
                                .transition(AllocationState::Ready)
                                .await
                                .map_err(|error| WorkerError::new(error.to_string()))?;
                            is_ready = true;
                            tracing::info!(allocation_id = %allocation.id, "guest runner is ready");
                        }
                        RunnerStatus::Active if !window.is_running() => {
                            if !is_ready {
                                reporter
                                    .transition(AllocationState::Ready)
                                    .await
                                    .map_err(|error| WorkerError::new(error.to_string()))?;
                            }
                            reporter
                                .transition(AllocationState::Running)
                                .await
                                .map_err(|error| WorkerError::new(error.to_string()))?;
                            window.start_job();
                            tracing::info!(allocation_id = %allocation.id, "guest runner accepted its job");
                        }
                        _ => {}
                    }
                }
                // Forgejo deleted the registration, so no later poll can say
                // anything more about this runner. A job can begin and end
                // between two polls, so an unobserved job is not no job: only
                // the child's own shutdown decides the allocation from here.
                Err(ForgejoError::Absent) => {
                    tracing::warn!(allocation_id = %allocation.id, observed_job = window.is_running(), "Forgejo released the runner registration");
                    return self
                        .ensure_released_runner_exits(&mut child, &window, allocation, cancellation)
                        .await;
                }
                Err(error) => {
                    let observation_failures = window.unobserved(error);
                    tracing::warn!(allocation_id = %allocation.id, %error, observation_failures, "cannot observe Forgejo runner status");
                }
            }
            if let Some(message) = window.overrun(tokio::time::Instant::now()) {
                let _ = child.kill().await;
                tracing::warn!(allocation_id = %allocation.id, message, "guest runner supervision ended");
                return Err(WorkerError::new(message));
            }
            tokio::time::sleep(self.poll).await;
        }
    }

    /// Waits out a runner whose registration Forgejo already deleted. Its job
    /// is over from Forgejo's side, so only its shutdown is still owed time.
    async fn ensure_released_runner_exits(
        &self,
        child: &mut Child,
        window: &SupervisionWindow,
        allocation: &Allocation,
        cancellation: &CancellationToken,
    ) -> Result<(), WorkerError> {
        let deadline = window.released_deadline(tokio::time::Instant::now());
        loop {
            if cancellation.is_cancelled() {
                let _ = child.kill().await;
                tracing::warn!(allocation_id = %allocation.id, "guest runner cancelled");
                return Err(WorkerError::new("allocation was cancelled"));
            }
            if let Some(status) = child
                .try_wait()
                .map_err(|_| WorkerError::new("cannot inspect guest runner"))?
            {
                return window.ensure_exit_completed_a_job(status, allocation);
            }
            if tokio::time::Instant::now() >= deadline {
                let _ = child.kill().await;
                tracing::warn!(allocation_id = %allocation.id, "guest runner did not shut down before its released-runner deadline");
                return Err(WorkerError::new(
                    "guest runner did not exit after Forgejo released its registration",
                ));
            }
            tokio::time::sleep(self.poll).await;
        }
    }
}

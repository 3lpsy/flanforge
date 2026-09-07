use flanforge_core::{Allocation, Profile};
use tokio_util::sync::CancellationToken;

use flanforge_forgejo::ForgejoError;
use flanforge_manager::{AllocationReporter, WorkerError};

use super::SupervisionWindow;
use crate::{
    channel::GuestProcess,
    job::{GuestJob, StartedRunner},
};

impl GuestJob {
    /// # Errors
    /// Returns an error when the guest runner overruns a supervision deadline,
    /// exits without completing a job, or the supervision is cancelled.
    pub async fn supervise(
        &self,
        started: StartedRunner,
        allocation: &Allocation,
        profile: &Profile,
        reporter: &AllocationReporter,
        cancellation: &CancellationToken,
    ) -> Result<(), WorkerError> {
        let StartedRunner {
            mut process,
            idle_deadline,
        } = started;
        let mut window = SupervisionWindow::new(profile, idle_deadline);
        loop {
            if cancellation.is_cancelled() {
                process.ensure_stopped().await;
                tracing::warn!(allocation_id = %allocation.id, "guest runner cancelled");
                return Err(WorkerError::new("allocation was cancelled"));
            }
            if let Some(exit) = process.try_exit().await? {
                return window.ensure_exit_is_a_verdict(exit, allocation);
            }
            // Shutdown charges cancellation latency to its grace, so neither
            // the Forgejo call nor the poll interval may outlive the token.
            let observed = tokio::select! {
                () = cancellation.cancelled() => continue,
                observed = self.forgejo_client().runner_status(
                    &allocation.request.repository,
                    allocation.runner_id.unwrap_or_default(),
                ) => observed,
            };
            match observed {
                Ok(status) => {
                    window.observed();
                    for state in window.advance(status) {
                        reporter
                            .transition(*state)
                            .await
                            .map_err(|error| WorkerError::new(error.to_string()))?;
                        tracing::info!(allocation_id = %allocation.id, ?status, ?state, "guest runner status advanced the allocation");
                    }
                }
                // Forgejo deleted the registration, so no later poll can say
                // anything more about this runner. A job can begin and end
                // between two polls, so an unobserved job is not no job: only
                // the runner's own shutdown decides the allocation from here.
                Err(ForgejoError::Absent) => {
                    tracing::warn!(allocation_id = %allocation.id, observed_job = window.is_running(), "Forgejo released the runner registration");
                    return self
                        .ensure_released_runner_exits(
                            &mut process,
                            &window,
                            allocation,
                            cancellation,
                        )
                        .await;
                }
                Err(error) => {
                    let observation_failures = window.unobserved(error);
                    tracing::warn!(allocation_id = %allocation.id, %error, observation_failures, "cannot observe Forgejo runner status");
                }
            }
            if let Some(message) = window.overrun(tokio::time::Instant::now()) {
                process.ensure_stopped().await;
                tracing::warn!(allocation_id = %allocation.id, message, "guest runner supervision ended");
                return Err(WorkerError::new(message));
            }
            tokio::select! {
                () = cancellation.cancelled() => {}
                () = tokio::time::sleep(self.poll()) => {}
            }
        }
    }

    /// Waits out a runner whose registration Forgejo already deleted. Its job
    /// is over from Forgejo's side, so only its shutdown is still owed time.
    async fn ensure_released_runner_exits(
        &self,
        process: &mut Box<dyn GuestProcess>,
        window: &SupervisionWindow,
        allocation: &Allocation,
        cancellation: &CancellationToken,
    ) -> Result<(), WorkerError> {
        let deadline = window.released_deadline(tokio::time::Instant::now());
        loop {
            // This window is short and every iteration decides whether the
            // allocation fails, so a rate-limited channel is asked to probe now
            // rather than on its own schedule.
            process.probe_now();
            if cancellation.is_cancelled() {
                process.ensure_stopped().await;
                tracing::warn!(allocation_id = %allocation.id, "guest runner cancelled");
                return Err(WorkerError::new("allocation was cancelled"));
            }
            if let Some(exit) = process.try_exit().await? {
                return window.ensure_exit_is_a_verdict(exit, allocation);
            }
            if tokio::time::Instant::now() >= deadline {
                process.ensure_stopped().await;
                tracing::warn!(allocation_id = %allocation.id, "guest runner did not shut down before its released-runner deadline");
                return Err(WorkerError::new(
                    "guest runner did not exit after Forgejo released its registration",
                ));
            }
            tokio::select! {
                () = cancellation.cancelled() => {}
                () = tokio::time::sleep(self.poll()) => {}
            }
        }
    }
}

use std::time::Duration;

use flanforge_core::{Allocation, AllocationState, Profile};
use flanforge_forgejo::RunnerCredentials;
use flanforge_manager::{AllocationReporter, WorkerError};
use tokio_util::sync::CancellationToken;

use super::{GuestJob, RunnerDelivery, StartedRunner};
use crate::{
    guest::{GuestSession, RunnerSpawn},
    supervisor::RELEASED_RUNNER_GRACE,
};

impl GuestJob {
    /// Registers, prepares, and starts the one-job runner in a ready guest.
    ///
    /// # Errors
    ///
    /// Returns an error when registration, delivery, authorization, or startup fails.
    pub async fn start_runner(
        &self,
        allocation: &mut Allocation,
        profile: &Profile,
        reporter: &AllocationReporter,
        cancellation: &CancellationToken,
        session: &GuestSession,
    ) -> Result<StartedRunner, WorkerError> {
        self.ensure_session(session)?;
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(profile.boot_timeout_seconds);
        let credentials = self
            .register(allocation, reporter, cancellation, deadline)
            .await?;
        Self::phase(
            cancellation,
            deadline,
            "guest runner preparation exceeded its timeout",
            async {
                match self.delivery() {
                    RunnerDelivery::HostCopy(source) => {
                        self.guest().stage_runner(session, source).await
                    }
                    RunnerDelivery::Image => self.guest().ensure_runner_available(session).await,
                }
            },
        )
        .await?;
        reporter
            .transition(AllocationState::WaitingForJob)
            .await
            .map_err(|error| WorkerError::new(error.to_string()))?;
        // One idle budget covers waiting for the job and waiting for the runner
        // to take it, so a never-worked allocation costs one idle TTL, not two.
        let idle_deadline =
            tokio::time::Instant::now() + Duration::from_secs(profile.idle_timeout_seconds);
        let handle = self
            .wait_for_job_handle(allocation, profile, cancellation, idle_deadline)
            .await?;
        tracing::info!(allocation_id = %allocation.id, "authorized Forgejo job is ready");
        let spawn_deadline =
            tokio::time::Instant::now() + Duration::from_secs(profile.cleanup_timeout_seconds);
        let server_url = self.forgejo_client().server_url();
        let request = RunnerSpawn {
            server_url: server_url.as_str(),
            credentials: &credentials,
            label: allocation.runner_label.as_str(),
            handle: &handle,
            allocation_id: allocation.id.into_uuid(),
            lifetime: guest_lifetime(profile),
        };
        let process = Self::phase(
            cancellation,
            spawn_deadline,
            "guest runner start exceeded its timeout",
            self.guest().spawn_session_runner(session, &request),
        )
        .await?;
        Ok(StartedRunner {
            process,
            idle_deadline,
        })
    }

    /// Registers one ephemeral runner and records it on the allocation before
    /// anything in the guest can depend on it.
    async fn register(
        &self,
        allocation: &mut Allocation,
        reporter: &AllocationReporter,
        cancellation: &CancellationToken,
        deadline: tokio::time::Instant,
    ) -> Result<RunnerCredentials, WorkerError> {
        let runner_name = format!("flanforged-{}", allocation.id);
        tracing::info!(allocation_id = %allocation.id, "registering ephemeral Forgejo runner");
        let credentials = Self::phase(
            cancellation,
            deadline,
            "runner registration exceeded its timeout",
            async {
                self.forgejo_client()
                    .create_runner(&allocation.request.repository, &runner_name)
                    .await
                    .map_err(|error| WorkerError::new(error.to_string()))
            },
        )
        .await?;
        if let Err(error) = reporter.set_runner_id(credentials.id).await {
            let _ = self
                .forgejo_client()
                .delete_runner(&allocation.request.repository, credentials.id)
                .await;
            return Err(WorkerError::new(error.to_string()));
        }
        allocation.set_runner_id(credentials.id);
        Ok(credentials)
    }
}

/// The guest-side backstop for a daemon that never comes back: the job budget
/// the operator configured, plus the grace a released runner is allowed, so a
/// guest never ends a job the daemon is still legitimately supervising.
fn guest_lifetime(profile: &Profile) -> Duration {
    Duration::from_secs(profile.job_timeout_seconds).saturating_add(RELEASED_RUNNER_GRACE)
}

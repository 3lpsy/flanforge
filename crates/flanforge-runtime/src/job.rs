use flanforge_core::{Allocation, Profile};
use tokio_util::sync::CancellationToken;

use flanforge_manager::WorkerError;

use super::worker::FlanForgeWorker;

impl FlanForgeWorker {
    pub(super) async fn wait_for_job_handle(
        &self,
        allocation: &Allocation,
        profile: &Profile,
        cancellation: &CancellationToken,
        deadline: tokio::time::Instant,
    ) -> Result<String, WorkerError> {
        Self::phase(
            cancellation,
            deadline,
            "authorized Forgejo job did not become ready",
            async {
                loop {
                    let handle = self
                        .forgejo
                        .job_handle(
                            &allocation.request.repository,
                            allocation.runner_label.as_str(),
                            &profile.job_name,
                            allocation.request.run_attempt,
                        )
                        .await;
                    match handle {
                        Ok(Some(handle)) => return Ok(handle),
                        Ok(None) => tracing::trace!(allocation_id = %allocation.id, "authorized Forgejo job is not waiting yet"),
                        Err(error) if error.is_retryable() => {
                            tracing::debug!(allocation_id = %allocation.id, %error, "Forgejo unavailable while polling for job");
                        }
                        Err(error) => return Err(WorkerError::new(error.to_string())),
                    }
                    tokio::time::sleep(self.poll).await;
                }
            },
        )
        .await
    }
}

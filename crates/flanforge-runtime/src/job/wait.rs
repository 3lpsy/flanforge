use flanforge_core::{Allocation, Profile};
use flanforge_forgejo::{JobBinding, JobDiagnosis};
use flanforge_manager::WorkerError;
use tokio_util::sync::CancellationToken;

use super::GuestJob;

const NOT_READY: &str = "authorized Forgejo job did not become ready";

impl GuestJob {
    /// Polls for the one job handle this allocation is authorized to run.
    ///
    /// # Errors
    /// Returns an error when the dependent job is queued with a `runs-on` the
    /// search can never return, when the handle does not appear before the
    /// deadline, or when the wait is cancelled. A timeout names the last
    /// observed reason so it is not confused with the others.
    pub async fn wait_for_job_handle(
        &self,
        allocation: &Allocation,
        profile: &Profile,
        cancellation: &CancellationToken,
        deadline: tokio::time::Instant,
    ) -> Result<String, WorkerError> {
        let binding = JobBinding {
            label: allocation.runner_label.as_str(),
            job_name: &profile.job_name,
            run_id: allocation.request.run_id,
        };
        // The borrow of `observed` ends with this block, so the timeout message
        // below can read what the wait last saw.
        let mut observed: Option<&'static str> = None;
        {
            let poll = self.poll_for_job_handle(allocation, &binding, &mut observed);
            tokio::select! {
                () = cancellation.cancelled() => {
                    return Err(WorkerError::new("allocation was cancelled"));
                }
                outcome = tokio::time::timeout_at(deadline, poll) => {
                    if let Ok(result) = outcome {
                        return result;
                    }
                }
            }
        }
        Err(WorkerError::new(observed.map_or_else(
            || NOT_READY.to_owned(),
            |reason| format!("{NOT_READY}: {reason}"),
        )))
    }

    async fn poll_for_job_handle(
        &self,
        allocation: &Allocation,
        binding: &JobBinding<'_>,
        observed: &mut Option<&'static str>,
    ) -> Result<String, WorkerError> {
        loop {
            match self
                .forgejo_client()
                .job_handle(&allocation.request.repository, binding)
                .await
            {
                Ok(Some(handle)) => return Ok(handle),
                Ok(None) => {
                    tracing::trace!(allocation_id = %allocation.id, run_id = allocation.request.run_id, run_attempt = allocation.request.run_attempt, "authorized Forgejo job is not waiting yet");
                    if let Some(error) = self.diagnose_wait(allocation, binding, observed).await {
                        return Err(error);
                    }
                }
                Err(error) if error.is_retryable() => {
                    tracing::debug!(allocation_id = %allocation.id, %error, "Forgejo unavailable while polling for job");
                }
                Err(error) => return Err(WorkerError::new(error.to_string())),
            }
            tokio::time::sleep(self.poll()).await;
        }
    }

    /// Records why the labelled search is still empty, and fails the wait only
    /// on the one answer no further polling can change. A diagnosis that cannot
    /// be read is not one: keep waiting instead.
    async fn diagnose_wait(
        &self,
        allocation: &Allocation,
        binding: &JobBinding<'_>,
        observed: &mut Option<&'static str>,
    ) -> Option<WorkerError> {
        let diagnosis = match self
            .forgejo_client()
            .job_diagnosis(&allocation.request.repository, binding)
            .await
        {
            Ok(diagnosis) => diagnosis,
            Err(error) => {
                tracing::debug!(allocation_id = %allocation.id, %error, "cannot re-read Forgejo jobs to explain the wait");
                return None;
            }
        };
        *observed = Some(diagnosis.reason());
        let JobDiagnosis::RunsOnMismatch(runs_on) = diagnosis else {
            return None;
        };
        tracing::warn!(allocation_id = %allocation.id, reason = "runs_on", labels = runs_on.len(), "dependent Forgejo job can never match the allocation label");
        Some(WorkerError::new(format!(
            "dependent Forgejo job \"{}\" is queued with runs-on [{}]; the allocation label must be its only entry (expected [\"{}\"])",
            binding.job_name,
            runs_on.join(", "),
            binding.label,
        )))
    }
}

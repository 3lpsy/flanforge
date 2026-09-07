use std::time::Instant;

use flanforge_core::{Allocation, Profile, RetentionOutcome, RetentionPhase, RetentionResult};
use tokio::process::Child;
use tokio_util::sync::CancellationToken;

use flanforge_manager::{AllocationReporter, ManagerError};

use super::{super::retention::RetentionRequest, FlanForgeWorker};

impl FlanForgeWorker {
    /// Applies every retention gate the runtime owns, then reports where the
    /// sequence stopped.
    pub(super) async fn ensure_retained(
        &self,
        allocation: &Allocation,
        profile: &Profile,
        vm: &mut Child,
        ip: &str,
        reporter: &AllocationReporter,
        cancellation: &CancellationToken,
    ) {
        if !allocation.is_retention_eligible() {
            return;
        }
        let started = Instant::now();
        let outcome = match self.retention_request(allocation, profile, reporter).await {
            Ok(request) => {
                self.retain_warm(request, vm, ip, reporter, cancellation)
                    .await
            }
            Err(RetentionRequestError::Skipped(reason)) => {
                tracing::info!(allocation_id = %allocation.id, reason, "retention skipped");
                RetentionOutcome::new(
                    RetentionResult::Skipped,
                    RetentionPhase::Verify,
                    reason,
                    None,
                )
            }
            Err(RetentionRequestError::Failed(error)) => {
                tracing::error!(allocation_id = %allocation.id, %error, "retention authority is unavailable");
                RetentionOutcome::new(
                    RetentionResult::Failed,
                    RetentionPhase::Verify,
                    "retention authority is unavailable",
                    None,
                )
            }
        }
        .with_duration(started.elapsed());
        if let Err(error) = reporter.set_retention(outcome).await {
            tracing::warn!(allocation_id = %allocation.id, %error, "cannot persist the retention outcome");
        }
    }

    /// The profile is re-read at retention time, so a reload that withdrew
    /// production stops it immediately.
    async fn retention_request<'a>(
        &self,
        allocation: &'a Allocation,
        profile: &'a Profile,
        reporter: &AllocationReporter,
    ) -> Result<RetentionRequest<'a>, RetentionRequestError> {
        let plan = reporter
            .retention_plan()
            .await
            .map_err(RetentionRequestError::Failed)?
            .ok_or(RetentionRequestError::Skipped(
                "the profile no longer authorizes production",
            ))?;
        if profile.warm_template.as_ref() != Some(&plan.warm_template) {
            return Err(RetentionRequestError::Skipped(
                "the warm image name changed while the job ran",
            ));
        }
        let base_fingerprint = allocation
            .source
            .as_ref()
            .and_then(|source| source.base_fingerprint.clone())
            .ok_or(RetentionRequestError::Skipped(
                "the base fingerprint is unknown",
            ))?;
        Ok(RetentionRequest {
            allocation,
            profile,
            warm_template: plan.warm_template,
            base_fingerprint,
            generation: plan.generation,
            previous: plan.previous,
        })
    }
}

enum RetentionRequestError {
    Skipped(&'static str),
    Failed(ManagerError),
}

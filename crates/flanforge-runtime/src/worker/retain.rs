use flanforge_core::{Allocation, Profile, RetentionOutcome, RetentionPhase, RetentionResult};
use tokio::process::Child;
use tokio_util::sync::CancellationToken;

use flanforge_manager::AllocationReporter;

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
        let outcome = match self.retention_request(allocation, profile, reporter).await {
            Ok(request) => {
                self.retain_warm(request, vm, ip, reporter, cancellation)
                    .await
            }
            Err(reason) => {
                tracing::info!(allocation_id = %allocation.id, reason, "retention skipped");
                RetentionOutcome::new(
                    RetentionResult::Skipped,
                    RetentionPhase::Strip,
                    reason,
                    None,
                )
            }
        };
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
    ) -> Result<RetentionRequest<'a>, &'static str> {
        let plan = reporter
            .retention_plan()
            .await
            .ok_or("the profile no longer authorizes production")?;
        if profile.warm_template.as_ref() != Some(&plan.warm_template) {
            return Err("the warm image name changed while the job ran");
        }
        let base_fingerprint = allocation
            .source
            .as_ref()
            .and_then(|source| source.base_fingerprint.clone())
            .ok_or("the base fingerprint is unknown")?;
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

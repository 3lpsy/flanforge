use flanforge_core::{Allocation, Profile, RetentionOutcome, RetentionPhase, RetentionResult};
use flanforge_manager::{AllocationReporter, ManagerError, WorkerError};

use crate::{
    manifest::{load, path},
    warm::WarmRequest,
};

use super::{LibvirtWorker, model::PreparedGuest};

impl LibvirtWorker {
    /// Applies every retention gate the runtime owns, then reports where the
    /// sequence stopped. Retention can never fail the allocation.
    pub(super) async fn ensure_retained(
        &self,
        allocation: &Allocation,
        profile: &Profile,
        prepared: &PreparedGuest,
        reporter: &AllocationReporter,
    ) {
        if !allocation.is_retention_eligible() {
            return;
        }
        let outcome = match self.retention_request(allocation, profile, reporter).await {
            Ok(request) => self.retain_warm(&request, prepared, reporter).await,
            Err(RetentionRefusal::Skipped(reason)) => {
                tracing::info!(allocation_id = %allocation.id, reason, "retention skipped");
                RetentionOutcome::new(
                    RetentionResult::Skipped,
                    RetentionPhase::Verify,
                    reason,
                    None,
                )
            }
            Err(RetentionRefusal::Failed(reason)) => {
                tracing::error!(allocation_id = %allocation.id, %reason, "retention authority is unavailable");
                RetentionOutcome::new(
                    RetentionResult::Failed,
                    RetentionPhase::Verify,
                    "retention authority is unavailable",
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
        profile: &Profile,
        reporter: &AllocationReporter,
    ) -> Result<WarmRequest<'a>, RetentionRefusal> {
        let plan = reporter
            .retention_plan()
            .await
            .map_err(RetentionRefusal::from)?
            .ok_or(RetentionRefusal::Skipped(
                "the profile no longer authorizes production",
            ))?;
        if profile.warm_template.as_ref() != Some(&plan.warm_template) {
            return Err(RetentionRefusal::Skipped(
                "the warm image name changed while the job ran",
            ));
        }
        let base_fingerprint = allocation
            .source
            .as_ref()
            .and_then(|source| source.base_fingerprint.clone())
            .ok_or(RetentionRefusal::Skipped("the base fingerprint is unknown"))?;
        let manifest = self.load_manifest(allocation).await?;
        Ok(WarmRequest {
            allocation,
            profile: &allocation.request.profile,
            warm_template: plan.warm_template,
            base_fingerprint,
            generation: plan.generation,
            previous: plan.previous,
            manifest,
            virtual_bytes: storage_bytes(allocation, profile)?,
        })
    }

    /// The durable ownership manifest is the only authority over which volumes
    /// this allocation owns, so the capture re-reads it rather than trusting
    /// anything carried through the job.
    async fn load_manifest(
        &self,
        allocation: &Allocation,
    ) -> Result<flanforge_libvirt_wire::OwnershipManifest, RetentionRefusal> {
        let manifest_path = path(&self.state_dir, allocation.id.into_uuid());
        tokio::task::spawn_blocking(move || load(&manifest_path))
            .await
            .map_err(|_| RetentionRefusal::Failed("ownership load task failed".to_owned()))?
            .map_err(|error| RetentionRefusal::Failed(error.to_string()))
    }
}

fn storage_bytes(allocation: &Allocation, profile: &Profile) -> Result<u64, RetentionRefusal> {
    allocation
        .size
        .map_or(profile.storage_mb, |size| size.storage_mb)
        .checked_mul(1_048_576)
        .ok_or_else(|| RetentionRefusal::Failed("guest storage size overflows bytes".to_owned()))
}

enum RetentionRefusal {
    Skipped(&'static str),
    Failed(String),
}

impl From<ManagerError> for RetentionRefusal {
    fn from(error: ManagerError) -> Self {
        Self::Failed(error.to_string())
    }
}

impl From<RetentionRefusal> for WorkerError {
    fn from(refusal: RetentionRefusal) -> Self {
        match refusal {
            RetentionRefusal::Skipped(reason) => Self::new(reason),
            RetentionRefusal::Failed(reason) => Self::new(reason),
        }
    }
}

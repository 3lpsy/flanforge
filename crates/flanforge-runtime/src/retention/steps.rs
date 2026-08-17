use flanforge_core::{
    Allocation, RetentionOutcome, RetentionPhase, RetentionResult, VmName, WarmImageRecord,
    WarmImageState,
};
use tokio::process::Child;

use flanforge_manager::{AllocationReporter, WorkerError};

use super::{
    super::worker::FlanForgeWorker,
    request::{Names, RetentionRequest, Stopped, outcome, unix_time},
};

impl FlanForgeWorker {
    /// Replaces the implicit kill-on-drop with an ordered stop, so the staged
    /// image is captured from a flushed disk.
    pub(super) async fn stop_guest(
        &self,
        allocation: &Allocation,
        vm: &mut Child,
        deadline: tokio::time::Instant,
    ) -> Result<(), WorkerError> {
        self.tart
            .ensure_stopped(allocation, deadline - tokio::time::Instant::now())
            .await?;
        let _ = tokio::time::timeout_at(deadline, vm.wait()).await;
        Ok(())
    }

    pub(super) async fn ensure_staged(&self, staging: &VmName) -> Result<(), WorkerError> {
        if self.tart.is_stopped_image(staging).await {
            Ok(())
        } else {
            Err(WorkerError::new(
                "staged warm image is missing or not stopped",
            ))
        }
    }

    /// Retires the live image before publishing, so a full disk fails while the
    /// current generation is still intact.
    pub(super) async fn retire(
        &self,
        request: &RetentionRequest<'_>,
        names: &Names,
    ) -> Result<(), WorkerError> {
        self.tart
            .delete_image(&names.previous, Some(request.profile), None)
            .await?;
        if !self.tart.is_absent_image(&names.warm).await {
            self.tart
                .clone_image(&names.warm, &names.previous, request.profile, None)
                .await?;
            self.tart
                .delete_image(&names.warm, Some(request.profile), None)
                .await?;
        }
        Ok(())
    }

    pub(super) async fn publish(
        &self,
        request: &RetentionRequest<'_>,
        names: &Names,
    ) -> Result<(), WorkerError> {
        self.tart
            .clone_image(&names.staging, &names.warm, request.profile, None)
            .await?;
        self.tart
            .delete_image(&names.staging, Some(request.profile), None)
            .await
    }

    pub(super) async fn record(
        &self,
        request: &RetentionRequest<'_>,
        names: &Names,
        reporter: &AllocationReporter,
        state: WarmImageState,
    ) -> Result<(), WorkerError> {
        reporter
            .record_warm_image(WarmImageRecord {
                profile: request.allocation.request.profile.clone(),
                warm_template: names.warm.clone(),
                generation: request.generation,
                base_fingerprint: request.base_fingerprint.clone(),
                produced_by: request.allocation.id,
                produced_at_unix: unix_time(),
                state,
                previous: request.previous.clone(),
            })
            .await
            .map_err(|error| WorkerError::new(error.to_string()))
    }

    /// A failure after the live image was retired restores the previous
    /// generation; anything earlier leaves the current image untouched.
    pub(super) async fn rollback(
        &self,
        request: &RetentionRequest<'_>,
        names: &Names,
        stopped: Stopped<'_>,
        reporter: &AllocationReporter,
    ) -> RetentionOutcome {
        let (phase, error, deadline) = (stopped.phase, stopped.error, stopped.deadline);
        self.drop_candidate(request, names, deadline).await;
        if !is_live_image_removed(phase) {
            return outcome(
                RetentionResult::Failed,
                phase,
                error.to_string(),
                request.generation,
            );
        }
        // Recovery reads a staging record with no candidate as a promotion that
        // finished, so it must name the generation the image actually is.
        self.revert_record(request, names, reporter).await;
        self.restore_outcome(request, names, stopped).await
    }

    pub(super) async fn drop_candidate(
        &self,
        request: &RetentionRequest<'_>,
        names: &Names,
        deadline: tokio::time::Instant,
    ) {
        let _ = tokio::time::timeout_at(
            deadline,
            self.tart
                .delete_image(&names.staging, Some(request.profile), None),
        )
        .await;
    }

    /// Reports `RolledBack` only when the previous generation is live again;
    /// an absent, failed, or timed-out restore is a failure, because that is
    /// the outcome an operator has to act on.
    pub(super) async fn restore_outcome(
        &self,
        request: &RetentionRequest<'_>,
        names: &Names,
        stopped: Stopped<'_>,
    ) -> RetentionOutcome {
        let (phase, error, deadline) = (stopped.phase, stopped.error, stopped.deadline);
        if self.tart.is_absent_image(&names.previous).await {
            return outcome(
                RetentionResult::Failed,
                phase,
                error.to_string(),
                request.generation,
            );
        }
        match self
            .ensure_previous_restored(request, names, deadline)
            .await
        {
            Ok(()) => {
                tracing::warn!(allocation_id = %request.allocation.id, "retention rolled back to the previous generation");
                outcome(
                    RetentionResult::RolledBack,
                    phase,
                    error.to_string(),
                    request.generation,
                )
            }
            Err(restore_error) => {
                tracing::error!(allocation_id = %request.allocation.id, %restore_error, "retention could not restore the previous generation; no warm image survives");
                outcome(
                    RetentionResult::Failed,
                    phase,
                    format!("{error}; restore failed: {restore_error}"),
                    request.generation,
                )
            }
        }
    }

    /// Restores only when the live name is actually gone: a clone onto a
    /// surviving image would fail, and a delete-then-clone would replace a good
    /// generation with an older one. One flat result, so an elapsed budget is a
    /// failed restore rather than a successful one.
    async fn ensure_previous_restored(
        &self,
        request: &RetentionRequest<'_>,
        names: &Names,
        deadline: tokio::time::Instant,
    ) -> Result<(), WorkerError> {
        if !self.tart.is_absent_image(&names.warm).await {
            return Ok(());
        }
        tokio::time::timeout_at(
            deadline,
            self.tart
                .clone_image(&names.previous, &names.warm, request.profile, None),
        )
        .await
        .map_err(|_| WorkerError::new("retention rollback exceeded its budget"))?
    }

    /// Rewrites the record as the surviving generation. A first generation has
    /// none, and recovery removes that record once it sees no image.
    async fn revert_record(
        &self,
        request: &RetentionRequest<'_>,
        names: &Names,
        reporter: &AllocationReporter,
    ) {
        let Some(previous) = &request.previous else {
            return;
        };
        let reverted = WarmImageRecord {
            profile: request.allocation.request.profile.clone(),
            warm_template: names.warm.clone(),
            generation: previous.generation,
            base_fingerprint: previous.base_fingerprint.clone(),
            produced_by: previous.produced_by,
            produced_at_unix: previous.produced_at_unix,
            state: WarmImageState::Promoted,
            previous: None,
        };
        if let Err(error) = reporter.record_warm_image(reverted).await {
            tracing::error!(allocation_id = %request.allocation.id, %error, "cannot revert the warm image record after a rollback");
        }
    }
}

/// The only phases whose failure proves the live image was removed. Every
/// earlier phase, including the staging-record write, destroys nothing.
pub(super) const fn is_live_image_removed(phase: RetentionPhase) -> bool {
    matches!(phase, RetentionPhase::Retire | RetentionPhase::Promote)
}

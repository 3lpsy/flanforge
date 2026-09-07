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

    /// Reconciles the physical live name and durable authority while the
    /// caller still excludes consumers and promotions for this profile.
    pub(super) async fn rollback(
        &self,
        request: &RetentionRequest<'_>,
        names: &Names,
        stopped: Stopped<'_>,
        reporter: &AllocationReporter,
    ) -> RetentionOutcome {
        let (phase, error, deadline) = (stopped.phase, stopped.error, stopped.deadline);
        self.drop_candidate(request, names, deadline).await;
        let is_live = !self.tart.is_absent_image(&names.warm).await;
        let restored = match failed_authority(phase, is_live) {
            FailedAuthority::Unchanged => {
                return outcome(
                    RetentionResult::Failed,
                    phase,
                    error.to_string(),
                    request.generation,
                );
            }
            FailedAuthority::CandidateLive => {
                return self
                    .finalize_live_candidate(request, names, stopped, reporter)
                    .await;
            }
            FailedAuthority::PreviousLive => Ok(()),
            FailedAuthority::LiveAbsent => {
                self.ensure_previous_restored(request, names, deadline)
                    .await
            }
        };
        match restored {
            Ok(()) => {
                self.persist_previous(request, names, stopped, reporter)
                    .await
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

    /// Publication can fail after the verified candidate acquired the live
    /// name. In that state the candidate, not `.previous`, is authoritative.
    async fn finalize_live_candidate(
        &self,
        request: &RetentionRequest<'_>,
        names: &Names,
        stopped: Stopped<'_>,
        reporter: &AllocationReporter,
    ) -> RetentionOutcome {
        match self
            .record(request, names, reporter, WarmImageState::Promoted)
            .await
        {
            Ok(()) => {
                tracing::warn!(allocation_id = %request.allocation.id, "retention finalized a candidate that reached the live name before publication failed");
                outcome(
                    RetentionResult::Promoted,
                    stopped.phase,
                    format!(
                        "warm image promoted; cleanup recovered after: {}",
                        stopped.error
                    ),
                    request.generation,
                )
            }
            Err(record_error) => {
                tracing::error!(allocation_id = %request.allocation.id, %record_error, "cannot persist the live candidate generation");
                outcome(
                    RetentionResult::Failed,
                    stopped.phase,
                    format!("{}; candidate record failed: {record_error}", stopped.error),
                    request.generation,
                )
            }
        }
    }

    async fn persist_previous(
        &self,
        request: &RetentionRequest<'_>,
        names: &Names,
        stopped: Stopped<'_>,
        reporter: &AllocationReporter,
    ) -> RetentionOutcome {
        let Some(previous) = request.previous.as_ref() else {
            if let Err(record_error) = reporter
                .ensure_warm_reverted(&request.allocation.request.profile, &names.warm, None)
                .await
            {
                return outcome(
                    RetentionResult::Failed,
                    stopped.phase,
                    format!(
                        "{}; authority cleanup failed: {record_error}",
                        stopped.error
                    ),
                    request.generation,
                );
            }
            return RetentionOutcome::new(
                RetentionResult::Failed,
                stopped.phase,
                stopped.error.to_string(),
                None,
            );
        };
        if let Err(record_error) = reporter
            .ensure_warm_reverted(
                &request.allocation.request.profile,
                &names.warm,
                Some(previous),
            )
            .await
        {
            return outcome(
                RetentionResult::Failed,
                stopped.phase,
                format!("{}; rollback record failed: {record_error}", stopped.error),
                request.generation,
            );
        }
        tracing::warn!(allocation_id = %request.allocation.id, generation = previous.generation, "retention rolled back to the previous generation");
        RetentionOutcome::new(
            RetentionResult::RolledBack,
            stopped.phase,
            stopped.error.to_string(),
            Some(previous.generation),
        )
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

    /// Reports whether the previous generation is physically live again.
    /// Durable authority is reconciled by `rollback` before it reports success.
    #[cfg(test)]
    pub(super) async fn restore_outcome(
        &self,
        request: &RetentionRequest<'_>,
        names: &Names,
        stopped: Stopped<'_>,
    ) -> RetentionOutcome {
        let (phase, error, deadline) = (stopped.phase, stopped.error, stopped.deadline);
        if !self.tart.is_absent_image(&names.warm).await {
            return outcome(
                RetentionResult::RolledBack,
                phase,
                error.to_string(),
                request.generation,
            );
        }
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
}

/// Only retirement and publication can change which generation owns the live
/// name. Publication still needs a host-state check: its candidate may be live.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FailedAuthority {
    Unchanged,
    PreviousLive,
    CandidateLive,
    LiveAbsent,
}

pub(super) const fn failed_authority(phase: RetentionPhase, is_live: bool) -> FailedAuthority {
    match (phase, is_live) {
        (RetentionPhase::Promote, true) => FailedAuthority::CandidateLive,
        (RetentionPhase::Retire, true) => FailedAuthority::PreviousLive,
        (RetentionPhase::Retire | RetentionPhase::Promote, false) => FailedAuthority::LiveAbsent,
        _ => FailedAuthority::Unchanged,
    }
}

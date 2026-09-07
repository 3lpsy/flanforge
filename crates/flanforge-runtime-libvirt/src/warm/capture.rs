use flanforge_core::{
    Allocation, BaseFingerprint, ProfileName, RetentionOutcome, RetentionPhase, RetentionResult,
    VmName, WarmGeneration, WarmImageState,
};
use flanforge_libvirt_wire::OwnershipManifest;
use flanforge_manager::AllocationReporter;
use uuid::Uuid;

use crate::{
    actor::{WarmCaptureRequest, WarmVerifyRequest},
    worker::{LibvirtWorker, PreparedGuest},
};

mod plan;
mod rollback;

pub(crate) use plan::unix_time;
use plan::{ensure_retirement_headroom, outcome, remaining, repoint, stop, stop_staged};

/// Everything one promotion needs, re-read from durable authority.
pub(crate) struct WarmRequest<'a> {
    pub(crate) allocation: &'a Allocation,
    pub(crate) profile: &'a ProfileName,
    pub(crate) warm_template: VmName,
    pub(crate) base_fingerprint: BaseFingerprint,
    pub(crate) generation: u64,
    pub(crate) previous: Option<WarmGeneration>,
    pub(crate) manifest: OwnershipManifest,
    pub(crate) virtual_bytes: u64,
}

/// Where a promotion stopped and how an operator should read it.
pub(super) struct Stopped {
    phase: RetentionPhase,
    result: RetentionResult,
    reason: String,
    /// Whether the record was already written as `Staging`, so the rollback
    /// has to put it back to the generation that survives.
    is_record_staged: bool,
}

impl LibvirtWorker {
    /// Verifies workflow completion, generalizes clone identity, then quiesces,
    /// captures, verifies, and repoints. It can never fail the allocation.
    ///
    /// `RetentionPhase::Retire` is never emitted here: no phase of a libvirt
    /// promotion destroys anything, so a rollback is one delete of a volume
    /// nothing references and the live pointer never moves.
    pub(crate) async fn retain_warm(
        &self,
        request: &WarmRequest<'_>,
        prepared: &PreparedGuest,
        reporter: &AllocationReporter,
    ) -> RetentionOutcome {
        let capture_id = Uuid::new_v4();
        let deadline = tokio::time::Instant::now() + self.warm_capture_timeout;
        match self
            .promote(request, prepared, reporter, capture_id, deadline)
            .await
        {
            Ok(()) => {
                tracing::info!(allocation_id = %request.allocation.id, profile = %request.profile, generation = request.generation, "warm image promoted");
                outcome(
                    RetentionResult::Promoted,
                    RetentionPhase::Promote,
                    "warm image promoted",
                    request.generation,
                )
            }
            Err(stopped) => {
                tracing::error!(allocation_id = %request.allocation.id, phase = ?stopped.phase, reason = %stopped.reason, "retention did not complete");
                // The rollback is a compensating action, so it gets its own
                // budget: the forward path's is already spent. A refusal at
                // the pre-flight gate created nothing to compensate for.
                if stopped.result != RetentionResult::Skipped {
                    self.rollback(request, reporter, capture_id, &stopped).await;
                }
                outcome(
                    stopped.result,
                    stopped.phase,
                    stopped.reason,
                    request.generation,
                )
            }
        }
    }

    async fn promote(
        &self,
        request: &WarmRequest<'_>,
        prepared: &PreparedGuest,
        reporter: &AllocationReporter,
        capture_id: Uuid,
        deadline: tokio::time::Instant,
    ) -> Result<(), Stopped> {
        let published = self
            .load_pointer(request.profile)
            .await
            .map_err(|error| stop(RetentionPhase::Stage, RetentionResult::Failed, error))?;
        ensure_retirement_headroom(published.as_ref())?;
        tokio::time::timeout(remaining(deadline)?, self.ensure_retention_marker(prepared))
            .await
            .map_err(|_| {
                stop(
                    RetentionPhase::Verify,
                    RetentionResult::Failed,
                    "workflow completion marker verification exceeded its timeout",
                )
            })?
            .map_err(|error| stop(RetentionPhase::Verify, RetentionResult::Failed, error))?;
        // Bounded like every other step here: the agent channel holds no
        // connection whose loss would end unresponsive guest work.
        tokio::time::timeout(remaining(deadline)?, self.ensure_generalized(prepared))
            .await
            .map_err(|_| {
                stop(
                    RetentionPhase::Verify,
                    RetentionResult::Failed,
                    "guest clone identity generalization exceeded its timeout",
                )
            })?
            .map_err(|error| stop(RetentionPhase::Verify, RetentionResult::Failed, error))?;
        self.actor
            .warm_quiesce(request.manifest.clone(), remaining(deadline)?)
            .await
            .map_err(|error| stop(RetentionPhase::Stop, RetentionResult::Failed, error))?;
        let staged = self
            .actor
            .warm_capture(
                WarmCaptureRequest {
                    manifest: request.manifest.clone(),
                    profile: request.profile.to_string(),
                    capture_id,
                    generation: request.generation,
                    virtual_bytes: request.virtual_bytes,
                },
                remaining(deadline)?,
            )
            .await
            .map_err(|error| stop(RetentionPhase::Stage, RetentionResult::Failed, error))?;
        let verified = self
            .actor
            .warm_verify(
                WarmVerifyRequest {
                    profile: request.profile.to_string(),
                    capture_id,
                    generation: request.generation,
                    produced_at_unix: staged.produced_at_unix(),
                    virtual_bytes: request.virtual_bytes,
                },
                remaining(deadline)?,
            )
            .await
            // A failed flatten assertion is evidence that will not change
            // without operator action, so it is a rejection rather than a
            // transient failure to retry on the next regeneration.
            .map_err(|error| stop(RetentionPhase::Verify, RetentionResult::Rejected, error))?;
        // Two-phase record: the candidate is durable before the pointer moves.
        // Still labelled Verify, because nothing has been destroyed.
        self.record(request, reporter, WarmImageState::Staging)
            .await
            .map_err(|error| stop_staged(RetentionPhase::Verify, RetentionResult::Failed, error))?;
        {
            // The capture ran for minutes: the document read at the pre-flight
            // gate may name generations a sweep has retired since, so the one
            // that is repointed is re-read inside the ordering guard.
            let _guard = self.pointer_guard().await;
            let published = self.load_pointer(request.profile).await.map_err(|error| {
                stop_staged(RetentionPhase::Promote, RetentionResult::Failed, error)
            })?;
            let document = repoint(request, published.as_ref(), verified).map_err(|error| {
                stop_staged(RetentionPhase::Promote, RetentionResult::Failed, error)
            })?;
            self.ensure_pointer_published(document)
                .await
                .map_err(|error| {
                    stop_staged(RetentionPhase::Promote, RetentionResult::Failed, error)
                })?;
        }
        self.record(request, reporter, WarmImageState::Promoted)
            .await
            .map_err(|error| {
                stop_staged(RetentionPhase::Promote, RetentionResult::Failed, error)
            })?;
        // The checkpoint has done its job; recovery's orphan sweep stays the
        // backstop rather than the only collector.
        if let Err(error) = self.forget_capture(capture_id).await {
            tracing::warn!(profile = %request.profile, %error, "promoted warm capture left its checkpoint");
        }
        Ok(())
    }
}

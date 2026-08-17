use std::time::Duration;

use flanforge_core::{RetentionOutcome, RetentionPhase, RetentionResult, WarmImageState};
use tokio::process::Child;
use tokio_util::sync::CancellationToken;

use flanforge_manager::AllocationReporter;

use flanforge_manager::WorkerError;

use super::{
    super::worker::FlanForgeWorker,
    request::{Names, RetentionRequest, Session, Stopped, derive, outcome},
};

impl FlanForgeWorker {
    /// Strips, stops, stages, and promotes the guest as this profile's warm
    /// image. Reports an outcome; it can never fail the allocation.
    pub(crate) async fn retain_warm(
        &self,
        request: RetentionRequest<'_>,
        vm: &mut Child,
        ip: &str,
        reporter: &AllocationReporter,
        cancellation: &CancellationToken,
    ) -> RetentionOutcome {
        let Ok(names) = derive(&request.warm_template) else {
            return outcome(
                RetentionResult::Failed,
                RetentionPhase::Stage,
                "warm image names are underivable",
                request.generation,
            );
        };
        // One budget for the whole sequence and for every step inside it, so a
        // stalled retention can never push cleanup past the shutdown grace.
        let deadline = tokio::time::Instant::now()
            + Duration::from_secs(request.profile.cleanup_timeout_seconds);
        let mut session = Session {
            vm,
            ip,
            reporter,
            cancellation,
            deadline,
        };
        match self.promote(&request, &names, &mut session).await {
            Ok(()) => {
                tracing::info!(allocation_id = %request.allocation.id, warm_template = %names.warm, generation = request.generation, "warm image promoted");
                outcome(
                    RetentionResult::Promoted,
                    RetentionPhase::Promote,
                    "warm image promoted",
                    request.generation,
                )
            }
            Err((phase, error)) => {
                tracing::error!(allocation_id = %request.allocation.id, ?phase, %error, "retention did not complete");
                // The rollback is a compensating action, so it gets its own
                // budget: the one the forward path exhausted is already spent.
                let stopped = Stopped {
                    phase,
                    error: &error,
                    deadline: tokio::time::Instant::now()
                        + Duration::from_secs(request.profile.cleanup_timeout_seconds),
                };
                self.rollback(&request, &names, stopped, reporter).await
            }
        }
    }

    async fn promote(
        &self,
        request: &RetentionRequest<'_>,
        names: &Names,
        session: &mut Session<'_>,
    ) -> Result<(), (RetentionPhase, WorkerError)> {
        let (cancellation, deadline) = (session.cancellation, session.deadline);
        let step = |phase: RetentionPhase| move |error| (phase, error);
        Self::phase(
            cancellation,
            deadline,
            "retention strip exceeded its budget",
            self.guest.ensure_stripped(session.ip),
        )
        .await
        .map_err(step(RetentionPhase::Strip))?;
        Self::phase(
            cancellation,
            deadline,
            "retention stop exceeded its budget",
            self.stop_guest(request.allocation, session.vm, deadline),
        )
        .await
        .map_err(step(RetentionPhase::Stop))?;
        Self::phase(
            cancellation,
            deadline,
            "retention staging exceeded its budget",
            self.tart.clone_image(
                &request.allocation.vm_name,
                &names.staging,
                request.profile,
                None,
            ),
        )
        .await
        .map_err(step(RetentionPhase::Stage))?;
        Self::phase(
            cancellation,
            deadline,
            "retention verification exceeded its budget",
            self.ensure_staged(&names.staging),
        )
        .await
        .map_err(step(RetentionPhase::Verify))?;

        // Two-phase record: the candidate is durable before anything is
        // destroyed. It is labelled Verify because nothing has been retired
        // yet, and Retire is the rollback's proof that the live image is gone.
        self.record(request, names, session.reporter, WarmImageState::Staging)
            .await
            .map_err(step(RetentionPhase::Verify))?;
        Self::phase(
            cancellation,
            deadline,
            "retention retirement exceeded its budget",
            self.retire(request, names),
        )
        .await
        .map_err(step(RetentionPhase::Retire))?;
        Self::phase(
            cancellation,
            deadline,
            "retention promotion exceeded its budget",
            self.publish(request, names),
        )
        .await
        .map_err(step(RetentionPhase::Promote))?;
        self.record(request, names, session.reporter, WarmImageState::Promoted)
            .await
            .map_err(step(RetentionPhase::Promote))
    }
}

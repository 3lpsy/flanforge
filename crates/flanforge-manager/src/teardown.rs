use flanforge_core::{Allocation, Profile};
use tokio_util::sync::CancellationToken;

use super::{AllocationManager, WorkerError};

/// Binds allocation teardown to the worker task's scope. Whatever ends that
/// task — a return, an unwind, or an `abort()` — the VM it cloned and the
/// runner it registered are torn down and the allocation terminalizes.
#[derive(Debug)]
pub(super) struct WorkerTeardown {
    /// Taking this claim is what makes teardown run exactly once: the normal
    /// path takes it and awaits teardown, and `Drop` only fires without it.
    claim: Option<Teardown>,
}

impl WorkerTeardown {
    pub(super) fn new(
        manager: AllocationManager,
        allocation: Allocation,
        profile: Profile,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            claim: Some(Teardown {
                manager,
                allocation,
                profile,
                cancellation,
            }),
        }
    }

    /// Tears down for a worker that returned, and disarms the guard.
    pub(super) async fn ensure_finished(&mut self, run_result: Result<(), WorkerError>) {
        let Some(teardown) = self.claim.take() else {
            return;
        };
        teardown.run(run_result).await;
    }
}

impl Drop for WorkerTeardown {
    fn drop(&mut self) {
        let Some(teardown) = self.claim.take() else {
            return;
        };
        let id = teardown.allocation.id;
        tracing::error!(allocation_id = %id, "allocation worker task ended without returning; tearing down from its guard");
        // The teardown budget is ambient in the manager, so this spawned run
        // is bounded by the same shutdown grace an awaited one gets: a stop
        // already out of grace abandons it and reports the leak.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::error!(allocation_id = %id, "no runtime is left to tear down the allocation; recovery reconciles it at the next start");
            return;
        };
        runtime.spawn(teardown.run(Err(WorkerError::new(
            "allocation worker task ended without returning",
        ))));
    }
}

#[derive(Debug)]
struct Teardown {
    manager: AllocationManager,
    allocation: Allocation,
    profile: Profile,
    cancellation: CancellationToken,
}

impl Teardown {
    async fn run(self, run_result: Result<(), WorkerError>) {
        let Self {
            manager,
            allocation,
            profile,
            cancellation,
        } = self;
        let id = allocation.id;
        if let Err(error) = &run_result {
            tracing::warn!(allocation_id = %id, %error, "allocation worker stopped with an error");
            // A full host proves nothing about the warm image it never used.
            if !error.is_capacity() {
                manager.ensure_warm_quarantined(id).await;
            }
            let _ = manager.set_worker_error(id, error).await;
        }
        if let Err(error) = manager
            .finish_worker(&allocation, profile, &cancellation, run_result)
            .await
        {
            tracing::error!(allocation_id = %id, %error, "cannot finish allocation");
        }
        // No task listens to the token from here on, whether or not the entry
        // reached a terminal state, so the operator must not be told a
        // cancellation was signalled to somebody.
        manager.release_supervision(id).await;
    }
}

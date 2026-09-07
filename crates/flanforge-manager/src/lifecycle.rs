use std::time::Duration;

use flanforge_core::{Allocation, AllocationId, AllocationState, Profile};
use tokio_util::sync::CancellationToken;

use super::{
    AllocationManager, AllocationReporter, CleanupBudget, LeakReason, ManagerError, WorkerError,
    teardown::WorkerTeardown,
};

impl AllocationManager {
    pub(super) fn spawn_worker(
        &self,
        allocation: Allocation,
        profile: Profile,
        cancellation: CancellationToken,
    ) {
        let manager = self.clone();
        tracing::debug!(allocation_id = %allocation.id, "spawning allocation worker");
        tokio::spawn(async move {
            let reporter = AllocationReporter::new(manager.clone(), allocation.id);
            // Teardown is bound to this scope, so no exit from the worker —
            // including an unwind — can leave the VM or the runner behind.
            let mut teardown = WorkerTeardown::new(
                manager.clone(),
                allocation.clone(),
                profile.clone(),
                cancellation.clone(),
            );
            // The pool machine is made ready before the backend's own run, so
            // `run` has one branch to take rather than a lifecycle to fork.
            let run_result = match manager.ensure_hot_ready(&allocation, &reporter).await {
                Ok(()) => {
                    manager
                        .inner
                        .worker
                        .run(allocation, profile, reporter, cancellation)
                        .await
                }
                Err(error) => Err(error),
            };
            teardown.ensure_finished(run_result).await;
        });
    }

    #[cfg(test)]
    pub(crate) async fn is_supervised(&self, id: AllocationId) -> bool {
        self.inner
            .entries
            .lock()
            .await
            .get(&id)
            .is_some_and(|entry| entry.is_supervised)
    }

    pub(super) async fn release_supervision(&self, id: AllocationId) {
        if let Some(entry) = self.inner.entries.lock().await.get_mut(&id) {
            entry.is_supervised = false;
        }
    }

    pub(super) async fn finish_worker(
        &self,
        allocation: &Allocation,
        profile: Profile,
        cancellation: &CancellationToken,
        run_result: Result<(), WorkerError>,
    ) -> Result<(), ManagerError> {
        let requested = if cancellation.is_cancelled() {
            AllocationState::Cancelled
        } else if run_result.is_ok() {
            AllocationState::Completed
        } else {
            AllocationState::Failed
        };
        let terminal = self
            .ensure_terminal(allocation.clone(), profile, requested)
            .await?;
        tracing::info!(allocation_id = %allocation.id, state = ?terminal, "allocation worker finished");
        Ok(())
    }

    /// Runs teardown before requiring durable terminal bookkeeping. Cleanup
    /// exhaustion records `Failed`; state errors propagate after teardown.
    pub(super) async fn ensure_terminal(
        &self,
        fallback: Allocation,
        profile: Profile,
        requested: AllocationState,
    ) -> Result<AllocationState, ManagerError> {
        let id = fallback.id;
        if let Err(error) = self.ensure_cleaning(id).await {
            tracing::warn!(allocation_id = %id, %error, "cannot persist Cleaning before cleanup; teardown will still run");
        }
        let current = match self.get(id).await {
            Ok(current) => current,
            Err(error) => {
                tracing::warn!(allocation_id = %id, %error, "cannot refresh allocation before cleanup; using the caller's validated snapshot");
                fallback
            }
        };
        // Someone else finished it while this call was waiting: that is success,
        // not a refused transition and not an error to stamp on the record.
        if current.state.is_terminal() {
            return Ok(current.state);
        }
        // The pool takes its machine *before* cleanup, not after. Cleanup is
        // what destroys a guest, so it has to be able to ask whether the pool
        // kept this one — and it can only ask once the answer exists. Running
        // it the other way round would destroy the machine and then offer it.
        if current.is_hot() {
            self.ensure_hot_released(&current, &profile).await;
        }
        let terminal = match self
            .cleanup_with_retries(current.clone(), profile.clone())
            .await
        {
            Ok(()) => requested,
            Err(error) => {
                if let Ok(latest) = self.get(id).await
                    && latest.state.is_terminal()
                {
                    return Ok(latest.state);
                }
                tracing::error!(allocation_id = %id, %error, "cleanup exhausted its retries; allocation needs operator attention");
                if let Err(error) = self.set_error(id, format!("cleanup failed: {error}")).await {
                    tracing::warn!(allocation_id = %id, %error, "cannot persist the cleanup failure");
                }
                AllocationState::Failed
            }
        };
        // Retry the transition after teardown. A transient pre-cleanup write
        // failure must not prevent a durable terminal result.
        self.ensure_cleaning(id).await?;
        let latest = self.get(id).await?;
        if latest.state.is_terminal() {
            return Ok(latest.state);
        }
        self.transition(id, terminal).await?;
        Ok(terminal)
    }

    /// Retries teardown until it succeeds, the attempts run out, or the
    /// shutdown grace does. Spending a fixed grace on retries guarantees the
    /// service manager kills the process mid-cleanup instead of letting one
    /// attempt finish, so a closing manager gets a single bounded attempt and
    /// recovery reconciles the rest on the next start.
    async fn cleanup_with_retries(
        &self,
        allocation: Allocation,
        profile: Profile,
    ) -> Result<(), WorkerError> {
        let budget = self.teardown_budget(Duration::from_secs(profile.cleanup_timeout_seconds));
        let attempts = if budget.is_bounded() { 1 } else { 3 };
        let mut last_error = WorkerError::new("cleanup failed");
        for attempt in 1..=attempts {
            match self.cleanup_once(&allocation, budget).await {
                Ok(()) => {
                    tracing::info!(allocation_id = %allocation.id, attempt, "allocation cleanup completed");
                    return Ok(());
                }
                Err(error) => {
                    tracing::warn!(allocation_id = %allocation.id, attempt, %error, "cleanup attempt failed");
                    last_error = error;
                    if attempt < attempts {
                        tokio::time::sleep(self.inner.tuning.cleanup_retry_delay).await;
                    }
                }
            }
        }
        if budget.is_bounded() {
            self.record_leak(&allocation, LeakReason::CleanupFailed)
                .await;
        }
        Err(last_error)
    }

    /// One teardown attempt. The backend shortens its own steps to the budget;
    /// this is the bound that holds when it cannot.
    async fn cleanup_once(
        &self,
        allocation: &Allocation,
        budget: CleanupBudget,
    ) -> Result<(), WorkerError> {
        let attempt = self.inner.worker.cleanup(allocation.clone(), budget);
        let Some(remaining) = budget.remaining() else {
            return attempt.await;
        };
        tokio::time::timeout(remaining, attempt)
            .await
            .unwrap_or_else(|_| {
                Err(WorkerError::new(
                    "cleanup was abandoned when the shutdown grace expired",
                ))
            })
    }
}

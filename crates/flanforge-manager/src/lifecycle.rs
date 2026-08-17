use std::{sync::atomic::Ordering, time::Duration};

use flanforge_core::{Allocation, AllocationId, AllocationState, Profile};
use tokio_util::sync::CancellationToken;

use super::{AllocationManager, AllocationReporter, ManagerError, WorkerError};

impl AllocationManager {
    /// Cancels active allocations and waits for their cleanup.
    pub async fn shutdown(&self, timeout: Duration) -> bool {
        let mut receivers = {
            let entries = self.inner.entries.lock().await;
            self.inner.is_closing.store(true, Ordering::Release);
            entries
                .values()
                .filter(|entry| !entry.allocation.state.is_terminal())
                .map(|entry| {
                    entry.cancellation.cancel();
                    entry.sender.subscribe()
                })
                .collect::<Vec<_>>()
        };
        tracing::info!(
            active_allocations = receivers.len(),
            "allocation manager shutdown started"
        );
        let completed = tokio::time::timeout(timeout, async {
            for receiver in &mut receivers {
                while !receiver.borrow_and_update().state.is_terminal() {
                    if receiver.changed().await.is_err() {
                        break;
                    }
                }
            }
        })
        .await
        .is_ok();
        if completed {
            tracing::info!("allocation manager shutdown completed");
        } else {
            tracing::warn!("allocation manager shutdown timed out");
        }
        completed
    }

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
            let run_result = manager
                .inner
                .worker
                .run(
                    allocation.clone(),
                    profile.clone(),
                    reporter,
                    cancellation.clone(),
                )
                .await;
            if let Err(error) = &run_result {
                tracing::warn!(allocation_id = %allocation.id, %error, "allocation worker stopped with an error");
                manager.ensure_warm_quarantined(allocation.id).await;
                let _ = manager.set_error(allocation.id, error.to_string()).await;
            }
            if let Err(error) = manager
                .finish_worker(&allocation, profile, &cancellation, run_result)
                .await
            {
                tracing::error!(allocation_id = %allocation.id, %error, "cannot finish allocation");
            }
            // No task listens to the token from here on, whether or not the
            // entry reached a terminal state, so the operator must not be told
            // a cancellation was signalled to somebody.
            manager.release_supervision(allocation.id).await;
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

    async fn release_supervision(&self, id: AllocationId) {
        if let Some(entry) = self.inner.entries.lock().await.get_mut(&id) {
            entry.is_supervised = false;
        }
    }

    async fn finish_worker(
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
            .ensure_terminal(allocation.id, profile, requested)
            .await?;
        tracing::info!(allocation_id = %allocation.id, state = ?terminal, "allocation worker finished");
        Ok(())
    }

    /// Cleans the allocation up and always reaches a terminal state, so a
    /// persistently failing cleanup cannot hold the capacity slot forever.
    pub(super) async fn ensure_terminal(
        &self,
        id: AllocationId,
        profile: Profile,
        requested: AllocationState,
    ) -> Result<AllocationState, ManagerError> {
        self.ensure_cleaning(id).await?;
        let current = self.get(id).await?;
        // Someone else finished it while this call was waiting: that is success,
        // not a refused transition and not an error to stamp on the record.
        if current.state.is_terminal() {
            return Ok(current.state);
        }
        let terminal = match self.cleanup_with_retries(current, profile).await {
            Ok(()) => requested,
            Err(error) => {
                let latest = self.get(id).await?;
                if latest.state.is_terminal() {
                    return Ok(latest.state);
                }
                tracing::error!(allocation_id = %id, %error, "cleanup exhausted its retries; allocation needs operator attention");
                if let Err(error) = self.set_error(id, format!("cleanup failed: {error}")).await {
                    tracing::warn!(allocation_id = %id, %error, "cannot persist the cleanup failure");
                }
                AllocationState::Failed
            }
        };
        self.transition(id, terminal).await?;
        Ok(terminal)
    }

    async fn cleanup_with_retries(
        &self,
        allocation: Allocation,
        profile: Profile,
    ) -> Result<(), WorkerError> {
        let mut last_error = WorkerError::new("cleanup failed");
        for attempt in 1..=3 {
            match self
                .inner
                .worker
                .cleanup(allocation.clone(), profile.clone())
                .await
            {
                Ok(()) => {
                    tracing::info!(allocation_id = %allocation.id, attempt, "allocation cleanup completed");
                    return Ok(());
                }
                Err(error) => {
                    tracing::warn!(allocation_id = %allocation.id, attempt, %error, "cleanup attempt failed");
                    last_error = error;
                    if attempt < 3 {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        }
        Err(last_error)
    }
}

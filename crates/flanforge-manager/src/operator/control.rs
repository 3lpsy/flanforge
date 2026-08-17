use flanforge_core::{Allocation, AllocationId, AllocationState, unix_time};
use tokio::sync::watch;

use super::{
    super::{AllocationManager, ManagerError, recovery::cleanup_only_profile, resolve::profile},
    AllocationSummary,
};

impl AllocationManager {
    /// Every allocation the service knows, oldest first, because the one
    /// holding a slot is the one that has held it longest. Reports only;
    /// there is no policy decision on this path.
    #[must_use]
    pub async fn list(&self) -> Vec<AllocationSummary> {
        let now = unix_time();
        let mut summaries = self
            .inner
            .entries
            .lock()
            .await
            .values()
            .map(|entry| summarize(&entry.allocation, now))
            .collect::<Vec<_>>();
        summaries.sort_by(|left, right| {
            right
                .age_seconds
                .cmp(&left.age_seconds)
                .then_with(|| left.id.to_string().cmp(&right.id.to_string()))
        });
        summaries
    }

    /// Cancels by allocation ID alone: no profile, no claims, no request.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` for an unknown ID, or a lifecycle error when an
    /// unsupervised allocation cannot be driven to a terminal state.
    pub async fn cancel_by_id(&self, id: AllocationId) -> Result<Allocation, ManagerError> {
        let allocation = {
            let mut entries = self.inner.entries.lock().await;
            let entry = entries.get_mut(&id).ok_or(ManagerError::NotFound(id))?;
            if entry.allocation.state.is_terminal() {
                return Ok(entry.allocation.clone());
            }
            entry.cancellation.cancel();
            if entry.is_supervised {
                tracing::info!(allocation_id = %id, state = ?entry.allocation.state, "operator cancellation signalled");
                return Ok(entry.allocation.clone());
            }
            if entry.is_terminalizing {
                // A cancel is already driving this entry; wait for its answer
                // rather than running a second cleanup against one clone.
                let receiver = entry.sender.subscribe();
                drop(entries);
                return self.await_terminal(id, receiver).await;
            }
            entry.is_terminalizing = true;
            entry.allocation.clone()
        };
        // Nothing listens to an unsupervised entry's token, so the operator only
        // gets a truthful answer if cleanup is driven from here.
        tracing::warn!(allocation_id = %id, state = ?allocation.state, "operator is terminalizing an unsupervised allocation");
        let config = self.inner.config.current();
        let profile = profile(&config, &allocation.request)
            .unwrap_or_else(|_| cleanup_only_profile(&allocation));
        let terminalized = self
            .ensure_terminal(id, profile, AllocationState::Cancelled)
            .await;
        if let Some(entry) = self.inner.entries.lock().await.get_mut(&id) {
            entry.is_terminalizing = false;
        }
        terminalized?;
        self.get(id).await
    }

    async fn await_terminal(
        &self,
        id: AllocationId,
        mut receiver: watch::Receiver<Allocation>,
    ) -> Result<Allocation, ManagerError> {
        while !receiver.borrow_and_update().state.is_terminal() {
            if receiver.changed().await.is_err() {
                break;
            }
        }
        self.get(id).await
    }
}

fn summarize(allocation: &Allocation, now: u64) -> AllocationSummary {
    AllocationSummary {
        id: allocation.id,
        profile: allocation.request.profile.clone(),
        repository: allocation.request.repository.clone(),
        run_id: allocation.request.run_id,
        run_attempt: allocation.request.run_attempt,
        state: allocation.state,
        age_seconds: now.saturating_sub(allocation.created_at_unix),
        vm_name: allocation.vm_name.clone(),
        mode: allocation.mode,
        size: allocation.size,
        source: allocation.source.clone(),
    }
}

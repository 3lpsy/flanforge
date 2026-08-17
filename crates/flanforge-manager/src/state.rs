use std::collections::HashMap;

use flanforge_core::{Allocation, AllocationId, AllocationState, CloneSource, RetentionOutcome};

use super::{
    AllocationManager, ManagerError,
    service::{Entry, IN_MEMORY_ALLOCATION_LIMIT},
};

impl AllocationManager {
    pub(crate) async fn get(&self, id: AllocationId) -> Result<Allocation, ManagerError> {
        self.inner
            .entries
            .lock()
            .await
            .get(&id)
            .map(|entry| entry.allocation.clone())
            .ok_or(ManagerError::NotFound(id))
    }

    pub(crate) async fn transition(
        &self,
        id: AllocationId,
        state: AllocationState,
    ) -> Result<(), ManagerError> {
        let from = self.get(id).await?.state;
        self.update(id, |allocation| {
            allocation.transition(state).map_err(Into::into)
        })
        .await?;
        tracing::info!(allocation_id = %id, from = ?from, to = ?state, "allocation state changed");
        Ok(())
    }

    pub(crate) async fn set_runner_id(
        &self,
        id: AllocationId,
        runner_id: i64,
    ) -> Result<(), ManagerError> {
        if runner_id <= 0 {
            return Err(ManagerError::InvalidRunnerId);
        }
        self.update(id, |allocation| {
            allocation.set_runner_id(runner_id);
            Ok(())
        })
        .await?;
        tracing::debug!(allocation_id = %id, runner_id, "runner cleanup identity persisted");
        Ok(())
    }

    pub(crate) async fn set_vm_created(&self, id: AllocationId) -> Result<(), ManagerError> {
        self.update(id, |allocation| {
            allocation.set_vm_created();
            Ok(())
        })
        .await?;
        tracing::debug!(allocation_id = %id, "Tart VM creation persisted");
        Ok(())
    }

    pub(crate) async fn set_source(
        &self,
        id: AllocationId,
        source: CloneSource,
    ) -> Result<(), ManagerError> {
        let name = source.name.clone();
        let reason = source.fallback_reason;
        self.update(id, |allocation| {
            allocation.set_source(source);
            Ok(())
        })
        .await?;
        tracing::info!(allocation_id = %id, source = %name, ?reason, "allocation clone source recorded");
        Ok(())
    }

    pub(crate) async fn set_retention(
        &self,
        id: AllocationId,
        outcome: RetentionOutcome,
    ) -> Result<(), ManagerError> {
        let summary = format!("{:?}/{:?}", outcome.outcome, outcome.phase);
        self.update(id, |allocation| {
            allocation.set_retention(outcome);
            Ok(())
        })
        .await?;
        tracing::info!(allocation_id = %id, retention = %summary, "allocation retention outcome recorded");
        Ok(())
    }

    pub(super) async fn set_error(
        &self,
        id: AllocationId,
        error: impl Into<String>,
    ) -> Result<(), ManagerError> {
        let error = error.into();
        self.update(id, |allocation| {
            allocation.set_error(error);
            Ok(())
        })
        .await
    }

    pub(super) async fn ensure_cleaning(&self, id: AllocationId) -> Result<(), ManagerError> {
        let allocation = self.get(id).await?;
        if allocation.state == AllocationState::Cleaning || allocation.state.is_terminal() {
            return Ok(());
        }
        self.transition(id, AllocationState::Cleaning).await
    }

    async fn update<F>(&self, id: AllocationId, update: F) -> Result<(), ManagerError>
    where
        F: FnOnce(&mut Allocation) -> Result<(), ManagerError>,
    {
        let mut allocation = {
            let entries = self.inner.entries.lock().await;
            entries
                .get(&id)
                .map(|entry| entry.allocation.clone())
                .ok_or(ManagerError::NotFound(id))?
        };
        update(&mut allocation)?;
        self.inner.store.save(&allocation).await?;
        let sender = {
            let mut entries = self.inner.entries.lock().await;
            let entry = entries.get_mut(&id).ok_or(ManagerError::NotFound(id))?;
            entry.allocation = allocation.clone();
            entry.sender.clone()
        };
        sender.send_replace(allocation);
        Ok(())
    }
}

pub(super) fn prune_terminal_history(entries: &mut HashMap<AllocationId, Entry>) {
    let remove_count = entries.len().saturating_sub(IN_MEMORY_ALLOCATION_LIMIT);
    if remove_count == 0 {
        return;
    }

    let mut terminal = entries
        .values()
        .filter(|entry| entry.allocation.state.is_terminal())
        .map(|entry| {
            (
                entry.allocation.updated_at_unix,
                entry.allocation.created_at_unix,
                entry.allocation.id,
            )
        })
        .collect::<Vec<_>>();
    terminal.sort_by_key(|allocation| (allocation.0, allocation.1, allocation.2.to_string()));
    for (_, _, id) in terminal.into_iter().take(remove_count) {
        entries.remove(&id);
    }
}

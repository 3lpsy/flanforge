use std::collections::BTreeSet;

use flanforge_core::{Allocation, AllocationState, Config, NetworkMode, Profile};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::super::{
    AllocationManager, ManagerError,
    resolve::profile,
    service::{Entry, IN_MEMORY_ALLOCATION_LIMIT},
};

/// Cleanup budget applied when the record's configured profile is gone.
const FALLBACK_CLEANUP_TIMEOUT_SECONDS: u64 = 300;

impl AllocationManager {
    /// Loads durable allocations and cleans interrupted work.
    ///
    /// # Errors
    ///
    /// Returns an error only for store-wide conditions; a record that cannot be
    /// reconciled is logged and counted so startup still completes.
    pub async fn recover(&self) -> Result<(), ManagerError> {
        let config = self.inner.config.current();
        let allocations = self
            .inner
            .store
            .load_recent(IN_MEMORY_ALLOCATION_LIMIT)
            .await?;
        let interrupted = allocations
            .iter()
            .filter(|allocation| !allocation.state.is_terminal())
            .count();
        tracing::info!(
            loaded = allocations.len(),
            interrupted,
            "durable allocations loaded"
        );
        self.ensure_entries_loaded(&allocations).await?;

        let mut unreconciled = 0_usize;
        for allocation in allocations
            .into_iter()
            .filter(|allocation| !allocation.state.is_terminal())
        {
            tracing::warn!(allocation_id = %allocation.id, state = ?allocation.state, "recovering interrupted allocation");
            if let Err(error) = self.recover_allocation(&config, &allocation).await {
                unreconciled += 1;
                tracing::error!(allocation_id = %allocation.id, %error, "interrupted allocation was not reconciled");
            }
        }
        if unreconciled > 0 {
            tracing::error!(
                unreconciled,
                "interrupted allocations remain and need operator attention"
            );
        }
        self.ensure_warm_consistent(&config).await
    }

    /// Rebuilds the in-memory entries from durable records, so the committed
    /// capacity is charged again before anything is reconciled. A recovered
    /// entry is unsupervised: no worker is listening to its token.
    pub(crate) async fn ensure_entries_loaded(
        &self,
        allocations: &[Allocation],
    ) -> Result<(), ManagerError> {
        let mut entries = self.inner.entries.lock().await;
        for allocation in allocations {
            if entries.contains_key(&allocation.id) {
                return Err(ManagerError::DuplicateAllocation(allocation.id));
            }
            let (sender, _) = watch::channel(allocation.clone());
            entries.insert(
                allocation.id,
                Entry {
                    allocation: allocation.clone(),
                    sender,
                    cancellation: CancellationToken::new(),
                    is_supervised: false,
                    is_terminalizing: false,
                },
            );
        }
        Ok(())
    }

    /// Reconciles one interrupted record without aborting the recovery pass.
    async fn recover_allocation(
        &self,
        config: &Config,
        allocation: &Allocation,
    ) -> Result<(), ManagerError> {
        let profile = match profile(config, &allocation.request) {
            Ok(profile) => profile,
            Err(error) => {
                tracing::warn!(allocation_id = %allocation.id, %error, "reconciling with the fallback cleanup profile");
                cleanup_only_profile(allocation)
            }
        };
        self.set_error(allocation.id, "daemon restarted during allocation")
            .await?;
        self.ensure_terminal(allocation.id, profile, AllocationState::Failed)
            .await?;
        tracing::info!(allocation_id = %allocation.id, "interrupted allocation recovered");
        Ok(())
    }
}

/// Cleanup-only profile for a record whose configured profile was removed.
/// Cleanup reads the timeout alone, and empty allowlists authorize nothing.
pub(crate) fn cleanup_only_profile(allocation: &Allocation) -> Profile {
    Profile {
        repository: allocation.request.repository.clone(),
        template: allocation.vm_name.clone(),
        runner_label: allocation.runner_label.clone(),
        job_name: String::new(),
        allowed_workflows: BTreeSet::new(),
        allowed_events: BTreeSet::new(),
        allowed_refs: BTreeSet::new(),
        allowed_ref_prefixes: BTreeSet::new(),
        require_protected_ref: true,
        network: NetworkMode::Default,
        cpu_count: 1,
        memory_mb: 2_048,
        boot_timeout_seconds: FALLBACK_CLEANUP_TIMEOUT_SECONDS,
        idle_timeout_seconds: FALLBACK_CLEANUP_TIMEOUT_SECONDS,
        job_timeout_seconds: FALLBACK_CLEANUP_TIMEOUT_SECONDS,
        cleanup_timeout_seconds: FALLBACK_CLEANUP_TIMEOUT_SECONDS,
        warm_template: None,
        regeneration_workflow: None,
        reap: false,
    }
}

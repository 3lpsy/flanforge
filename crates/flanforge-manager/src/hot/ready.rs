use flanforge_core::{Allocation, AllocationState, HotDrainReason};

use super::super::{AllocationManager, AllocationReporter, WorkerError};

impl AllocationManager {
    /// Checks that the machine this allocation claimed is still there, before
    /// the backend's own `run` is entered.
    ///
    /// This is the whole of what a hot claim still pays for. There is nothing
    /// to build: the pool holds only machines a previous allocation left
    /// behind, so a claim either finds one alive or finds nothing.
    ///
    /// # Errors
    ///
    /// Returns an error when the claimed machine is not live. The machine is
    /// evicted and this allocation fails, rather than the daemon silently
    /// re-cloning under an allocation the operator was told reused a guest.
    pub(crate) async fn ensure_hot_ready(
        &self,
        allocation: &Allocation,
        reporter: &AllocationReporter,
    ) -> Result<(), WorkerError> {
        let Some(vm_name) = allocation.origin.hot_vm_name() else {
            return Ok(());
        };
        let guest = match self.inner.hot.load(vm_name).await {
            Ok(Some(guest)) => guest,
            Ok(None) => {
                return Err(WorkerError::new(
                    "the hot guest record this allocation claimed is gone",
                ));
            }
            Err(error) => return Err(WorkerError::new(error.to_string())),
        };
        // `Preparing` is where a hot allocation and a cold one still agree;
        // what happens next is the whole saving.
        reporter
            .transition(AllocationState::Preparing)
            .await
            .map_err(|error| WorkerError::new(error.to_string()))?;
        if !self.inner.worker.is_hot_live(&guest).await {
            self.ensure_hot_evicted(&guest, HotDrainReason::MachineGone)
                .await;
            return Err(WorkerError::new("the claimed hot guest is no longer live"));
        }
        tracing::info!(
            vm_name = %guest.vm_name,
            jobs_served = guest.jobs_served,
            "claimed a hot guest; the clone, the boot, and the readiness wait are all skipped"
        );
        Ok(())
    }
}

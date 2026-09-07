use flanforge_core::{
    AllocationId, AllocationState, CloneSource, Profile, ProfileName, RetentionOutcome, VmName,
    WarmGeneration, WarmImageRecord,
};

use super::super::{
    AllocationManager, ManagerError,
    warm::{RetentionPlan, SourceSelection, retention_plan},
};

#[derive(Clone)]
pub struct AllocationReporter {
    manager: AllocationManager,
    allocation_id: AllocationId,
}

impl std::fmt::Debug for AllocationReporter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AllocationReporter")
            .field("allocation_id", &self.allocation_id)
            .finish_non_exhaustive()
    }
}

impl AllocationReporter {
    pub(crate) fn new(manager: AllocationManager, allocation_id: AllocationId) -> Self {
        Self {
            manager,
            allocation_id,
        }
    }

    /// Records a validated lifecycle transition and persists it.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid transitions or state-store failures.
    pub async fn transition(&self, state: AllocationState) -> Result<(), ManagerError> {
        self.manager.transition(self.allocation_id, state).await
    }

    /// Records the opaque Forgejo runner ID used for cleanup.
    ///
    /// # Errors
    ///
    /// Returns an error when the allocation is missing or persistence fails.
    pub async fn set_runner_id(&self, runner_id: i64) -> Result<(), ManagerError> {
        self.manager
            .set_runner_id(self.allocation_id, runner_id)
            .await
    }

    /// Records that Tart successfully created this allocation's clone.
    ///
    /// # Errors
    ///
    /// Returns an error when the allocation is missing or persistence fails.
    pub async fn set_vm_created(&self) -> Result<(), ManagerError> {
        self.manager.set_vm_created(self.allocation_id).await
    }

    /// Records the clone source resolved at create or at clone time.
    ///
    /// # Errors
    ///
    /// Returns an error when the allocation is missing or persistence fails.
    pub async fn set_source(&self, source: CloneSource) -> Result<(), ManagerError> {
        self.manager.set_source(self.allocation_id, source).await
    }

    /// Re-resolves the source against current warm authority immediately
    /// before a backend clone.
    ///
    /// # Errors
    /// Returns an error when the allocation or warm-image store is unavailable.
    pub async fn resolve_clone_source(
        &self,
        profile: &Profile,
    ) -> Result<SourceSelection, ManagerError> {
        let allocation = self.manager.get(self.allocation_id).await?;
        Ok(self
            .manager
            .resolve_source(&allocation.request.profile, profile, allocation.mode)
            .await)
    }

    /// Atomically records source and generation after a successful clone.
    ///
    /// # Errors
    /// Returns an error when the allocation is missing or persistence fails.
    pub async fn set_clone_source(
        &self,
        source: CloneSource,
        warm_generation: Option<u64>,
    ) -> Result<(), ManagerError> {
        self.manager
            .set_clone_source(self.allocation_id, source, warm_generation)
            .await
    }

    /// Records where retention stopped and what it produced.
    ///
    /// # Errors
    ///
    /// Returns an error when the allocation is missing or persistence fails.
    pub async fn set_retention(&self, outcome: RetentionOutcome) -> Result<(), ManagerError> {
        self.manager
            .set_retention(self.allocation_id, outcome)
            .await
    }

    /// Persists a warm image record for this allocation's profile.
    ///
    /// # Errors
    ///
    /// Returns an error when the record is invalid or persistence fails.
    pub async fn record_warm_image(&self, record: WarmImageRecord) -> Result<(), ManagerError> {
        self.manager.record_warm_image(record).await
    }

    /// Reverts this profile's warm record to the generation that survives an
    /// abandoned promotion, or drops it when none does.
    ///
    /// # Errors
    ///
    /// Returns an error when the record cannot be written or removed.
    pub async fn ensure_warm_reverted(
        &self,
        profile: &ProfileName,
        warm_template: &VmName,
        previous: Option<&WarmGeneration>,
    ) -> Result<(), ManagerError> {
        self.manager
            .ensure_warm_reverted(profile, warm_template, previous)
            .await
    }

    /// Re-reads retention authority and propagates observation failures.
    ///
    /// # Errors
    /// Returns an error when allocation, image, or host state cannot be read.
    pub async fn retention_plan(&self) -> Result<Option<RetentionPlan>, ManagerError> {
        let allocation = self.manager.get(self.allocation_id).await?;
        retention_plan(&self.manager, &allocation).await
    }
}

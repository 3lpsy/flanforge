use std::collections::BTreeSet;

use async_trait::async_trait;
use flanforge_core::{
    Allocation, AllocationId, AllocationState, BaseFingerprint, CloneSource, Profile,
    RetentionOutcome, VmName, WarmImageRecord,
};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::{
    AllocationManager, ManagerError,
    reaper::ReapAuthorization,
    warm::{RetentionPlan, retention_plan},
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MachineState {
    Running,
    Stopped,
    Other,
}

/// One machine the host reports, whoever owns it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostMachine {
    pub name: String,
    pub state: MachineState,
    /// From the VM directory mtime; None when `tart_home` is unset.
    pub age_seconds: Option<u64>,
}

/// A deletion the reaper proposes, carrying the evidence the runtime re-checks.
#[derive(Clone, Copy, Debug)]
pub struct ReapRequest<'a> {
    pub name: &'a VmName,
    pub authorization: &'a ReapAuthorization,
    /// The profile that declares the name, when one still does.
    pub profile: Option<&'a Profile>,
    /// The daemon's own record claiming the name, for a repointed image.
    pub record: Option<&'a WarmImageRecord>,
    /// Every image name live configuration claims; none of them is deletable,
    /// whatever a record says, and whether or not its profile still exists.
    pub reserved: &'a BTreeSet<String>,
}

#[async_trait]
pub trait AllocationWorker: std::fmt::Debug + Send + Sync {
    async fn run(
        &self,
        allocation: Allocation,
        profile: Profile,
        reporter: AllocationReporter,
        cancellation: CancellationToken,
    ) -> Result<(), WorkerError>;

    async fn cleanup(&self, allocation: Allocation, profile: Profile) -> Result<(), WorkerError>;

    /// Every machine the host reports, regardless of owner.
    async fn machines(&self) -> Result<Vec<HostMachine>, WorkerError> {
        Ok(Vec::new())
    }

    /// Fingerprints a template so a rebuilt base invalidates warm images.
    async fn base_fingerprint(&self, _template: &VmName) -> Option<BaseFingerprint> {
        None
    }

    /// Deletes a VM the reaper authorized; the runtime re-checks ownership.
    async fn delete_vm(&self, _request: ReapRequest<'_>) -> Result<(), WorkerError> {
        Err(WorkerError::new("deletion is not supported by this worker"))
    }

    /// Copies one declared image name onto another, for rollback restores.
    async fn clone_image(
        &self,
        _source: &VmName,
        _destination: &VmName,
        _profile: &Profile,
    ) -> Result<(), WorkerError> {
        Err(WorkerError::new(
            "image cloning is not supported by this worker",
        ))
    }
}

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

    /// Re-reads the profile and the image record at retention time; None when
    /// production is no longer authorized.
    pub async fn retention_plan(&self) -> Option<RetentionPlan> {
        let allocation = self.manager.get(self.allocation_id).await.ok()?;
        retention_plan(&self.manager, &allocation).await
    }
}

#[derive(Clone, Debug, thiserror::Error)]
#[error("{message}")]
pub struct WorkerError {
    message: String,
}

impl WorkerError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: flanforge_core::bounded_text(message, 512),
        }
    }
}

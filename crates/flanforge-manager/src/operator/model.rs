use flanforge_core::{
    AllocationId, AllocationMode, AllocationState, CloneSource, GuestSize, ProfileName,
    RepositoryName, VmName, WarmImageState,
};
use flanforge_wire::{CapacityStatus, RuntimeStatus};
use serde::{Deserialize, Serialize};

use super::super::SweepReport;

/// One allocation as the host sees it; a projection of the durable record, not
/// a second place state is kept.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AllocationSummary {
    pub id: AllocationId,
    pub profile: ProfileName,
    pub repository: RepositoryName,
    pub run_id: u64,
    pub run_attempt: u32,
    pub state: AllocationState,
    pub age_seconds: u64,
    pub vm_name: VmName,
    pub mode: AllocationMode,
    pub size: Option<GuestSize>,
    pub source: Option<CloneSource>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WarmImageStatus {
    pub profile: ProfileName,
    pub warm_template: VmName,
    pub generation: u64,
    pub state: WarmImageState,
    pub produced_at_unix: u64,
    /// False once the profile stopped naming this image.
    pub is_referenced: bool,
    /// Proven unusable this run; cleared by a restart or the next promotion.
    pub is_quarantined: bool,
    /// Superseded generations a backend still pins because a live or
    /// recoverable consumer may be reading them. `None` where the backend
    /// retains none at all, so parity with Tart's status output holds.
    #[serde(default)]
    pub retained_generations: Option<u32>,
}

/// In-memory fields read blank after a restart, so each is labelled rather
/// than defaulted to something an operator would read as "never happened".
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperatorStatus {
    pub runtime: RuntimeStatus,
    pub capacity: CapacityStatus,
    /// Reload count since this process started, not since the file changed.
    pub config_generation: u64,
    pub restart_pending: Vec<String>,
    pub warm_images: Vec<WarmImageStatus>,
    /// False when warm-image authority could not be read.
    pub is_warm_image_store_visible: bool,
    /// None when no sweep has run *in this process*.
    pub last_sweep: Option<SweepReport>,
}

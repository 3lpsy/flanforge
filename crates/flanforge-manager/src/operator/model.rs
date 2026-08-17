use flanforge_core::{
    AllocationId, AllocationMode, AllocationState, CloneSource, GuestSize, ProfileName,
    RepositoryName, VmName, WarmImageState,
};
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

/// What admission would decide right now. Every field is derived on read.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CapacityStatus {
    pub active_allocations: usize,
    pub foreign_running: usize,
    pub max_running_vms: u8,
    pub committed_cpu_count: u32,
    pub committed_memory_mb: u32,
    pub host_cpu_count: Option<u8>,
    pub host_memory_mb: Option<u32>,
    /// False when the host listing could not be taken, which is itself busy.
    pub is_host_visible: bool,
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
}

/// In-memory fields read blank after a restart, so each is labelled rather
/// than defaulted to something an operator would read as "never happened".
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperatorStatus {
    pub capacity: CapacityStatus,
    /// Reload count since this process started, not since the file changed.
    pub config_generation: u64,
    pub restart_pending: Vec<String>,
    pub warm_images: Vec<WarmImageStatus>,
    /// None when no sweep has run *in this process*.
    pub last_sweep: Option<SweepReport>,
}

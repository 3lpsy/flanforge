use serde::{Deserialize, Serialize};
use validator::Validate;

/// Admission capacity projected onto the operator HTTP surface.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct CapacityStatus {
    #[validate(range(max = 65_535))]
    pub active_allocations: u32,
    #[validate(range(max = 65_535))]
    pub foreign_running: u32,
    /// Machines the hot pool holds right now, idle or claimed. They occupy
    /// slots continuously, so the concurrency cost is visible without asking.
    #[serde(default)]
    #[validate(range(max = 65_535))]
    pub hot_running: u32,
    /// Slots the pool may hold, `runtime.max_hot_vms`. Zero is hot off.
    #[serde(default)]
    pub max_hot_vms: u8,
    #[validate(range(min = 1))]
    pub max_running_vms: u8,
    #[validate(range(max = 65_535))]
    pub committed_cpu_count: u32,
    #[validate(range(max = 16_777_216))]
    pub committed_memory_mb: u32,
    #[validate(range(max = 16_777_216))]
    pub committed_storage_mb: u64,
    #[validate(range(min = 1))]
    pub host_cpu_count: Option<u8>,
    #[validate(range(min = 2_048, max = 1_048_576))]
    pub host_memory_mb: Option<u32>,
    #[validate(range(min = 16_384, max = 16_777_216))]
    pub host_storage_mb: Option<u64>,
    /// False when the host listing failed, which is itself busy.
    pub is_host_visible: bool,
}

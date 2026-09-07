use serde::{Deserialize, Serialize};

use crate::{CapacityStatus, RuntimeStatus};

/// The dashboard document: everything the operator view renders in one call.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct StatusView {
    pub runtime: RuntimeStatus,
    pub capacity: CapacityStatus,
    /// Reload count since this process started, not since the file changed.
    pub config_generation: u64,
    pub restart_pending: Vec<String>,
    /// Risky-but-permitted configuration, as one line each.
    pub advisories: Vec<String>,
    pub warm_images: Vec<WarmImageView>,
    pub is_warm_image_store_visible: bool,
    /// None when no sweep has run in this process.
    pub last_sweep: Option<SweepView>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WarmImageView {
    pub profile: String,
    pub warm_template: String,
    pub generation: u64,
    pub state: String,
    pub produced_at_unix: u64,
    pub is_referenced: bool,
    pub is_quarantined: bool,
    pub retained_generations: Option<u32>,
}

/// One live allocation as the UI lists it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AllocationView {
    pub id: String,
    pub profile: String,
    pub repository: String,
    pub run_id: u64,
    pub run_attempt: u32,
    pub state: String,
    pub mode: String,
    pub vm_name: String,
    pub age_seconds: u64,
    pub cpu_count: Option<u8>,
    pub memory_mb: Option<u32>,
    pub storage_mb: Option<u64>,
    /// The clone source's name, when one was recorded.
    pub source: Option<String>,
}

/// One durable allocation record as history renders it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AllocationRecordView {
    pub id: String,
    pub profile: String,
    pub repository: String,
    pub run_id: i64,
    pub run_attempt: i64,
    pub state: String,
    pub mode: String,
    pub vm_name: String,
    pub created_at_unix: i64,
    pub updated_at_unix: i64,
    pub error: Option<String>,
    pub terminal_reason: Option<String>,
    pub warm_generation: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AllocationPage {
    pub items: Vec<AllocationRecordView>,
    /// Pass back as `before` to continue; absent on the last page.
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AllocationDetail {
    pub record: AllocationRecordView,
    /// Present while the allocation is resident in the running daemon.
    pub live: Option<AllocationView>,
    /// Newest first, bounded.
    pub events: Vec<EventView>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HotGuestView {
    pub vm_name: String,
    pub profile: String,
    pub lane: String,
    pub state: String,
    pub age_seconds: u64,
    pub idle_seconds: u64,
    pub jobs_served: u32,
    pub claimed_by: Option<String>,
    pub drain_reason: Option<String>,
    pub is_machine_present: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SweepView {
    pub planned: u64,
    pub deleted: Vec<String>,
    pub skipped: Vec<String>,
    pub unaged: Vec<String>,
    pub pruned_records: Vec<String>,
    pub retired_images: u64,
    pub inert_reason: Option<String>,
    pub finished_at_unix: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EventView {
    pub id: i64,
    pub occurred_at_unix: i64,
    pub kind: String,
    pub allocation_id: Option<String>,
    pub vm_name: Option<String>,
    pub profile: Option<String>,
    pub actor: Option<String>,
    /// A small JSON document with kind-specific detail.
    pub payload: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EventPage {
    pub items: Vec<EventView>,
    /// Pass back as `before` to continue; absent on the last page.
    pub next_cursor: Option<i64>,
}

/// Whether a retirement waits for the current claim. Drain is the polite one.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, validator::Validate)]
#[serde(default, deny_unknown_fields)]
pub struct HotRetireRequest {
    pub evict: bool,
}

/// Whether the sweep may delete. Absent or false plans only.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, validator::Validate)]
#[serde(default, deny_unknown_fields)]
pub struct ReapRequest {
    pub delete: bool,
}

/// One captured daemon log line as the UI renders it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LogLineView {
    /// Monotonic per-process sequence; dedupes backlog against live lines.
    pub seq: u64,
    pub ts_unix_ms: u64,
    pub level: String,
    pub target: String,
    pub message: String,
}

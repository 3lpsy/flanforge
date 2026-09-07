use serde::{Deserialize, Serialize};

/// Why a workflow's `hot` request did not produce a retained machine.
///
/// `hot` is a preference, exactly as `warm` is: the workflow asks, the profile
/// permits, and a refusal falls back to an ordinary allocation that is torn
/// down. It never fails the request, so the reason is recorded rather than
/// returned — the same shape as `FallbackReason` on a warm request that booted
/// cold.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HotRefusal {
    /// The profile does not enable hot, so `hot = true` means nothing here.
    NotEnabled,
    /// `hot.lanes` does not admit the lane this request's signed
    /// `ref_protected` claim puts it in.
    LaneNotAdmitted,
    /// A regeneration must capture from a guest whose history is one boot and
    /// one job, so it never runs hot and never retains.
    Regeneration,
    /// The runtime backend does not declare `RuntimeCapability::HotGuests`.
    Unsupported,
    /// `runtime.max_hot_vms` leaves the pool no room for another machine.
    PoolFull,
}

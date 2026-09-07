use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HotState {
    Provisioning,
    Idle,
    Claimed,
    Recycling,
    Draining,
    Evicted,
}

impl HotState {
    /// `Evicted` is the only terminal state; the record is history after it.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Evicted)
    }

    /// True while the record still names a machine the host may be running,
    /// which is what admission charges and the reaper protects.
    #[must_use]
    pub const fn is_holding_machine(self) -> bool {
        !self.is_terminal()
    }

    /// Only `Claimed` binds an allocation, so only it may carry `claimed_by`.
    #[must_use]
    pub const fn is_claim_bearing(self) -> bool {
        matches!(self, Self::Claimed)
    }

    /// `Idle -> Evicted` exists so recovery and the idle-TTL sweep retire an
    /// unclaimed machine without a fictitious drain. `Claimed -> Evicted` does
    /// not: a claimed machine leaves through `Draining`, so the record always
    /// shows the claim ended before the machine did.
    #[must_use]
    pub fn can_transition_to(self, next: Self) -> bool {
        if self == next {
            return true;
        }
        match self {
            Self::Provisioning => matches!(next, Self::Idle | Self::Evicted),
            Self::Idle => matches!(next, Self::Claimed | Self::Draining | Self::Evicted),
            Self::Claimed => matches!(next, Self::Recycling | Self::Draining),
            Self::Recycling => matches!(next, Self::Idle | Self::Evicted),
            Self::Draining => matches!(next, Self::Evicted),
            Self::Evicted => false,
        }
    }
}

/// Why a machine stopped taking claims. Recorded, never inferred.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HotDrainReason {
    /// A run asked to evict this profile's pool. The workflow, not a timer, is
    /// the authority on when a hot session ends.
    EvictRequested,
    /// An allocation could not be admitted while an idle machine held a slot.
    /// A retained machine is a cache, and a cache yields under pressure.
    CapacityPressure,
    MaxLifetime,
    MaxJobs,
    /// The lane already holds `max_idle` machines, so a release overshot it.
    /// Distinct from `IdleTtl`: this machine was retired for the company it
    /// keeps, not for how long it sat unclaimed.
    MaxIdle,
    IdleTtl,
    ResetFailed,
    StaleBase,
    ConfigReloaded,
    /// The host no longer reports the machine.
    MachineGone,
    /// Present but not verifiable as clean, so never adopted.
    Unverifiable,
    OperatorRequest,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("invalid hot guest state transition from {from:?} to {to:?}")]
pub struct HotTransitionError {
    pub from: HotState,
    pub to: HotState,
}

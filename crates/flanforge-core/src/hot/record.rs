use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationError};

use crate::{
    AllocationId, CloneSource, GuestSize, ProfileName, VmName,
    hot::{HotDrainReason, HotLane, HotState, HotTransitionError},
};

/// One durable record per hot machine, at `<state_dir>/hot/<vm-name>.json`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Validate)]
#[serde(deny_unknown_fields)]
#[validate(schema(function = "validate_claim"))]
pub struct HotGuest {
    #[validate(nested)]
    pub vm_name: VmName,
    #[validate(nested)]
    pub profile: ProfileName,
    pub lane: HotLane,
    pub state: HotState,
    /// The size the allocation that earned the machine ran under. Admission
    /// charges this, so a machine of unknown size is not representable, and a
    /// machine retained under a superseded profile is charged what it costs.
    #[validate(nested)]
    pub size: GuestSize,
    #[validate(nested)]
    pub source: CloneSource,
    #[validate(range(min = 1))]
    pub warm_generation: Option<u64>,
    /// How long this machine may live, in seconds from `booted_at_unix`, as
    /// the run that retained it asked. `None` takes the profile's ceiling, and
    /// the ceiling binds either way — a reload that lowers it shortens a
    /// machine already running under the old one.
    #[serde(default)]
    #[validate(range(min = 1))]
    pub age_limit_seconds: Option<u64>,
    pub booted_at_unix: u64,
    pub updated_at_unix: u64,
    pub jobs_served: u32,
    pub claimed_by: Option<AllocationId>,
    pub drain_reason: Option<HotDrainReason>,
}

impl HotGuest {
    /// Creates a `Provisioning` record. Written at release, not before a clone:
    /// the machine already exists and has already served the job that earned
    /// it, so `Provisioning` is only the window before the backend confirms it
    /// holds the machine.
    #[must_use]
    pub fn new(
        vm_name: VmName,
        profile: ProfileName,
        lane: HotLane,
        size: GuestSize,
        source: CloneSource,
        warm_generation: Option<u64>,
        age_limit_seconds: Option<u64>,
    ) -> Self {
        let now = flanforge_utils::unix_time();
        Self {
            vm_name,
            profile,
            lane,
            state: HotState::Provisioning,
            size,
            source,
            warm_generation,
            age_limit_seconds,
            booted_at_unix: now,
            updated_at_unix: now,
            jobs_served: 0,
            claimed_by: None,
            drain_reason: None,
        }
    }

    /// Moves the record along its state machine, stamping the update time.
    ///
    /// # Errors
    ///
    /// Returns an error when the edge is not one the state machine permits.
    pub fn ensure_state(&mut self, next: HotState) -> Result<(), HotTransitionError> {
        if !self.state.can_transition_to(next) {
            return Err(HotTransitionError {
                from: self.state,
                to: next,
            });
        }
        self.state = next;
        self.updated_at_unix = flanforge_utils::unix_time();
        Ok(())
    }

    /// Binds one allocation under the caller's lock. State and claim move
    /// together because the record's one cross-field invariant is that only a
    /// `Claimed` record carries a `claimed_by`; setting either alone breaks it.
    ///
    /// # Errors
    ///
    /// Returns an error unless the record is `Idle`.
    pub fn ensure_claimed(&mut self, id: AllocationId) -> Result<(), HotTransitionError> {
        if self.state != HotState::Idle {
            return Err(HotTransitionError {
                from: self.state,
                to: HotState::Claimed,
            });
        }
        self.ensure_state(HotState::Claimed)?;
        self.claimed_by = Some(id);
        Ok(())
    }

    /// Releases the claim into `Recycling`, clearing `claimed_by` and counting
    /// the job in the same step: the claim, the state, and the reuse count are
    /// one fact about the machine and must not be settable apart.
    ///
    /// # Errors
    ///
    /// Returns an error unless the record is `Claimed`.
    pub fn ensure_released(&mut self) -> Result<(), HotTransitionError> {
        if self.state != HotState::Claimed {
            return Err(HotTransitionError {
                from: self.state,
                to: HotState::Recycling,
            });
        }
        self.ensure_state(HotState::Recycling)?;
        self.claimed_by = None;
        self.jobs_served = self.jobs_served.saturating_add(1);
        Ok(())
    }

    /// Whether this machine is claimable for that profile and lane. Profile
    /// pinning is absolute and the lane is derived from a signed claim.
    #[must_use]
    pub fn is_serving(&self, profile: &ProfileName, lane: HotLane) -> bool {
        self.state == HotState::Idle && &self.profile == profile && self.lane == lane
    }
}

/// A record claiming an allocation while `Idle` is exactly the shape that lets
/// two allocations onto one machine, so it must never survive a load.
fn validate_claim(guest: &HotGuest) -> Result<(), ValidationError> {
    if guest.claimed_by.is_some() == guest.state.is_claim_bearing() {
        Ok(())
    } else {
        Err(ValidationError::new("hot_claim"))
    }
}

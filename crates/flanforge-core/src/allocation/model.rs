use serde::{Deserialize, Serialize};
use uuid::Uuid;
use validator::{Validate, ValidationError};

use crate::{
    AllocationState, HotLane, HotRefusal, ProfileName, RepositoryName, RunnerLabel,
    StateTransitionError, VmName,
    allocation::{
        AllocationMode, AllocationOrigin, CloneKind, CloneSource, GuestSize, TerminalReason,
    },
    bounded_text,
    image::RetentionOutcome,
    unix_time,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct AllocationId(Uuid);

impl AllocationId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    #[must_use]
    pub const fn into_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for AllocationId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for AllocationId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::str::FromStr for AllocationId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct AllocationRequest {
    #[validate(nested)]
    pub profile: ProfileName,
    #[validate(nested)]
    pub repository: RepositoryName,
    #[validate(range(min = 1))]
    pub run_id: u64,
    #[validate(range(min = 1))]
    pub run_attempt: u32,
}

impl AllocationRequest {
    #[must_use]
    pub fn is_same_attempt(&self, other: &Self) -> bool {
        self.repository == other.repository
            && self.run_id == other.run_id
            && self.run_attempt == other.run_attempt
            && self.profile == other.profile
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Validate)]
pub struct Allocation {
    pub id: AllocationId,
    #[validate(nested)]
    pub request: AllocationRequest,
    pub state: AllocationState,
    #[validate(nested)]
    pub vm_name: VmName,
    #[validate(nested)]
    pub runner_label: RunnerLabel,
    #[serde(default)]
    pub vm_created: bool,
    #[validate(range(min = 1))]
    pub runner_id: Option<i64>,
    pub created_at_unix: u64,
    pub updated_at_unix: u64,
    #[validate(custom(function = "validate_error"))]
    pub error: Option<String>,
    #[serde(default)]
    pub mode: AllocationMode,
    #[serde(default)]
    #[validate(nested)]
    pub size: Option<GuestSize>,
    #[serde(default)]
    #[validate(nested)]
    pub source: Option<CloneSource>,
    /// Whether the guest was cloned for this allocation or reused from the hot
    /// pool. Defaulted on read, so records written before hot still parse.
    #[serde(default)]
    #[validate(nested)]
    pub origin: AllocationOrigin,
    /// The pool lane this allocation's guest is retained into when its job
    /// ends, or `None` when the guest is destroyed as usual. One field rather
    /// than a flag beside a lane: a retained machine without a lane is not a
    /// machine anything could claim.
    #[serde(default)]
    pub hot_lane: Option<HotLane>,
    /// Why a `hot` request produced no retained machine. `None` alongside a
    /// `None` lane means the workflow never asked.
    #[serde(default)]
    pub hot_refusal: Option<HotRefusal>,
    /// How long this run asked its retained machine to live, beneath the
    /// profile's ceiling. `None` takes the ceiling itself.
    #[serde(default)]
    #[validate(range(min = 1))]
    pub hot_age_seconds: Option<u64>,
    /// Promoted warm generation selected for this allocation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[validate(range(min = 1))]
    pub warm_generation: Option<u64>,
    #[serde(default)]
    #[validate(nested)]
    pub retention: Option<RetentionOutcome>,
    /// Set only when the terminal state alone would misreport what happened.
    #[serde(default)]
    pub terminal_reason: Option<TerminalReason>,
}

fn validate_error(value: &str) -> Result<(), ValidationError> {
    if value.len() <= 512 && !value.contains('\0') {
        Ok(())
    } else {
        Err(ValidationError::new("error_text"))
    }
}

impl Allocation {
    #[must_use]
    pub fn new(
        request: AllocationRequest,
        vm_name: VmName,
        runner_label: RunnerLabel,
        mode: AllocationMode,
        size: GuestSize,
    ) -> Self {
        let now = unix_time();
        Self {
            id: AllocationId::new(),
            request,
            state: AllocationState::Requested,
            vm_name,
            runner_label,
            vm_created: false,
            runner_id: None,
            created_at_unix: now,
            updated_at_unix: now,
            error: None,
            mode,
            size: Some(size),
            source: None,
            origin: AllocationOrigin::Cloned,
            hot_lane: None,
            hot_refusal: None,
            hot_age_seconds: None,
            warm_generation: None,
            retention: None,
            terminal_reason: None,
        }
    }

    /// Moves the allocation through its monotonic lifecycle.
    ///
    /// # Errors
    ///
    /// Returns an error when the requested transition skips a required phase
    /// or attempts to restart a terminal allocation.
    pub fn transition(&mut self, state: AllocationState) -> Result<(), StateTransitionError> {
        if !self.state.can_transition_to(state) {
            return Err(StateTransitionError {
                from: self.state,
                to: state,
            });
        }
        self.state = state;
        self.updated_at_unix = unix_time();
        Ok(())
    }

    pub fn set_runner_id(&mut self, runner_id: i64) {
        self.runner_id = Some(runner_id);
        self.updated_at_unix = unix_time();
    }

    pub fn set_vm_created(&mut self) {
        self.vm_created = true;
        self.updated_at_unix = unix_time();
    }

    pub fn set_error(&mut self, error: impl Into<String>) {
        self.error = Some(bounded_text(error, 512));
        self.updated_at_unix = unix_time();
    }

    pub fn set_source(&mut self, source: CloneSource) {
        if source.kind != CloneKind::Warm {
            self.warm_generation = None;
        }
        self.source = Some(source);
        self.updated_at_unix = unix_time();
    }

    /// Records the source and generation proven at the successful clone
    /// boundary. A template fallback can never retain planned warm evidence.
    pub fn set_clone_source(&mut self, source: CloneSource, warm_generation: Option<u64>) {
        self.warm_generation = (source.kind == CloneKind::Warm)
            .then_some(warm_generation)
            .flatten();
        self.source = Some(source);
        self.updated_at_unix = unix_time();
    }

    /// Records what admission decided about this allocation's `hot` request:
    /// the lane its guest is retained into, the machine it reused, and the
    /// source that machine was itself built from. One call, because a lane
    /// without an outcome and an outcome without a lane are both incoherent.
    pub fn set_hot(
        &mut self,
        lane: Option<HotLane>,
        origin: AllocationOrigin,
        source: Option<CloneSource>,
        age_seconds: Option<u64>,
    ) {
        if let Some(source) = source {
            self.source = Some(source);
        }
        self.origin = origin;
        self.hot_lane = lane;
        self.hot_age_seconds = age_seconds;
        self.hot_refusal = None;
        self.updated_at_unix = unix_time();
    }

    /// Records that a `hot` request was refused. The allocation proceeds as an
    /// ordinary one, so this never fails anything.
    pub fn set_hot_refused(&mut self, refusal: HotRefusal) {
        self.hot_lane = None;
        self.hot_refusal = Some(refusal);
        self.updated_at_unix = unix_time();
    }

    /// Whether this allocation's guest is kept when its job ends, either
    /// because it reused a pool machine or because it is being retained into
    /// the pool. The one predicate teardown branches on.
    #[must_use]
    pub const fn is_hot(&self) -> bool {
        self.hot_lane.is_some() || self.origin.is_hot_reuse()
    }

    pub fn set_retention(&mut self, outcome: RetentionOutcome) {
        self.retention = Some(outcome);
        self.updated_at_unix = unix_time();
    }

    /// The host was full, not broken; the API owes this caller a busy answer.
    pub fn set_capacity_busy(&mut self) {
        self.terminal_reason = Some(TerminalReason::Capacity);
        self.updated_at_unix = unix_time();
    }

    #[must_use]
    pub fn is_capacity_busy(&self) -> bool {
        self.terminal_reason == Some(TerminalReason::Capacity)
    }

    /// Regeneration that completed; the remaining gates are profile-side.
    #[must_use]
    pub fn is_retention_eligible(&self) -> bool {
        self.mode == AllocationMode::Regenerate && self.vm_created
    }
}

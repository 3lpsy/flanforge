use serde::{Deserialize, Serialize};
use uuid::Uuid;
use validator::{Validate, ValidationError};

use crate::{
    AllocationState, ProfileName, RepositoryName, RunnerLabel, StateTransitionError, VmName,
    allocation::{AllocationMode, CloneSource, GuestSize},
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
    #[serde(default)]
    #[validate(nested)]
    pub retention: Option<RetentionOutcome>,
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
            retention: None,
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
        self.source = Some(source);
        self.updated_at_unix = unix_time();
    }

    pub fn set_retention(&mut self, outcome: RetentionOutcome) {
        self.retention = Some(outcome);
        self.updated_at_unix = unix_time();
    }

    /// Regeneration that completed; the remaining gates are profile-side.
    #[must_use]
    pub fn is_retention_eligible(&self) -> bool {
        self.mode == AllocationMode::Regenerate && self.vm_created
    }
}

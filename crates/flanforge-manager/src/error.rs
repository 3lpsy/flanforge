use flanforge_core::{AllocationId, AuthorizationError, StateTransitionError};
use thiserror::Error;

use flanforge_store::StoreError;

use super::WorkerError;

/// Why admission refused. Logged with the refusal, never returned in a body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BusyReason {
    Serialized,
    Slots,
    Budget,
    ProbeFailed,
    /// One profile's three image names are owned by one producer at a time.
    Regenerating,
}

#[derive(Debug, Error)]
pub enum ManagerError {
    #[error("unknown profile {0}")]
    UnknownProfile(String),
    #[error("allocation capacity is busy")]
    Busy {
        reason: BusyReason,
        holder: Option<AllocationId>,
    },
    #[error("allocation {0} was not found")]
    NotFound(AllocationId),
    #[error("duplicate allocation ID {0} in durable state")]
    DuplicateAllocation(AllocationId),
    #[error("invalid generated VM name: {0}")]
    InvalidVmName(String),
    #[error("invalid generated runner label: {0}")]
    InvalidRunnerLabel(String),
    #[error("request exceeds its profile ceiling")]
    RequestExceedsProfile,
    #[error("allocation request is structurally invalid")]
    InvalidRequest,
    #[error("service is shutting down")]
    ShuttingDown,
    #[error("runner ID must be positive")]
    InvalidRunnerId,
    #[error(transparent)]
    Authorization(#[from] AuthorizationError),
    #[error(transparent)]
    Transition(#[from] StateTransitionError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Worker(#[from] WorkerError),
}

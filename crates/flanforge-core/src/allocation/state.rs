use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AllocationState {
    Requested,
    Preparing,
    Booting,
    Registering,
    WaitingForJob,
    Ready,
    Running,
    Cleaning,
    Completed,
    Failed,
    Cancelled,
}

impl AllocationState {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    #[must_use]
    pub fn can_transition_to(self, next: Self) -> bool {
        if self == next {
            return true;
        }
        match self {
            Self::Requested => matches!(next, Self::Preparing | Self::Cleaning),
            // `Registering` is the hot claim: the machine is already up, so
            // there is nothing to boot. Adding the edge rather than a state is
            // deliberate — allocation states are a wire contract the allocator
            // job reads, and an unseen transition breaks nobody.
            Self::Preparing => matches!(next, Self::Booting | Self::Registering | Self::Cleaning),
            Self::Booting => matches!(next, Self::Registering | Self::Cleaning),
            Self::Registering => matches!(next, Self::WaitingForJob | Self::Cleaning),
            Self::WaitingForJob => matches!(next, Self::Ready | Self::Running | Self::Cleaning),
            Self::Ready => matches!(next, Self::Running | Self::Cleaning),
            Self::Running => matches!(next, Self::Cleaning),
            Self::Cleaning => matches!(next, Self::Completed | Self::Failed | Self::Cancelled),
            Self::Completed | Self::Failed | Self::Cancelled => false,
        }
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("invalid allocation state transition from {from:?} to {to:?}")]
pub struct StateTransitionError {
    pub from: AllocationState,
    pub to: AllocationState,
}

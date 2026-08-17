mod model;
mod size;
mod sizing;
mod source;
mod state;

pub use model::{Allocation, AllocationId, AllocationRequest};
pub use size::GuestSize;
pub use sizing::{RequestOptions, SizeCeilingError, resolve_mode, resolve_size};
pub use source::{AllocationMode, CloneKind, CloneSource, FallbackReason};
pub use state::{AllocationState, StateTransitionError};

#[cfg(test)]
mod tests;

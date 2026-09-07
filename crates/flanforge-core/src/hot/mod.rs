mod lane;
mod record;
mod refusal;
mod request;
mod state;

pub use lane::{HotLane, HotLanePolicy};
pub use record::HotGuest;
pub use refusal::HotRefusal;
pub use request::{HotCeilingError, HotRequest, resolve_hot_age};
pub use state::{HotDrainReason, HotState, HotTransitionError};

#[cfg(test)]
mod tests;

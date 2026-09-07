//! The daemon's durable-state ports: store traits, the event sink, and the
//! single-instance mutation lock. Sqlite implementations live in
//! `flanforge-orm`.

mod allocations;
mod error;
mod event;
mod hot;
mod images;
mod lock;

pub use allocations::AllocationStore;
pub use error::StoreError;
pub use event::{
    BroadcastEventSink, Event, EventKind, EventSink, MAX_EVENT_PAYLOAD_BYTES, NullEventSink,
    TeeEventSink,
};
pub use hot::HotGuestStore;
pub use images::WarmImageStore;
pub use lock::StateMutationLock;

#[cfg(test)]
mod tests;

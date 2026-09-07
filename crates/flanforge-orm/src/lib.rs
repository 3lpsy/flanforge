//! Sea-orm entities for the daemon's database and the conversions between
//! rows and the domain records in `flanforge-core`.

pub mod entities;

mod convert;
mod import;
mod services;
mod stores;

pub use convert::{
    ConvertError, allocation_from_model, allocation_to_model, hot_guest_from_model,
    hot_guest_to_model, warm_image_from_model, warm_image_to_model,
};
pub use import::{
    ImportError, ImportReport, MetaError, ensure_state_dir_identity, import_json_state,
};
pub use services::{
    AllocationCursor, AuthSource, HistoryService, SessionRecord, SessionService, UserError,
    UserRecord, UserService,
};
pub use stores::{
    PruneOutcome, SqliteAllocationStore, SqliteEventSink, SqliteHotGuestStore,
    SqliteWarmImageStore, prune_history,
};

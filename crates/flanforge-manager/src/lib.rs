mod admission;
mod config_reload;
mod error;
mod hot;
mod lifecycle;
mod operator;
mod reaper;
mod recovery;
mod resolve;
mod service;
mod shutdown;
mod state;
mod teardown;
mod tuning;
mod warm;
mod worker;

pub use config_reload::ConfigHandle;
pub use error::{BusyReason, ManagerError};
pub use hot::HotGuestStatus;
pub use operator::{
    AllocationSummary, CapacityStatus, OPERATOR_TOKEN_HEADER, OperatorStatus, WarmImageStatus,
    ensure_operator_token, is_token_match, operator_token_path, read_operator_token,
};
pub use reaper::{
    ReapAuthorization, ReapCandidate, ReapInputs, SweepInertReason, SweepReport, plan_sweep,
    reserved_image_names, unaged_candidates,
};
pub use service::{AllocationManager, CreateAllocation};
pub use shutdown::{AllocationLeak, LeakReason, ShutdownReport};
pub use warm::{RetentionPlan, SourceSelection};
pub use worker::{
    AllocationReporter, AllocationWorker, CleanupBudget, HostMachine, HotReset, HotRetainRequest,
    ImageSweep, MachineOwnership, MachineState, ReapRequest, RetiredImage, WarmAvailability,
    WorkerError,
};

#[cfg(test)]
mod tests;

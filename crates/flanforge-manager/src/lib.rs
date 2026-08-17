mod admission;
mod config_reload;
mod error;
mod lifecycle;
mod operator;
mod reaper;
mod recovery;
mod resolve;
mod service;
mod state;
mod warm;
mod worker;

pub use config_reload::ConfigHandle;
pub use error::{BusyReason, ManagerError};
pub use operator::{
    AllocationSummary, CapacityStatus, OPERATOR_TOKEN_HEADER, OperatorStatus, WarmImageStatus,
    ensure_operator_token, is_token_match, operator_token_path, read_operator_token,
};
pub use reaper::{
    ReapAuthorization, ReapCandidate, ReapInputs, SweepInertReason, SweepReport, plan_sweep,
    reserved_image_names,
};
pub use service::{AllocationManager, CreateAllocation};
pub use warm::RetentionPlan;
pub use worker::{
    AllocationReporter, AllocationWorker, HostMachine, MachineState, ReapRequest, WorkerError,
};

#[cfg(test)]
mod tests;

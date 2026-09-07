mod channel;
mod guest;
mod job;
mod recycle;
mod supervisor;

pub use channel::{
    GuestCapture, GuestChannel, GuestCommand, GuestExit, GuestFileTransfer, GuestOutput,
    GuestProcess, GuestProgram, GuestSecret, GuestSpawn,
};
pub use guest::{
    GuestControl, GuestSession, LIBVIRT_TAILSCALE_PATH, REGENERATION_SENTINEL, RunnerSpawn,
    SshChannel, TART_TAILSCALE_PATH, ensure_guest_known_hosts, retention_marker_script,
    shell_quote, temporary_guest_paths, temporary_guest_patterns,
};
pub use job::{GuestJob, RunnerDelivery, RunnerRegistration, StartedRunner};
pub use recycle::{
    RECYCLE_CONTRACT_VERSION, RECYCLE_HELPER, RECYCLE_MAX_REPORT_BYTES, RecycleGate, RecycleVerdict,
};

#[cfg(feature = "test-support")]
#[doc(hidden)]
pub use guest::{runner_script_with_token_template, ssh_process_for_test};

#[cfg(feature = "test-support")]
#[doc(hidden)]
pub use supervisor::SupervisionWindow;

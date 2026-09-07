mod control;
mod input;
mod marker;
mod paths;
mod runner;
mod script;
mod session;
mod ssh;
mod tailscale;

pub use control::GuestControl;
pub use marker::{REGENERATION_SENTINEL, retention_marker_script};
pub use paths::{temporary_guest_paths, temporary_guest_patterns};
pub use runner::RunnerSpawn;
#[cfg(feature = "test-support")]
pub use script::runner_script_with_token_template;
pub use script::shell_quote;
pub use session::GuestSession;
pub(crate) use session::GuestTrust;
#[cfg(feature = "test-support")]
pub use ssh::ssh_process_for_test;
pub use ssh::{SshChannel, ensure_guest_known_hosts};
pub use tailscale::{LIBVIRT_TAILSCALE_PATH, TART_TAILSCALE_PATH};

#[cfg(test)]
mod tests;

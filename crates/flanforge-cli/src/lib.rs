//! Command-line model and local macOS service/configuration operations.

mod cli;
mod commands;
mod paths;

pub use cli::{Arguments, DaemonCommand, TopCommand};
pub use commands::{
    run_allocation_command, run_config_command, run_daemon_command, run_profile_command,
    run_reaper_command,
};
pub use paths::selected_config_path;

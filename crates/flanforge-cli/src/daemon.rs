use std::path::PathBuf;

use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct DaemonArgs {
    #[command(subcommand)]
    pub command: DaemonCommand,
}

#[derive(Clone, Debug, Subcommand)]
pub enum DaemonCommand {
    /// Run the foreground daemon process.
    Run,
    /// Install this binary and its native service definition.
    Install,
    /// Start the installed service.
    Start(DaemonControlArgs),
    /// Stop the installed service.
    Stop,
    /// Restart the installed service.
    Restart(DaemonControlArgs),
    /// Report whether the service is loaded, running, and healthy.
    Status,
    /// Check native runtime prerequisites without changing host state.
    Doctor,
    /// Print the service log.
    Logs(DaemonLogsArgs),
    /// Report native host permission gates; macOS can establish supported gates.
    #[command(name = "priv")]
    Priv(DaemonPrivArgs),
}

/// Native preflight accompanies control by default. On macOS, both consent
/// gates re-key to the binary path after every deploy.
#[derive(Clone, Copy, Debug, Args)]
pub struct DaemonControlArgs {
    /// Skip the status report that otherwise follows.
    #[arg(long)]
    pub no_status: bool,
    /// Skip the privilege gate report that otherwise follows.
    #[arg(long)]
    pub no_privcheck: bool,
}

#[derive(Clone, Copy, Debug, Args)]
pub struct DaemonLogsArgs {
    /// Keep printing new lines as they are written.
    #[arg(short, long)]
    pub follow: bool,
    /// Number of trailing lines to print first.
    #[arg(short = 'n', long = "lines", default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..=100_000))]
    pub lines: u32,
    /// Read the provider's error stream when it has a separate one.
    #[arg(long)]
    pub stderr: bool,
}

#[derive(Clone, Debug, Args)]
#[allow(clippy::struct_excessive_bools)] // Clap exposes one flag per gate.
pub struct DaemonPrivArgs {
    /// Report every gate and change nothing. This is the default.
    #[arg(long, conflicts_with_all = ["grant_firewall", "prompt"])]
    pub check: bool,
    /// Add and unblock the installed binary in the Application Firewall.
    #[arg(long)]
    pub grant_firewall: bool,
    /// Perform the guarded operations now so macOS raises its consent prompts
    /// while an operator is present to answer them.
    #[arg(long)]
    pub prompt: bool,
    /// Marker of the sudo re-execution, naming the binary to authorize.
    #[arg(long, hide = true, value_name = "PATH")]
    pub elevated: Option<PathBuf>,
    /// Run the consent probes only, printing one result line per gate.
    #[arg(long, hide = true)]
    pub probe: bool,
}

impl DaemonPrivArgs {
    /// Reporting only: no grant, no prompt. What `start`/`restart` ask for, and
    /// what a bare `daemon priv` already does.
    #[must_use]
    pub fn report_only() -> Self {
        Self {
            check: true,
            grant_firewall: false,
            prompt: false,
            elevated: None,
            probe: false,
        }
    }
}

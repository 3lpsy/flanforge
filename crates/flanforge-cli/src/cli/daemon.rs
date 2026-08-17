use std::path::PathBuf;

use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct DaemonArgs {
    #[command(subcommand)]
    pub command: DaemonCommand,
}

#[derive(Clone, Debug, Subcommand)]
pub enum DaemonCommand {
    /// Run the foreground daemon process (used by launchd).
    Run,
    /// Install this binary and the per-user macOS `LaunchAgent`.
    Install,
    /// Start the installed `LaunchAgent`.
    Start,
    /// Stop the installed `LaunchAgent`.
    Stop,
    /// Restart the installed `LaunchAgent`.
    Restart,
    /// Report whether the service is loaded, running, and healthy.
    Status,
    /// Print the service log.
    Logs(DaemonLogsArgs),
    /// Report, and optionally establish, the macOS host permission gates.
    #[command(name = "priv")]
    Priv(DaemonPrivArgs),
}

#[derive(Clone, Copy, Debug, Args)]
pub struct DaemonLogsArgs {
    /// Keep printing new lines as they are written.
    #[arg(short, long)]
    pub follow: bool,
    /// Number of trailing lines to print first.
    #[arg(short = 'n', long = "lines", default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..=100_000))]
    pub lines: u32,
    /// Read the launchd error log instead of the configured log file.
    #[arg(long)]
    pub stderr: bool,
}

#[derive(Clone, Debug, Args)]
#[allow(clippy::struct_excessive_bools)] // Clap exposes one flag per gate.
pub struct DaemonPrivArgs {
    /// Report every gate and change nothing. This is the default.
    #[arg(long, conflicts_with_all = ["grant_firewall", "grant_all", "prompt"])]
    pub check: bool,
    /// Add and unblock the installed binary in the Application Firewall.
    #[arg(long)]
    pub grant_firewall: bool,
    /// Apply every gate that can be granted programmatically.
    #[arg(long)]
    pub grant_all: bool,
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

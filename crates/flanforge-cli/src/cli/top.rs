use std::path::PathBuf;

use clap::{Parser, Subcommand};

use super::{AllocationArgs, ConfigArgs, DaemonArgs, ProfileArgs, ReaperArgs};

#[derive(Debug, Parser)]
#[command(version, about = "Tart-backed Forgejo runner control plane")]
pub struct Arguments {
    #[arg(short = 'c', long, global = true, value_name = "FILE")]
    pub config: Option<PathBuf>,
    #[command(subcommand)]
    pub command: TopCommand,
}

#[derive(Debug, Subcommand)]
pub enum TopCommand {
    /// Run, install, or control the macOS service.
    Daemon(DaemonArgs),
    /// Inspect the TOML configuration.
    Config(ConfigArgs),
    /// Create, inspect, or edit project profiles.
    Profile(Box<ProfileArgs>),
    /// List or cancel allocations on the running service.
    Allocation(AllocationArgs),
    /// Sweep clones and images nothing refers to.
    Reaper(ReaperArgs),
}

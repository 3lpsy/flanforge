use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::{
    allocation::AllocationArgs, config::ConfigArgs, daemon::DaemonArgs, hot::HotArgs,
    image::ImageArgs, overrides::ConfigOverrideArgument, profile::ProfileArgs, reaper::ReaperArgs,
    runtime::RuntimeArgs, webui::WebuiArgs,
};

#[derive(Debug, Parser)]
#[command(version, about = "Ephemeral Forgejo runner control plane")]
pub struct Arguments {
    #[arg(short = 'c', long, global = true, value_name = "FILE")]
    pub config: Option<PathBuf>,
    /// Override one typed configuration key after TOML and environment values.
    #[arg(long = "set", global = true, value_name = "KEY=VALUE")]
    pub overrides: Vec<ConfigOverrideArgument>,
    #[command(subcommand)]
    pub command: TopCommand,
}

#[derive(Debug, Subcommand)]
pub enum TopCommand {
    /// Run, install, or control the native service.
    Daemon(DaemonArgs),
    /// Inspect the TOML configuration.
    Config(ConfigArgs),
    /// Create, inspect, or edit project profiles.
    Profile(Box<ProfileArgs>),
    /// List or cancel allocations on the running service.
    Allocation(AllocationArgs),
    /// Sweep clones and images nothing refers to.
    Reaper(ReaperArgs),
    /// List, drain, or evict the machines the hot pool holds.
    Hot(HotArgs),
    /// Inspect or immutably import libvirt base images.
    Image(ImageArgs),
    /// Exercise a configured runtime without a Forgejo job.
    Runtime(RuntimeArgs),
    /// Manage the web UI on the running service.
    Webui(WebuiArgs),
}

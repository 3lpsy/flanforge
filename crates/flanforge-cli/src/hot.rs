use clap::{Args, Subcommand};
use flanforge_core::VmName;

#[derive(Debug, Args)]
pub struct HotArgs {
    #[command(subcommand)]
    pub command: HotCommand,
}

#[derive(Clone, Debug, Subcommand)]
pub enum HotCommand {
    /// List the machines the hot pool holds.
    List(HotListArgs),
    /// Stop a machine taking new claims, and destroy it when its claim ends.
    Drain(HotRetireArgs),
    /// Destroy a machine now, claim or no claim.
    Evict(HotRetireArgs),
}

#[derive(Clone, Copy, Debug, Args)]
pub struct HotListArgs {
    /// Print the service response verbatim instead of a table.
    #[arg(long)]
    pub json: bool,
}

#[derive(Clone, Debug, Args)]
pub struct HotRetireArgs {
    #[arg(value_name = "VM")]
    pub vm_name: VmName,
}

use clap::{Args, Subcommand};
use flanforge_core::AllocationId;

#[derive(Debug, Args)]
pub struct AllocationArgs {
    #[command(subcommand)]
    pub command: AllocationCommand,
}

#[derive(Clone, Copy, Debug, Subcommand)]
pub enum AllocationCommand {
    /// List what the running service knows about its allocations.
    List(AllocationListArgs),
    /// Cancel one allocation by ID and tear its guest down.
    Cancel(AllocationCancelArgs),
}

#[derive(Clone, Copy, Debug, Args)]
pub struct AllocationListArgs {
    /// Print the service response verbatim instead of a table.
    #[arg(long)]
    pub json: bool,
}

#[derive(Clone, Copy, Debug, Args)]
pub struct AllocationCancelArgs {
    #[arg(value_name = "ID")]
    pub id: AllocationId,
}

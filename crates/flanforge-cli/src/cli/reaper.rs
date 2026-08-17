use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct ReaperArgs {
    #[command(subcommand)]
    pub command: ReaperCommand,
}

#[derive(Clone, Copy, Debug, Subcommand)]
pub enum ReaperCommand {
    /// Sweep unreferenced clones and images. Plans only unless `--delete`.
    Run(ReaperRunArgs),
}

#[derive(Clone, Copy, Debug, Args)]
pub struct ReaperRunArgs {
    /// Delete the planned candidates instead of only reporting them.
    #[arg(long)]
    pub delete: bool,
}

use clap::{Args, Subcommand};
use flanforge_core::ProfileName;

#[derive(Debug, Args)]
pub struct RuntimeArgs {
    #[command(subcommand)]
    pub command: RuntimeCommand,
}

#[derive(Clone, Debug, Subcommand)]
pub enum RuntimeCommand {
    /// Boot, verify, and always remove one profile-sized disposable VM.
    Smoke(RuntimeSmokeArgs),
}

#[derive(Clone, Debug, Args)]
pub struct RuntimeSmokeArgs {
    /// Server-owned profile supplying image, size, network, and timeouts.
    #[arg(long, value_name = "PROFILE")]
    pub profile: ProfileName,
}

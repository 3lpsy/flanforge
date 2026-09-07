use clap::{Args, Subcommand, ValueEnum};
use flanforge_core::{ProfileName, RepositoryName, RunnerLabel, VmName};

#[derive(Debug, Args)]
pub struct ProfileArgs {
    #[command(subcommand)]
    pub command: ProfileCommand,
}

#[derive(Debug, Subcommand)]
pub enum ProfileCommand {
    /// Add a fully specified profile to the selected config.
    Create(Box<ProfileCreateArgs>),
    /// Print a profile or one of its keys.
    Get(ProfileGetArgs),
    /// Replace one profile key and atomically validate/save the config.
    Set(ProfileSetArgs),
}

#[derive(Debug, Args)]
pub struct ProfileCreateArgs {
    #[arg(short = 'p', long, value_name = "NAME")]
    pub profile: ProfileName,
    #[arg(long)]
    pub repository: RepositoryName,
    #[arg(long)]
    pub template: VmName,
    #[arg(long)]
    pub runner_label: RunnerLabel,
    #[arg(long)]
    pub job_name: String,
    #[arg(long = "allowed-workflow", required = true)]
    pub allowed_workflows: Vec<String>,
    #[arg(long = "allowed-event", required = true)]
    pub allowed_events: Vec<String>,
    /// Ref glob policy; a retired prefix `refs/tags/v` is now `refs/tags/v*`.
    #[arg(long = "allowed-ref", required = true)]
    pub allowed_refs: Vec<String>,
    #[arg(long, default_value_t = false, action = clap::ArgAction::Set)]
    pub require_protected_ref: bool,
    #[arg(long, value_enum, default_value_t = NetworkArgument::Default)]
    pub network: NetworkArgument,
    #[arg(long, default_value_t = 4)]
    pub cpu_count: u8,
    #[arg(long, default_value_t = 8_192)]
    pub memory_mb: u32,
    #[arg(long, default_value_t = 40_960)]
    pub storage_mb: u64,
    #[arg(long, default_value_t = 300)]
    pub boot_timeout_seconds: u64,
    #[arg(long, default_value_t = 600)]
    pub idle_timeout_seconds: u64,
    #[arg(long, default_value_t = 7_200)]
    pub job_timeout_seconds: u64,
    #[arg(long, default_value_t = 120)]
    pub cleanup_timeout_seconds: u64,
    /// Warm image this project's jobs may boot from.
    #[arg(long, value_name = "NAME")]
    pub warm_template: Option<VmName>,
    /// The only workflow allowed to produce that image.
    #[arg(long, value_name = "FILE")]
    pub regeneration_workflow: Option<String>,
    /// Let the periodic sweep retire this profile's unreferenced images.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub reap: bool,
}

#[derive(Debug, Args)]
pub struct ProfileGetArgs {
    #[arg(short = 'p', long, value_name = "NAME")]
    pub profile: ProfileName,
    #[arg(value_name = "KEY")]
    pub key: Option<String>,
}

#[derive(Debug, Args)]
pub struct ProfileSetArgs {
    #[arg(short = 'p', long, value_name = "NAME")]
    pub profile: ProfileName,
    #[arg(value_name = "KEY")]
    pub key: String,
    #[arg(value_name = "VALUE", allow_hyphen_values = true)]
    pub value: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum NetworkArgument {
    #[default]
    Default,
    Softnet,
}

impl From<NetworkArgument> for flanforge_core::NetworkMode {
    fn from(value: NetworkArgument) -> Self {
        match value {
            NetworkArgument::Default => Self::Default,
            NetworkArgument::Softnet => Self::Softnet,
        }
    }
}

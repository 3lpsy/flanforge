use std::path::PathBuf;

use clap::{ArgGroup, Args, Subcommand, ValueEnum};
use flanforge_core::ProfileName;

#[derive(Debug, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print all or one section of the selected TOML document.
    View(ConfigViewArgs),
    /// Write a commented starter TOML document the operator then edits.
    Generate(ConfigGenerateArgs),
}

#[derive(Debug, Args)]
pub struct ConfigGenerateArgs {
    /// Runtime-specific starter document. Defaults to the native backend.
    #[arg(long, value_enum, default_value_t)]
    pub backend: ConfigBackend,
    /// Absolute destination; defaults to the selected configuration path.
    #[arg(short = 'o', long, value_name = "PATH")]
    pub output: Option<PathBuf>,
    /// Replace an existing document instead of refusing to overwrite it.
    #[arg(long, default_value_t = false)]
    pub force: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum ConfigBackend {
    Tart,
    Libvirt,
}

impl Default for ConfigBackend {
    fn default() -> Self {
        match flanforge_core::RuntimeBackendKind::default() {
            flanforge_core::RuntimeBackendKind::Tart => Self::Tart,
            flanforge_core::RuntimeBackendKind::Libvirt => Self::Libvirt,
        }
    }
}

#[derive(Debug, Args)]
#[allow(clippy::struct_excessive_bools)] // Clap exposes mutually exclusive section flags.
#[command(group(
    ArgGroup::new("section")
        .args(["logging", "server", "oidc", "forgejo", "runtime", "guest", "tailscale", "profiles", "profile"])
        .multiple(false)
))]
pub struct ConfigViewArgs {
    /// Print the effective configuration: defaults, document, environment.
    #[arg(long)]
    pub resolved: bool,
    #[arg(long)]
    pub logging: bool,
    #[arg(long)]
    pub server: bool,
    #[arg(long)]
    pub oidc: bool,
    #[arg(long)]
    pub forgejo: bool,
    #[arg(long)]
    pub runtime: bool,
    #[arg(long)]
    pub guest: bool,
    #[arg(long)]
    pub tailscale: bool,
    #[arg(short = 'P', long)]
    pub profiles: bool,
    #[arg(short = 'p', long, value_name = "NAME")]
    pub profile: Option<ProfileName>,
}

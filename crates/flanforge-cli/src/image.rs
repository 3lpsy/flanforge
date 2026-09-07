use std::path::PathBuf;

use clap::{Args, Subcommand};
use flanforge_core::VmName;

#[derive(Debug, Args)]
pub struct ImageArgs {
    #[command(subcommand)]
    pub command: ImageCommand,
}

#[derive(Clone, Debug, Subcommand)]
pub enum ImageCommand {
    /// Verify a local image and manifest without changing runtime state.
    Inspect(ImageInspectArgs),
    /// Import and immutably publish a verified base image.
    Import(ImageImportArgs),
}

#[derive(Clone, Debug, Args)]
pub struct ImageInspectArgs {
    /// Local qcow2 artifact to verify.
    #[arg(long, value_name = "QCOW2")]
    pub image: PathBuf,
    /// Optional build manifest. Omit it and the image describes itself.
    #[arg(long, value_name = "JSON")]
    pub manifest: Option<PathBuf>,
}

#[derive(Clone, Debug, Args)]
pub struct ImageImportArgs {
    /// Immutable logical name referenced by configured profiles.
    #[arg(long, value_name = "NAME")]
    pub name: VmName,
    /// Local qcow2 artifact to verify and upload.
    #[arg(long, value_name = "QCOW2")]
    pub image: PathBuf,
    /// Optional build manifest. Omit it and the image describes itself.
    #[arg(long, value_name = "JSON")]
    pub manifest: Option<PathBuf>,
}

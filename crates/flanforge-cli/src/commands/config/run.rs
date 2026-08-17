use std::path::Path;

use anyhow::Result;

use crate::cli::ConfigCommand;

use super::{generate::generate, view::view};

/// Executes a configuration inspection or generation command.
///
/// # Errors
///
/// Returns an error when the selected document cannot be read or rendered,
/// when the selected section or profile does not exist, or when a starter
/// document cannot be written and validated.
pub async fn run_config_command(path: &Path, command: ConfigCommand) -> Result<()> {
    match command {
        ConfigCommand::View(arguments) => view(path, &arguments).await,
        ConfigCommand::Generate(arguments) => generate(path, &arguments).await,
    }
}

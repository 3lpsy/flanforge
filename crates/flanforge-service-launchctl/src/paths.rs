use std::{io, path::Path};

use anyhow::{Context, Result, bail};

use crate::constants::SERVICE_LABEL;

pub use flanforge_paths::LaunchdPaths;

/// Resolves the current user's `LaunchAgent` paths.
///
/// # Errors
/// Returns an error when the service home or derived paths are invalid.
pub fn discover() -> Result<LaunchdPaths> {
    let home = flanforge_paths::service_home().context("cannot resolve service home")?;
    flanforge_paths::launchd_paths(&home, SERVICE_LABEL).context("cannot resolve launchctl paths")
}

pub(crate) async fn is_present(path: &Path) -> Result<bool> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.is_file() || metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => bail!("{} is not a regular file or symlink", path.display()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => {
            Err(error).with_context(|| format!("cannot inspect service path {}", path.display()))
        }
    }
}

use std::{io, path::Path};

use anyhow::{Context, Result, bail};

pub use flanforge_paths::SystemdPaths;

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

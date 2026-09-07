use std::path::Path;

use anyhow::{Context, Result};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

/// Atomically installs the running executable at `destination`.
///
/// # Errors
/// Returns an error when the executable cannot be located, read, or published.
pub async fn install_current_binary(destination: &Path, mode: u32) -> Result<()> {
    let source = std::env::current_exe().context("cannot locate current executable")?;
    let bytes = tokio::fs::read(&source)
        .await
        .context("cannot read current executable")?;
    atomic_write(destination, &bytes, mode)
        .await
        .context("cannot install service binary")?;
    Ok(())
}

/// Atomically publishes `bytes` at `path` with an explicit Unix mode.
///
/// # Errors
/// Returns an error when the destination has no parent or publication fails.
pub async fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("destination has no parent"))?;
    let temporary = parent.join(format!(".flanforge-service-{}.tmp", Uuid::new_v4()));
    let mut options = tokio::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        options.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    let mut file = options
        .open(&temporary)
        .await
        .context("cannot create temporary service file")?;
    if let Err(error) = write_and_sync(&mut file, bytes).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error);
    }
    drop(file);
    if let Err(error) = tokio::fs::rename(&temporary, path).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error).context("cannot atomically install service file");
    }
    sync_parent(parent).await
}

async fn write_and_sync(file: &mut tokio::fs::File, bytes: &[u8]) -> Result<()> {
    file.write_all(bytes)
        .await
        .context("cannot write service file")?;
    file.flush().await.context("cannot flush service file")?;
    file.sync_all().await.context("cannot sync service file")
}

async fn sync_parent(parent: &Path) -> Result<()> {
    tokio::fs::File::open(parent)
        .await
        .context("cannot open service directory")?
        .sync_all()
        .await
        .context("cannot sync service directory")
}

#[cfg(test)]
mod tests;

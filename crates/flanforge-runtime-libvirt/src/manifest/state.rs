use flanforge_libvirt_wire::OwnershipManifest;
use tokio::io::AsyncWriteExt;

use crate::RuntimeError;

pub(crate) async fn create_private_directory(path: &std::path::Path) -> Result<(), RuntimeError> {
    let mut builder = tokio::fs::DirBuilder::new();
    builder.mode(0o700).recursive(false);
    builder.create(path).await.map_err(RuntimeError::manifest)
}

pub(crate) async fn write_known_hosts(
    path: &std::path::Path,
    contents: &str,
) -> Result<(), RuntimeError> {
    let directory = path
        .parent()
        .ok_or_else(|| RuntimeError::manifest("host-key anchor has no parent directory"))?
        .to_owned();
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options.open(path).await.map_err(RuntimeError::manifest)?;
    file.write_all(contents.as_bytes())
        .await
        .map_err(RuntimeError::manifest)?;
    file.sync_all().await.map_err(RuntimeError::manifest)?;
    sync_directory(directory).await
}

pub(crate) async fn save_async(
    manifest: OwnershipManifest,
    path: std::path::PathBuf,
) -> Result<(), RuntimeError> {
    tokio::task::spawn_blocking(move || super::save(&manifest, &path))
        .await
        .map_err(|_| RuntimeError::manifest("ownership save task failed"))?
}

async fn sync_directory(directory: std::path::PathBuf) -> Result<(), RuntimeError> {
    tokio::task::spawn_blocking(move || std::fs::File::open(directory)?.sync_all())
        .await
        .map_err(|_| RuntimeError::manifest("directory sync task failed"))?
        .map_err(RuntimeError::manifest)
}

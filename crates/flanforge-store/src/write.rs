use std::{
    fs::File,
    path::{Path, PathBuf},
};

use serde::Serialize;
use tokio::{
    fs::{self, OpenOptions},
    io::AsyncWriteExt,
};
use uuid::Uuid;

use super::StoreError;

/// Writes a private record atomically: a fresh 0600 temporary, fsync, rename,
/// then a directory fsync, so a crash never publishes a partial record.
pub(crate) async fn write_private_json<T: Serialize + Sync>(
    directory: &Path,
    destination: &Path,
    value: &T,
) -> Result<(), StoreError> {
    let temporary = directory.join(format!(".{}.tmp", Uuid::new_v4()));
    let bytes = serde_json::to_vec_pretty(value)?;

    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).await?;
    if let Err(error) = async {
        file.write_all(&bytes).await?;
        file.write_all(b"\n").await?;
        file.flush().await?;
        file.sync_all().await
    }
    .await
    {
        let _ = fs::remove_file(&temporary).await;
        return Err(StoreError::Io(error));
    }
    fs::rename(&temporary, destination).await?;
    sync_directory(directory.to_path_buf()).await
}

/// Renames an unusable record aside so it is reported once and never reloaded.
pub(crate) async fn quarantine(path: &Path) {
    let destination = path.with_extension("json.corrupt");
    if let Err(error) = fs::rename(path, &destination).await {
        tracing::error!(path = %path.display(), %error, "cannot quarantine unreadable state record");
    } else {
        tracing::error!(path = %path.display(), quarantined = %destination.display(), "quarantined unreadable state record");
    }
}

pub(crate) async fn ensure_private_directory(path: &Path) -> Result<(), StoreError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).await?;
    }
    Ok(())
}

async fn sync_directory(path: PathBuf) -> Result<(), StoreError> {
    tokio::task::spawn_blocking(move || File::open(path)?.sync_all())
        .await
        .map_err(|_| StoreError::Sync)??;
    Ok(())
}

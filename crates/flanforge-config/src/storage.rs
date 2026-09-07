use std::{fs::File, io, path::Path};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use toml_edit::DocumentMut;
use uuid::Uuid;

use crate::ConfigLoadError;

pub(crate) const MAX_CONFIG_BYTES: u64 = 1_048_576;

pub(crate) async fn read_document_text(path: &Path) -> Result<String, ConfigLoadError> {
    let mut options = tokio::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let file = options.open(path).await.map_err(map_open_error)?;
    let metadata = file.metadata().await.map_err(ConfigLoadError::Inspect)?;
    if !metadata.is_file() {
        return Err(ConfigLoadError::NotRegular);
    }
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(ConfigLoadError::TooLarge);
    }
    let capacity = usize::try_from(metadata.len())
        .map_err(|_| ConfigLoadError::TooLarge)?
        .saturating_add(1);
    let mut bytes = Vec::with_capacity(capacity);
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(ConfigLoadError::Read)?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(ConfigLoadError::TooLarge);
    }
    String::from_utf8(bytes).map_err(|error| ConfigLoadError::Utf8(error.utf8_error()))
}

fn map_open_error(error: io::Error) -> ConfigLoadError {
    #[cfg(unix)]
    if error.raw_os_error() == Some(libc::ELOOP) {
        return ConfigLoadError::NotRegular;
    }
    ConfigLoadError::Inspect(error)
}

/// Renders the format-preserving document, so comments and layout survive.
pub(crate) async fn write_document(
    path: &Path,
    document: &DocumentMut,
) -> Result<(), ConfigLoadError> {
    write_config_text(path, &document.to_string()).await
}

/// Durably replaces a configuration file with owner-only TOML text.
///
/// # Errors
/// Returns an error when the replacement cannot be written, renamed, or synced.
pub async fn write_config_text(path: &Path, text: &str) -> Result<(), ConfigLoadError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary = parent.join(format!(".flanforge-config-{}.tmp", Uuid::new_v4()));
    let mut options = tokio::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&temporary)
        .await
        .map_err(ConfigLoadError::Write)?;
    if let Err(error) = write_and_sync(&mut file, text.as_bytes()).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(ConfigLoadError::Write(error));
    }
    drop(file);
    if let Err(error) = tokio::fs::rename(&temporary, path).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(ConfigLoadError::Write(error));
    }
    sync_parent(parent).await
}

async fn write_and_sync(file: &mut tokio::fs::File, bytes: &[u8]) -> io::Result<()> {
    file.write_all(bytes).await?;
    file.flush().await?;
    file.sync_all().await
}

async fn sync_parent(parent: &Path) -> Result<(), ConfigLoadError> {
    let directory = parent.to_owned();
    tokio::task::spawn_blocking(move || File::open(directory)?.sync_all())
        .await
        .map_err(|_| ConfigLoadError::Sync)?
        .map_err(ConfigLoadError::Write)
}

use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    time::Duration,
};

use fs2::FileExt;

use super::error::ConfigLoadError;

const ACQUIRE_ATTEMPTS: u32 = 50;
const ACQUIRE_RETRY: Duration = Duration::from_millis(100);

/// Serializes configuration writers — the daemon's edit endpoint and the
/// CLI's `profile`/`config` commands — around their open→edit→save sequence.
/// A hand edit in an editor is outside it and lands last-writer-wins through
/// the atomic rename, never as corruption.
#[derive(Debug)]
pub struct ConfigWriteLock {
    _file: File,
}

impl ConfigWriteLock {
    /// Acquires `<config dir>/.config.lock`, waiting briefly for a concurrent
    /// writer rather than failing on the first contention.
    ///
    /// # Errors
    ///
    /// Returns an error when the lock stays held past the bounded wait.
    pub async fn acquire(config_path: &Path) -> Result<Self, ConfigLoadError> {
        let lock_path = lock_path(config_path);
        tokio::task::spawn_blocking(move || acquire_blocking(&lock_path))
            .await
            .map_err(|_| ConfigLoadError::LockUnavailable)?
    }
}

fn lock_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(".config.lock")
}

fn acquire_blocking(lock_path: &Path) -> Result<ConfigWriteLock, ConfigLoadError> {
    if let Some(parent) = lock_path.parent()
        && !parent.as_os_str().is_empty()
    {
        // `config generate` may be creating the very first file here.
        let _ = std::fs::create_dir_all(parent);
    }
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // A lock file that cannot even be created is a read-only deployment
    // (a mounted config), not a concurrent writer; the two answers differ.
    let file = options
        .open(lock_path)
        .map_err(|_| ConfigLoadError::LockUnwritable)?;
    for _ in 0..ACQUIRE_ATTEMPTS {
        if FileExt::try_lock_exclusive(&file).is_ok() {
            return Ok(ConfigWriteLock { _file: file });
        }
        std::thread::sleep(ACQUIRE_RETRY);
    }
    Err(ConfigLoadError::LockUnavailable)
}

use std::{
    fmt,
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
};

use fs2::FileExt;
use tokio::fs;

use super::StoreError;

/// Exclusive authority to mutate runtime state and its owned resources.
pub struct StateMutationLock {
    directory: PathBuf,
    _file: File,
}

impl fmt::Debug for StateMutationLock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StateMutationLock")
            .field("directory", &self.directory)
            .finish_non_exhaustive()
    }
}

impl StateMutationLock {
    /// Creates the private state directory and acquires its process-wide lock.
    ///
    /// # Errors
    /// Returns an error when the directory is unsafe or another process owns
    /// the mutation lock.
    pub async fn acquire(directory: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let directory = directory.into();
        fs::create_dir_all(&directory).await?;
        ensure_private_directory(&directory).await?;
        let lock_path = directory.join("instance.lock");
        let file = open_private_lock(&lock_path)?;
        FileExt::try_lock_exclusive(&file).map_err(|source| StoreError::Lock {
            path: lock_path,
            source,
        })?;
        tracing::info!(directory = %directory.display(), "runtime mutation state locked");
        Ok(Self {
            directory,
            _file: file,
        })
    }

    /// Reports whether this lock protects the given state directory.
    #[must_use]
    pub fn is_for(&self, directory: &Path) -> bool {
        self.directory == directory
    }
}

async fn ensure_private_directory(path: &Path) -> Result<(), StoreError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).await?;
    }
    Ok(())
}

fn open_private_lock(path: &std::path::Path) -> Result<File, StoreError> {
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

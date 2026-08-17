use std::{
    fmt,
    fs::{File, OpenOptions as StdOpenOptions},
    path::{Path, PathBuf},
    time::SystemTime,
};

use async_trait::async_trait;
use flanforge_core::{Allocation, AllocationId};
use fs2::FileExt;
use tokio::fs;
use validator::Validate;

use super::{
    StoreError,
    write::{ensure_private_directory, quarantine, write_private_json},
};

#[async_trait]
pub trait AllocationStore: fmt::Debug + Send + Sync {
    async fn load_recent(&self, limit: usize) -> Result<Vec<Allocation>, StoreError>;
    async fn save(&self, allocation: &Allocation) -> Result<(), StoreError>;
}

pub struct JsonStateStore {
    directory: PathBuf,
    _instance_lock: File,
}

impl fmt::Debug for JsonStateStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JsonStateStore")
            .field("directory", &self.directory)
            .finish_non_exhaustive()
    }
}

impl JsonStateStore {
    /// Opens the state directory and acquires its single-instance lock.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory cannot be created or another
    /// daemon owns the lock.
    pub async fn open(directory: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let directory = directory.into();
        fs::create_dir_all(&directory).await?;
        ensure_private_directory(&directory).await?;
        let lock_path = directory.join("instance.lock");
        let lock = open_private_lock(&lock_path)?;
        FileExt::try_lock_exclusive(&lock).map_err(|source| StoreError::Lock {
            path: lock_path,
            source,
        })?;
        tracing::info!(directory = %directory.display(), "allocation state store locked");
        Ok(Self {
            directory,
            _instance_lock: lock,
        })
    }

    fn allocation_path(&self, id: AllocationId) -> PathBuf {
        self.directory.join(format!("{id}.json"))
    }
}

#[async_trait]
impl AllocationStore for JsonStateStore {
    async fn load_recent(&self, limit: usize) -> Result<Vec<Allocation>, StoreError> {
        let mut directory = fs::read_dir(&self.directory).await?;
        let mut paths = Vec::new();
        while let Some(entry) = directory.next_entry().await? {
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                paths.push((entry.metadata().await?.modified()?, path));
            }
        }
        retain_recent(&mut paths, limit);
        tracing::debug!(
            files = paths.len(),
            limit,
            "loading recent allocation state"
        );

        let mut allocations = Vec::with_capacity(paths.len());
        let mut quarantined = 0_usize;
        for (_, path) in paths {
            let bytes = fs::read(&path).await?;
            // A record that cannot be read cannot be proven to own a VM or a
            // runner registration, so keep it for forensics and keep booting.
            let allocation = match serde_json::from_slice::<Allocation>(&bytes) {
                Ok(allocation) if allocation.validate().is_ok() => allocation,
                Ok(_) => {
                    quarantine(&path).await;
                    quarantined += 1;
                    continue;
                }
                Err(source) => {
                    tracing::error!(path = %path.display(), %source, "allocation state record is unreadable");
                    quarantine(&path).await;
                    quarantined += 1;
                    continue;
                }
            };
            allocations.push(allocation);
        }
        tracing::info!(
            allocations = allocations.len(),
            quarantined,
            "allocation state loaded"
        );
        Ok(allocations)
    }

    async fn save(&self, allocation: &Allocation) -> Result<(), StoreError> {
        let destination = self.allocation_path(allocation.id);
        allocation
            .validate()
            .map_err(|_| StoreError::InvalidAllocation {
                path: destination.clone(),
            })?;
        write_private_json(&self.directory, &destination, allocation).await?;
        tracing::trace!(allocation_id = %allocation.id, state = ?allocation.state, "allocation state persisted");
        Ok(())
    }
}

pub(super) fn retain_recent(paths: &mut Vec<(SystemTime, PathBuf)>, limit: usize) {
    paths.sort_by(|(left_time, left_path), (right_time, right_path)| {
        right_time
            .cmp(left_time)
            .then_with(|| right_path.cmp(left_path))
    });
    paths.truncate(limit);
}

fn open_private_lock(path: &Path) -> Result<File, StoreError> {
    let mut options = StdOpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

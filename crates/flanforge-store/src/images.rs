use std::{
    fmt,
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use flanforge_core::{ProfileName, WarmImageRecord};
use tokio::fs;
use validator::Validate;

use super::{
    StoreError,
    write::{ensure_private_directory, quarantine, write_private_json},
};

#[async_trait]
pub trait WarmImageStore: fmt::Debug + Send + Sync {
    async fn load_all(&self) -> Result<Vec<WarmImageRecord>, StoreError>;
    async fn load(&self, profile: &ProfileName) -> Result<Option<WarmImageRecord>, StoreError>;
    async fn save(&self, record: &WarmImageRecord) -> Result<(), StoreError>;
    async fn remove(&self, profile: &ProfileName) -> Result<(), StoreError>;
}

/// One record per profile under `<state_dir>/images`, inside the directory the
/// allocation store already holds exclusively, so it takes no second lock.
pub struct JsonWarmImageStore {
    directory: PathBuf,
}

impl fmt::Debug for JsonWarmImageStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JsonWarmImageStore")
            .field("directory", &self.directory)
            .finish_non_exhaustive()
    }
}

impl JsonWarmImageStore {
    /// Opens the image directory inside the already-locked state directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory cannot be created or made private.
    pub async fn open(state_dir: impl AsRef<Path>) -> Result<Self, StoreError> {
        let directory = state_dir.as_ref().join("images");
        fs::create_dir_all(&directory).await?;
        ensure_private_directory(&directory).await?;
        tracing::debug!(directory = %directory.display(), "warm image store opened");
        Ok(Self { directory })
    }

    fn record_path(&self, profile: &ProfileName) -> PathBuf {
        self.directory.join(format!("{profile}.json"))
    }

    /// Decodes one record, quarantining anything that cannot be trusted so a
    /// bad file degrades to "no warm image" instead of wedging the daemon.
    async fn read_record(&self, path: &Path) -> Option<WarmImageRecord> {
        let bytes = match fs::read(path).await {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::error!(path = %path.display(), %error, "cannot read warm image record");
                return None;
            }
        };
        match serde_json::from_slice::<WarmImageRecord>(&bytes) {
            Ok(record) if record.validate().is_ok() => Some(record),
            Ok(_) => {
                quarantine(path).await;
                None
            }
            Err(source) => {
                tracing::error!(path = %path.display(), %source, "warm image record is unreadable");
                quarantine(path).await;
                None
            }
        }
    }
}

#[async_trait]
impl WarmImageStore for JsonWarmImageStore {
    async fn load_all(&self) -> Result<Vec<WarmImageRecord>, StoreError> {
        let mut directory = fs::read_dir(&self.directory).await?;
        let mut records = Vec::new();
        while let Some(entry) = directory.next_entry().await? {
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            if let Some(record) = self.read_record(&path).await {
                records.push(record);
            }
        }
        tracing::debug!(records = records.len(), "warm image records loaded");
        Ok(records)
    }

    async fn load(&self, profile: &ProfileName) -> Result<Option<WarmImageRecord>, StoreError> {
        let path = self.record_path(profile);
        if !fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(None);
        }
        // A record filed under another profile cannot describe this one, so it
        // is treated as absent rather than trusted.
        Ok(self
            .read_record(&path)
            .await
            .filter(|record| &record.profile == profile))
    }

    async fn save(&self, record: &WarmImageRecord) -> Result<(), StoreError> {
        let destination = self.record_path(&record.profile);
        record
            .validate()
            .map_err(|_| StoreError::InvalidWarmImage {
                path: destination.clone(),
            })?;
        write_private_json(&self.directory, &destination, record).await?;
        tracing::info!(
            profile = %record.profile,
            warm_template = %record.warm_template,
            generation = record.generation,
            state = ?record.state,
            "warm image record persisted"
        );
        Ok(())
    }

    async fn remove(&self, profile: &ProfileName) -> Result<(), StoreError> {
        let path = self.record_path(profile);
        match fs::remove_file(&path).await {
            Ok(()) => {
                tracing::info!(%profile, "warm image record removed");
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(StoreError::Io(error)),
        }
    }
}

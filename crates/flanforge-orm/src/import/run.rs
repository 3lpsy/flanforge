use std::path::{Path, PathBuf};

use flanforge_core::{Allocation, HotGuest, WarmImageRecord, unix_time};
use sea_orm::{DatabaseConnection, DbErr};
use serde::de::DeserializeOwned;
use thiserror::Error;
use validator::Validate;

use super::meta::{self, IMPORT_COMPLETED_KEY};
use crate::stores::{SqliteAllocationStore, SqliteHotGuestStore, SqliteWarmImageStore};
use flanforge_store::{AllocationStore, HotGuestStore, StoreError, WarmImageStore};

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("database operation failed: {0}")]
    Db(#[from] DbErr),
    #[error("state I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("import write failed: {0}")]
    Store(#[from] StoreError),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ImportReport {
    pub allocations: usize,
    pub warm_images: usize,
    pub hot_guests: usize,
    pub corrupt: usize,
}

/// One-time backfill of the per-file JSON state into the database. Idempotent:
/// upserts are safe, files move to `json-archive/` only after their table is
/// written, and the completion mark is written last, so a crash mid-import
/// simply re-runs. Returns `None` when a previous run already completed.
///
/// Must run under the state mutation lock, after migrations, before recovery.
///
/// # Errors
///
/// Returns an error when the database or the archive move fails; a record
/// that will not parse is renamed aside and counted, never fatal.
pub async fn import_json_state(
    connection: &DatabaseConnection,
    state_dir: &Path,
) -> Result<Option<ImportReport>, ImportError> {
    if meta::read(connection, IMPORT_COMPLETED_KEY)
        .await?
        .is_some()
    {
        return Ok(None);
    }
    let mut report = ImportReport::default();
    let archive_root = state_dir.join("json-archive");

    let store = SqliteAllocationStore::new(connection.clone());
    let records = read_records::<Allocation>(state_dir, &mut report).await?;
    report.allocations = records.len();
    for (path, record) in records {
        store.save(&record).await?;
        archive(&path, &archive_root.join("allocations")).await?;
    }

    let store = SqliteWarmImageStore::new(connection.clone());
    let records = read_records::<WarmImageRecord>(&state_dir.join("images"), &mut report).await?;
    report.warm_images = records.len();
    for (path, record) in records {
        store.save(&record).await?;
        archive(&path, &archive_root.join("images")).await?;
    }

    let store = SqliteHotGuestStore::new(connection.clone());
    let records = read_records::<HotGuest>(&state_dir.join("hot"), &mut report).await?;
    report.hot_guests = records.len();
    for (path, record) in records {
        store.save(&record).await?;
        archive(&path, &archive_root.join("hot")).await?;
    }

    meta::write(connection, IMPORT_COMPLETED_KEY, &unix_time().to_string()).await?;
    Ok(Some(report))
}

/// Reads every `*.json` record in one directory. A record that will not parse
/// or validate is renamed to `*.json.corrupt` — kept for forensics, counted,
/// and never fatal, matching the JSON stores' quarantine behavior.
async fn read_records<T: DeserializeOwned + Validate>(
    directory: &Path,
    report: &mut ImportReport,
) -> Result<Vec<(PathBuf, T)>, ImportError> {
    let mut records = Vec::new();
    let mut entries = match tokio::fs::read_dir(directory).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(records),
        Err(error) => return Err(error.into()),
    };
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json")
            || !entry.file_type().await?.is_file()
        {
            continue;
        }
        let bytes = tokio::fs::read(&path).await?;
        match serde_json::from_slice::<T>(&bytes) {
            Ok(record) if record.validate().is_ok() => records.push((path, record)),
            _ => {
                report.corrupt += 1;
                let mut quarantined = path.clone().into_os_string();
                quarantined.push(".corrupt");
                if let Err(error) = tokio::fs::rename(&path, &quarantined).await {
                    tracing::error!(path = %path.display(), %error, "cannot quarantine record");
                } else {
                    tracing::error!(path = %path.display(), "unreadable record quarantined");
                }
            }
        }
    }
    Ok(records)
}

/// Moves an imported file into the archive; same filesystem, so a rename.
async fn archive(path: &Path, archive_dir: &Path) -> Result<(), ImportError> {
    tokio::fs::create_dir_all(archive_dir).await?;
    let file_name = path.file_name().unwrap_or_default();
    tokio::fs::rename(path, archive_dir.join(file_name)).await?;
    Ok(())
}

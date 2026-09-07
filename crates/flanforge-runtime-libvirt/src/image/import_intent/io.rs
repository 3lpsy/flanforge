use std::{
    collections::BTreeSet,
    fs::{File, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use fs2::FileExt;
use uuid::Uuid;

use crate::{
    RuntimeError,
    image::{StagedImage, ensure_staging_directory},
};

use super::ImportIntent;

#[derive(Debug)]
pub(crate) struct ImportJournal {
    directory: PathBuf,
    _lock: File,
}

impl ImportJournal {
    pub(crate) fn open(state_dir: &Path) -> Result<Self, RuntimeError> {
        let directory = state_dir.join("libvirt").join("imports");
        ensure_staging_directory(&directory)?;
        let lock_path = directory.join("import.lock");
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(lock_path)
            .map_err(RuntimeError::manifest)?;
        let metadata = lock.metadata().map_err(RuntimeError::manifest)?;
        if !metadata.file_type().is_file()
            || metadata.len() > 0
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(RuntimeError::manifest(
                "import lock is not an empty regular file",
            ));
        }
        match lock.try_lock_exclusive() {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                return Err(RuntimeError::ActorUnavailable);
            }
            Err(error) => return Err(RuntimeError::manifest(error)),
        }
        Ok(Self {
            directory,
            _lock: lock,
        })
    }

    pub(crate) fn record(
        &self,
        logical_name: flanforge_core::VmName,
        staged: &StagedImage,
    ) -> Result<ImportIntent, RuntimeError> {
        if staged.path.parent() != Some(self.directory.as_path()) {
            return Err(RuntimeError::ownership(
                "staged image is outside the import journal",
            ));
        }
        let intent = ImportIntent::new(logical_name, staged)?;
        let path = self.directory.join(intent.file_name());
        let temporary = self
            .directory
            .join(format!(".import-intent-{}.tmp", Uuid::new_v4()));
        let bytes = intent.encode()?;
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            std::fs::hard_link(&temporary, path)?;
            std::fs::remove_file(&temporary)?;
            sync_directory(&self.directory)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result.map_err(RuntimeError::manifest)?;
        Ok(intent)
    }

    pub(crate) fn pending(&self) -> Result<Vec<ImportIntent>, RuntimeError> {
        let mut intents = Vec::new();
        for entry in bounded_entries(&self.directory)? {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return Err(RuntimeError::manifest("import state filename is not UTF-8"));
            };
            if !is_intent_name(name) {
                continue;
            }
            let bytes =
                crate::file::read_bounded_regular(&entry.path(), 64 * 1_024, "import intent")?;
            let intent = ImportIntent::parse(&bytes)?;
            if intent.file_name() != name {
                return Err(RuntimeError::manifest(
                    "import intent filename disagrees with content",
                ));
            }
            intents.push(intent);
        }
        Ok(intents)
    }

    pub(crate) fn mark_created(
        &self,
        intent: &ImportIntent,
        volume_key: String,
    ) -> Result<ImportIntent, RuntimeError> {
        let updated = intent.with_volume_key(volume_key)?;
        let path = self.directory.join(intent.file_name());
        let temporary = self
            .directory
            .join(format!(".import-intent-{}.tmp", Uuid::new_v4()));
        let bytes = updated.encode()?;
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            let metadata = std::fs::symlink_metadata(&path)?;
            if !metadata.file_type().is_file() {
                return Err(std::io::Error::other(
                    "import intent destination is not a regular file",
                ));
            }
            std::fs::rename(&temporary, path)?;
            sync_directory(&self.directory)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result.map_err(RuntimeError::manifest)?;
        Ok(updated)
    }

    pub(crate) fn finish(&self, intent: &ImportIntent) -> Result<(), RuntimeError> {
        remove_regular_if_present(&self.directory.join(intent.staged_file_name()))?;
        remove_regular_if_present(&self.directory.join(intent.file_name()))?;
        sync_directory(&self.directory).map_err(RuntimeError::manifest)
    }

    pub(crate) fn remove_orphan_stages(
        &self,
        pending: &[ImportIntent],
    ) -> Result<(), RuntimeError> {
        let referenced = pending
            .iter()
            .map(|intent| self.directory.join(intent.staged_file_name()))
            .collect::<BTreeSet<_>>();
        for entry in bounded_entries(&self.directory)? {
            let path = entry.path();
            if referenced.contains(&path) {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                return Err(RuntimeError::manifest("import state filename is not UTF-8"));
            };
            let is_stage = name
                .strip_prefix("import-")
                .and_then(|value| value.strip_suffix(".qcow2"))
                .is_some_and(|value| Uuid::parse_str(value).is_ok());
            let is_temporary = name
                .strip_prefix(".import-intent-")
                .and_then(|value| value.strip_suffix(".tmp"))
                .is_some_and(|value| Uuid::parse_str(value).is_ok());
            if is_stage || is_temporary {
                remove_regular_if_present(&path)?;
            }
        }
        sync_directory(&self.directory).map_err(RuntimeError::manifest)
    }
}

fn bounded_entries(directory: &Path) -> Result<Vec<std::fs::DirEntry>, RuntimeError> {
    let entries = std::fs::read_dir(directory)
        .map_err(RuntimeError::manifest)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(RuntimeError::manifest)?;
    if entries.len() > 1_024 {
        return Err(RuntimeError::manifest("import state has too many entries"));
    }
    Ok(entries)
}

fn remove_regular_if_present(path: &Path) -> Result<(), RuntimeError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            std::fs::remove_file(path).map_err(RuntimeError::manifest)
        }
        Ok(_) => Err(RuntimeError::manifest(
            "refusing to remove non-file import state",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(RuntimeError::manifest(error)),
    }
}

fn sync_directory(directory: &Path) -> std::io::Result<()> {
    File::open(directory)?.sync_all()
}

fn is_intent_name(name: &str) -> bool {
    name.strip_prefix("import-")
        .and_then(|value| value.strip_suffix(".intent.json"))
        .is_some_and(|value| Uuid::parse_str(value).is_ok())
}

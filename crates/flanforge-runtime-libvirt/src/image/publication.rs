use std::{
    fs::OpenOptions,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

use flanforge_core::VmName;
use flanforge_libvirt_wire::{MAX_PUBLISHED_BASE_BYTES, PublishedBase};
use uuid::Uuid;

use crate::RuntimeError;

#[derive(Debug, thiserror::Error)]
#[error("{source}")]
pub(crate) struct PublishError {
    source: RuntimeError,
    is_committed: bool,
}

impl PublishError {
    pub(crate) const fn is_committed(&self) -> bool {
        self.is_committed
    }

    pub(crate) fn into_runtime_error(self) -> RuntimeError {
        self.source
    }

    fn uncommitted(source: RuntimeError) -> Self {
        Self {
            source,
            is_committed: false,
        }
    }

    fn committed(source: RuntimeError) -> Self {
        Self {
            source,
            is_committed: true,
        }
    }
}

pub(crate) fn publication_path(directory: &Path, logical_name: &VmName) -> PathBuf {
    directory.join(format!("{}.published.json", logical_name.as_str()))
}

pub(crate) fn load_published(
    directory: &Path,
    logical_name: &VmName,
) -> Result<PublishedBase, RuntimeError> {
    let path = publication_path(directory, logical_name);
    let bytes = crate::file::read_bounded_regular(
        &path,
        MAX_PUBLISHED_BASE_BYTES as u64,
        "published base pointer",
    )?;
    let publication = PublishedBase::parse(&bytes).map_err(RuntimeError::manifest)?;
    if publication.logical_name() != logical_name.as_str() {
        return Err(RuntimeError::manifest(
            "published base pointer names another logical image",
        ));
    }
    Ok(publication)
}

pub(crate) fn publish(directory: &Path, publication: &PublishedBase) -> Result<(), PublishError> {
    publish_with_finalizer(directory, publication, finalize)
}

/// Replaces an existing publication in place: the supersede arm of the
/// startup auto-import. The rename is the commit, so a crash leaves either
/// the old record or the new one, never a torn mix.
pub(crate) fn publish_replace(
    directory: &Path,
    publication: &PublishedBase,
) -> Result<(), PublishError> {
    publication
        .ensure_valid()
        .map_err(RuntimeError::manifest)
        .map_err(PublishError::uncommitted)?;
    std::fs::create_dir_all(directory)
        .map_err(RuntimeError::manifest)
        .map_err(PublishError::uncommitted)?;
    let logical = VmName::new(publication.logical_name())
        .map_err(RuntimeError::manifest)
        .map_err(PublishError::uncommitted)?;
    let path = publication_path(directory, &logical);
    let temporary = directory.join(format!(".published-{}.tmp", Uuid::new_v4()));
    let bytes = publication
        .encode()
        .map_err(RuntimeError::manifest)
        .map_err(PublishError::uncommitted)?;
    let before_commit = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        Ok::<(), std::io::Error>(())
    })();
    if let Err(error) = before_commit {
        let _ = std::fs::remove_file(&temporary);
        return Err(PublishError::uncommitted(RuntimeError::manifest(error)));
    }
    if let Err(error) = std::fs::rename(&temporary, &path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(PublishError::uncommitted(RuntimeError::manifest(error)));
    }
    OpenOptions::new()
        .read(true)
        .open(directory)
        .and_then(|file| file.sync_all())
        .map_err(RuntimeError::manifest)
        .map_err(PublishError::committed)
}

fn publish_with_finalizer(
    directory: &Path,
    publication: &PublishedBase,
    finalizer: impl FnOnce(&Path, &Path) -> Result<(), RuntimeError>,
) -> Result<(), PublishError> {
    publication
        .ensure_valid()
        .map_err(RuntimeError::manifest)
        .map_err(PublishError::uncommitted)?;
    std::fs::create_dir_all(directory)
        .map_err(RuntimeError::manifest)
        .map_err(PublishError::uncommitted)?;
    let logical = VmName::new(publication.logical_name())
        .map_err(RuntimeError::manifest)
        .map_err(PublishError::uncommitted)?;
    let path = publication_path(directory, &logical);
    let temporary = directory.join(format!(".published-{}.tmp", Uuid::new_v4()));
    let bytes = publication
        .encode()
        .map_err(RuntimeError::manifest)
        .map_err(PublishError::uncommitted)?;
    let before_commit = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        Ok::<(), std::io::Error>(())
    })();
    if let Err(error) = before_commit {
        let _ = std::fs::remove_file(&temporary);
        return Err(PublishError::uncommitted(RuntimeError::manifest(error)));
    }

    match std::fs::hard_link(&temporary, &path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing =
                load_published(directory, &logical).map_err(PublishError::uncommitted)?;
            if existing != *publication {
                let _ = std::fs::remove_file(&temporary);
                return Err(PublishError::uncommitted(RuntimeError::manifest(
                    "a different immutable base is already published under this name",
                )));
            }
        }
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            return Err(PublishError::uncommitted(RuntimeError::manifest(error)));
        }
    }

    finalizer(directory, &temporary).map_err(PublishError::committed)
}

fn finalize(directory: &Path, temporary: &Path) -> Result<(), RuntimeError> {
    std::fs::remove_file(temporary).map_err(RuntimeError::manifest)?;
    OpenOptions::new()
        .read(true)
        .open(directory)
        .and_then(|file| file.sync_all())
        .map_err(RuntimeError::manifest)
}

#[cfg(test)]
pub(crate) fn publish_with_post_commit_failure(
    directory: &Path,
    publication: &PublishedBase,
) -> Result<(), PublishError> {
    publish_with_finalizer(directory, publication, |_directory, _temporary| {
        Err(RuntimeError::manifest("forced post-commit failure"))
    })
}

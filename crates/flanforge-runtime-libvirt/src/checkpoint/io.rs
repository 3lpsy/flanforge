use std::{
    fs::{File, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};

use flanforge_libvirt_wire::{MAX_VOLUME_CHECKPOINT_BYTES, VolumeCheckpoint};
use uuid::Uuid;

use crate::{RuntimeError, file::read_private_bounded_regular_optional};

pub(crate) fn load(path: &Path) -> Result<Option<VolumeCheckpoint>, RuntimeError> {
    let Some(bytes) = read_private_bounded_regular_optional(
        path,
        MAX_VOLUME_CHECKPOINT_BYTES as u64,
        "volume checkpoint",
    )?
    else {
        return Ok(None);
    };
    VolumeCheckpoint::parse(&bytes)
        .map(Some)
        .map_err(RuntimeError::manifest)
}

pub(crate) fn create(checkpoint: &VolumeCheckpoint, path: &Path) -> Result<(), RuntimeError> {
    write(checkpoint, path, false)
}

pub(crate) fn save(checkpoint: &VolumeCheckpoint, path: &Path) -> Result<(), RuntimeError> {
    write(checkpoint, path, true)
}

fn write(checkpoint: &VolumeCheckpoint, path: &Path, replace: bool) -> Result<(), RuntimeError> {
    let bytes = checkpoint.encode().map_err(RuntimeError::manifest)?;
    let directory = path
        .parent()
        .ok_or_else(|| RuntimeError::manifest("checkpoint path has no parent"))?;
    ensure_private_directory(directory)?;
    let temporary = directory.join(format!(".volume-checkpoint-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        if replace {
            ensure_regular_destination(path)?;
            std::fs::rename(&temporary, path)?;
        } else {
            std::fs::hard_link(&temporary, path)?;
            std::fs::remove_file(&temporary)?;
        }
        File::open(directory)?.sync_all()
    })();
    if result.is_err() {
        let _ = remove_regular_if_present(&temporary);
    }
    result.map_err(RuntimeError::manifest)
}

pub(crate) fn remove(path: &Path) -> Result<(), RuntimeError> {
    remove_regular_if_present(path)?;
    let directory = path
        .parent()
        .ok_or_else(|| RuntimeError::manifest("checkpoint path has no parent"))?;
    File::open(directory)
        .and_then(|directory| directory.sync_all())
        .map_err(RuntimeError::manifest)
}

fn ensure_private_directory(path: &Path) -> Result<(), RuntimeError> {
    let metadata = std::fs::symlink_metadata(path).map_err(RuntimeError::manifest)?;
    if !metadata.file_type().is_dir() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(RuntimeError::manifest(
            "checkpoint directory is not a private directory",
        ));
    }
    Ok(())
}

// The octal mask names the group and other bits a reader checks for; a
// trailing-zero count does not.
#[allow(clippy::verbose_bit_mask)]
fn ensure_regular_destination(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.file_type().is_file() && metadata.permissions().mode() & 0o077 == 0 =>
        {
            Ok(())
        }
        Ok(_) => Err(std::io::Error::other(
            "checkpoint destination is not a private regular file",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn remove_regular_if_present(path: &Path) -> Result<(), RuntimeError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            std::fs::remove_file(path).map_err(RuntimeError::manifest)
        }
        Ok(_) => Err(RuntimeError::manifest(
            "refusing to remove non-file checkpoint state",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(RuntimeError::manifest(error)),
    }
}

use std::{
    fs::{File, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use uuid::Uuid;

use crate::RuntimeError;

/// Writes bytes to a fresh 0600 temporary beside their destination, synced.
///
/// # Errors
/// Returns an error when the directory or the temporary cannot be written.
pub(crate) fn write_private_temporary(
    directory: &Path,
    prefix: &str,
    bytes: &[u8],
) -> Result<PathBuf, RuntimeError> {
    let temporary = directory.join(format!(".{prefix}-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(RuntimeError::manifest(error));
    }
    Ok(temporary)
}

/// Replaces a private regular destination atomically, then fsyncs the parent.
///
/// The deliberate difference from cold publication: a cold base is written
/// once by `hard_link` and refuses a different pointer, while repointing is
/// the whole of a warm promotion.
///
/// # Errors
/// Returns an error when the destination is not a private regular file, or
/// when the rename or the directory sync fails.
pub(crate) fn ensure_replaced(temporary: &Path, destination: &Path) -> Result<(), RuntimeError> {
    let directory = destination
        .parent()
        .ok_or_else(|| RuntimeError::manifest("publication path has no parent"))?;
    let result = (|| {
        ensure_private_regular_destination(destination)?;
        std::fs::rename(temporary, destination)?;
        File::open(directory)?.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result.map_err(RuntimeError::manifest)
}

/// Removes a regular file and fsyncs its directory; absent is success.
///
/// # Errors
/// Returns an error when the path is not a regular file, or when removal or
/// the directory sync fails.
pub(crate) fn ensure_removed(path: &Path) -> Result<(), RuntimeError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            std::fs::remove_file(path).map_err(RuntimeError::manifest)?;
        }
        Ok(_) => {
            return Err(RuntimeError::manifest(
                "refusing to remove non-file publication state",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(RuntimeError::manifest(error)),
    }
    let directory = path
        .parent()
        .ok_or_else(|| RuntimeError::manifest("publication path has no parent"))?;
    File::open(directory)
        .and_then(|directory| directory.sync_all())
        .map_err(RuntimeError::manifest)
}

// The octal mask names the group and other bits a reader checks for; a
// trailing-zero count does not.
#[allow(clippy::verbose_bit_mask)]
fn ensure_private_regular_destination(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.file_type().is_file() && metadata.permissions().mode() & 0o077 == 0 =>
        {
            Ok(())
        }
        Ok(_) => Err(std::io::Error::other(
            "publication destination is not a private regular file",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

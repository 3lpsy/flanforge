use std::{
    fs::OpenOptions,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
};

use crate::RuntimeError;

pub(crate) fn read_bounded_regular(
    path: &Path,
    maximum_bytes: u64,
    description: &'static str,
) -> Result<Vec<u8>, RuntimeError> {
    read_bounded_regular_optional(path, maximum_bytes, description)?
        .ok_or_else(|| RuntimeError::manifest(format!("{description} is absent")))
}

pub(crate) fn read_bounded_regular_optional(
    path: &Path,
    maximum_bytes: u64,
    description: &'static str,
) -> Result<Option<Vec<u8>>, RuntimeError> {
    read_optional(path, maximum_bytes, description, false)
}

pub(crate) fn read_private_bounded_regular_optional(
    path: &Path,
    maximum_bytes: u64,
    description: &'static str,
) -> Result<Option<Vec<u8>>, RuntimeError> {
    read_optional(path, maximum_bytes, description, true)
}

fn read_optional(
    path: &Path,
    maximum_bytes: u64,
    description: &'static str,
    is_private: bool,
) -> Result<Option<Vec<u8>>, RuntimeError> {
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(RuntimeError::manifest(error)),
    };
    let before = file.metadata().map_err(RuntimeError::manifest)?;
    if !before.file_type().is_file()
        || before.len() > maximum_bytes
        || (is_private && before.permissions().mode() & 0o077 != 0)
    {
        return Err(RuntimeError::manifest(format!(
            "{description} is not a bounded regular file"
        )));
    }
    // Capacity is a hint; the read below is what enforces the bound.
    let mut bytes = Vec::with_capacity(usize::try_from(before.len()).unwrap_or(0));
    (&mut file)
        .take(maximum_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(RuntimeError::manifest)?;
    let after = file.metadata().map_err(RuntimeError::manifest)?;
    if bytes.len() as u64 > maximum_bytes || identity(&before) != identity(&after) {
        return Err(RuntimeError::manifest(format!(
            "{description} changed while it was read"
        )));
    }
    Ok(Some(bytes))
}

fn identity(metadata: &std::fs::Metadata) -> (u64, u64, u64, i64, i64) {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
    )
}

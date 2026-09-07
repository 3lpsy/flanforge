use std::{
    fs::{File, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};

use flanforge_libvirt_wire::{MAX_OWNERSHIP_MANIFEST_BYTES, OwnershipManifest};
use uuid::Uuid;

use crate::{RuntimeError, file::read_private_bounded_regular_optional};

use super::super::paths::is_ownership_temporary_name;

/// Bounds the orphan search so a corrupted state directory cannot make a sweep
/// walk without end.
const MAX_SCANNED_ALLOCATIONS: usize = 4_096;

pub(crate) fn load(path: &Path) -> Result<OwnershipManifest, RuntimeError> {
    let bytes = read_private_bounded_regular_optional(
        path,
        MAX_OWNERSHIP_MANIFEST_BYTES as u64,
        "ownership manifest",
    )?
    .ok_or(RuntimeError::ManifestNotFound)?;
    if bytes.is_empty() {
        return Err(RuntimeError::manifest("ownership manifest is empty"));
    }
    let manifest: OwnershipManifest =
        serde_json::from_slice(&bytes).map_err(RuntimeError::manifest)?;
    manifest.ensure_valid().map_err(RuntimeError::manifest)?;
    Ok(manifest)
}

/// The allocation whose durable manifest still claims this domain name.
///
/// A guest whose cleanup never finished keeps its manifest, and that manifest
/// outlives the allocation record: it is the only libvirt ownership evidence a
/// prefix-authorized orphan has, so a reap that finds none deletes nothing.
pub(crate) fn find_by_domain(state_dir: &Path, domain_name: &str) -> Result<Uuid, RuntimeError> {
    let allocations = flanforge_paths::libvirt_state_paths(state_dir).allocations;
    let mut scanned = 0_usize;
    for entry in std::fs::read_dir(&allocations).map_err(RuntimeError::manifest)? {
        let entry = entry.map_err(RuntimeError::manifest)?;
        scanned += 1;
        if scanned > MAX_SCANNED_ALLOCATIONS {
            return Err(RuntimeError::manifest(
                "libvirt state holds too many allocations to search",
            ));
        }
        let Some(allocation_id) = entry
            .file_name()
            .to_str()
            .and_then(|name| Uuid::parse_str(name).ok())
        else {
            continue;
        };
        if load(&super::super::paths::ownership(state_dir, allocation_id))
            .is_ok_and(|manifest| manifest.domain_name() == domain_name)
        {
            return Ok(allocation_id);
        }
    }
    Err(RuntimeError::ownership(
        "no libvirt ownership manifest claims that domain",
    ))
}

pub(crate) fn save(manifest: &OwnershipManifest, path: &Path) -> Result<(), RuntimeError> {
    manifest.ensure_valid().map_err(RuntimeError::manifest)?;
    let bytes = serde_json::to_vec(manifest).map_err(RuntimeError::manifest)?;
    if bytes.len() > MAX_OWNERSHIP_MANIFEST_BYTES {
        return Err(RuntimeError::manifest("ownership manifest is oversized"));
    }
    let directory = path
        .parent()
        .ok_or_else(|| RuntimeError::manifest("ownership path has no parent"))?;
    ensure_private_directory(directory)?;
    remove_stale_temporaries(directory)?;
    let temporary = directory.join(format!(".ownership-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_file() => {}
            Ok(_) => {
                return Err(std::io::Error::other(
                    "ownership destination is not a regular file",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        std::fs::rename(&temporary, path)?;
        sync_directory(directory)
    })();
    if result.is_err() {
        let _ = remove_regular_if_present(&temporary);
    }
    result.map_err(RuntimeError::manifest)
}

fn ensure_private_directory(path: &Path) -> Result<(), RuntimeError> {
    let metadata = std::fs::symlink_metadata(path).map_err(RuntimeError::manifest)?;
    if !metadata.file_type().is_dir() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(RuntimeError::manifest(
            "allocation state path is not a private directory",
        ));
    }
    Ok(())
}

fn remove_stale_temporaries(directory: &Path) -> Result<(), RuntimeError> {
    let entries = std::fs::read_dir(directory)
        .map_err(RuntimeError::manifest)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(RuntimeError::manifest)?;
    if entries.len() > 32 {
        return Err(RuntimeError::manifest(
            "allocation state has too many entries",
        ));
    }
    for entry in entries {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            return Err(RuntimeError::manifest(
                "allocation state filename is not UTF-8",
            ));
        };
        if is_ownership_temporary_name(&name) {
            remove_regular_if_present(&entry.path())?;
        }
    }
    Ok(())
}

fn remove_regular_if_present(path: &Path) -> Result<(), RuntimeError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            std::fs::remove_file(path).map_err(RuntimeError::manifest)
        }
        Ok(_) => Err(RuntimeError::manifest(
            "refusing to remove non-file ownership state",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(RuntimeError::manifest(error)),
    }
}

fn sync_directory(directory: &Path) -> std::io::Result<()> {
    File::open(directory)?.sync_all()
}

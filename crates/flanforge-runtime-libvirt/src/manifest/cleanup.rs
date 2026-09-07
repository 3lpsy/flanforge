use std::{
    fs::{File, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};

use flanforge_core::VmName;
use flanforge_libvirt_wire::{CleanupTombstone, MAX_CLEANUP_TOMBSTONE_BYTES, OwnershipManifest};
use uuid::Uuid;

use crate::{RuntimeError, file::read_private_bounded_regular_optional};

use super::{ServiceInstance, paths};

pub(crate) fn load(
    state_dir: &Path,
    allocation_id: Uuid,
) -> Result<Option<CleanupTombstone>, RuntimeError> {
    let directory = paths::allocation_dir(state_dir, allocation_id);
    remove_stale_temporaries(&directory)?;
    let Some(bytes) = read_private_bounded_regular_optional(
        &paths::cleanup(state_dir, allocation_id),
        MAX_CLEANUP_TOMBSTONE_BYTES as u64,
        "cleanup tombstone",
    )?
    else {
        return Ok(None);
    };
    CleanupTombstone::parse(&bytes)
        .map(Some)
        .map_err(RuntimeError::manifest)
}

pub(crate) fn record(
    state_dir: &Path,
    manifest: &OwnershipManifest,
) -> Result<CleanupTombstone, RuntimeError> {
    let directory = paths::allocation_dir(state_dir, manifest.allocation_id());
    ensure_private_directory(&directory)?;
    remove_stale_temporaries(&directory)?;
    let path = paths::cleanup(state_dir, manifest.allocation_id());
    if let Some(existing) = read(&path)? {
        ensure_matches_manifest(&existing, manifest)?;
        return Ok(existing);
    }
    let tombstone = CleanupTombstone::new(
        manifest.allocation_id(),
        manifest.service_instance(),
        manifest.domain_name().to_owned(),
        manifest.domain_uuid(),
        flanforge_utils::try_unix_time().map_err(RuntimeError::manifest)?,
    );
    let bytes = tombstone.encode().map_err(RuntimeError::manifest)?;
    let temporary = directory.join(format!(".cleanup-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)
            .map_err(RuntimeError::manifest)?;
        file.write_all(&bytes).map_err(RuntimeError::manifest)?;
        file.sync_all().map_err(RuntimeError::manifest)?;
        match std::fs::hard_link(&temporary, &path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = read(&path)?
                    .ok_or_else(|| RuntimeError::manifest("cleanup tombstone disappeared"))?;
                ensure_matches_manifest(&existing, manifest)?;
            }
            Err(error) => return Err(RuntimeError::manifest(error)),
        }
        std::fs::remove_file(&temporary).map_err(RuntimeError::manifest)?;
        sync_directory(&directory)?;
        Ok::<(), RuntimeError>(())
    })();
    if result.is_err() {
        let _ = remove_regular_if_present(&temporary);
    }
    result?;
    Ok(tombstone)
}

pub(crate) fn ensure_matches(
    tombstone: &CleanupTombstone,
    allocation_id: Uuid,
    vm_name: &VmName,
    instance: &ServiceInstance,
) -> Result<(), RuntimeError> {
    tombstone.ensure_valid().map_err(RuntimeError::manifest)?;
    if tombstone.allocation_id() != allocation_id
        || tombstone.domain_name() != vm_name.as_str()
        || tombstone.service_instance() != instance.id()
    {
        return Err(RuntimeError::ownership(
            "cleanup tombstone and durable allocation disagree",
        ));
    }
    Ok(())
}

fn ensure_matches_manifest(
    tombstone: &CleanupTombstone,
    manifest: &OwnershipManifest,
) -> Result<(), RuntimeError> {
    if tombstone.allocation_id() != manifest.allocation_id()
        || tombstone.service_instance() != manifest.service_instance()
        || tombstone.domain_name() != manifest.domain_name()
        || tombstone.domain_uuid() != manifest.domain_uuid()
    {
        return Err(RuntimeError::ownership(
            "cleanup tombstone and ownership manifest disagree",
        ));
    }
    Ok(())
}

fn read(path: &Path) -> Result<Option<CleanupTombstone>, RuntimeError> {
    let bytes = read_private_bounded_regular_optional(
        path,
        MAX_CLEANUP_TOMBSTONE_BYTES as u64,
        "cleanup tombstone",
    )?;
    bytes
        .map(|bytes| CleanupTombstone::parse(&bytes).map_err(RuntimeError::manifest))
        .transpose()
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
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries
            .collect::<Result<Vec<_>, _>>()
            .map_err(RuntimeError::manifest)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(RuntimeError::manifest(error)),
    };
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
        if paths::is_cleanup_temporary_name(&name) {
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
            "refusing to remove non-file cleanup state",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(RuntimeError::manifest(error)),
    }
}

fn sync_directory(directory: &Path) -> Result<(), RuntimeError> {
    File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(RuntimeError::manifest)
}

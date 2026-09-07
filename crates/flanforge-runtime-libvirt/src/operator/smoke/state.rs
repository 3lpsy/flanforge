use std::{
    fs::{DirBuilder, File},
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
};

use flanforge_libvirt_wire::OwnershipManifest;

use crate::{
    RuntimeError,
    checkpoint::{
        allocation_path as checkpoint_path, is_temporary_name, remove as remove_checkpoint,
    },
    manifest::{allocation_dir, create_private_directory, load, save_async, write_known_hosts},
};

const SMOKE_DIRECTORY: &str = "smoke";
const OWNERSHIP_FILE: &str = "ownership.json";

pub(super) async fn prepare(
    state_dir: &Path,
    manifest: &OwnershipManifest,
    known_hosts: &str,
) -> Result<(), RuntimeError> {
    ensure_smoke_directory(state_dir)?;
    save_async(manifest.clone(), ownership_path(state_dir)).await?;
    create_private_directory(&allocation_dir(state_dir, manifest.allocation_id())).await?;
    write_known_hosts(manifest.known_hosts_file(), known_hosts).await
}

pub(super) async fn persist(
    state_dir: &Path,
    manifest: &OwnershipManifest,
) -> Result<(), RuntimeError> {
    save_async(manifest.clone(), ownership_path(state_dir)).await
}

pub(super) async fn load_pending(
    state_dir: &Path,
) -> Result<Option<OwnershipManifest>, RuntimeError> {
    let path = ownership_path(state_dir);
    tokio::task::spawn_blocking(move || match load(&path) {
        Ok(manifest) => Ok(Some(manifest)),
        Err(RuntimeError::ManifestNotFound) => Ok(None),
        Err(error) => Err(error),
    })
    .await
    .map_err(|_| RuntimeError::manifest("smoke ownership load task failed"))?
}

pub(super) async fn remove(
    state_dir: &Path,
    manifest: &OwnershipManifest,
) -> Result<(), RuntimeError> {
    let state_dir = state_dir.to_owned();
    let manifest = manifest.clone();
    tokio::task::spawn_blocking(move || remove_blocking(&state_dir, &manifest))
        .await
        .map_err(|_| RuntimeError::manifest("smoke state cleanup task failed"))?
}

fn remove_blocking(state_dir: &Path, manifest: &OwnershipManifest) -> Result<(), RuntimeError> {
    let allocation = allocation_dir(state_dir, manifest.allocation_id());
    match std::fs::symlink_metadata(&allocation) {
        Ok(metadata)
            if metadata.file_type().is_dir()
                && metadata.permissions().mode().trailing_zeros() >= 6 =>
        {
            remove_checkpoint(&checkpoint_path(state_dir, manifest.allocation_id()))?;
            remove_regular_if_present(manifest.known_hosts_file())?;
            remove_checkpoint_temporaries(&allocation)?;
            remove_empty_directory(&allocation)?;
        }
        Ok(_) => {
            return Err(RuntimeError::manifest(
                "smoke allocation state is not a private directory",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(RuntimeError::manifest(error)),
    }
    remove_regular_if_present(&ownership_path(state_dir))?;
    let smoke = smoke_directory(state_dir);
    remove_empty_directory(&smoke)?;
    sync_existing_parent(&smoke)
}

fn ensure_smoke_directory(state_dir: &Path) -> Result<(), RuntimeError> {
    let path = smoke_directory(state_dir);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata)
            if metadata.file_type().is_dir()
                && metadata.permissions().mode().trailing_zeros() >= 6 =>
        {
            Ok(())
        }
        Ok(_) => Err(RuntimeError::manifest(
            "smoke state path is not a private directory",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = DirBuilder::new();
            builder.mode(0o700);
            builder.create(path).map_err(RuntimeError::manifest)
        }
        Err(error) => Err(RuntimeError::manifest(error)),
    }
}

fn remove_checkpoint_temporaries(directory: &Path) -> Result<(), RuntimeError> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries.collect::<Result<Vec<_>, _>>(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(RuntimeError::manifest(error)),
    }
    .map_err(RuntimeError::manifest)?;
    if entries.len() > 32 {
        return Err(RuntimeError::manifest("smoke state has too many entries"));
    }
    for entry in entries {
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| RuntimeError::manifest("smoke state filename is not UTF-8"))?;
        if !is_temporary_name(&name) {
            return Err(RuntimeError::manifest(
                "smoke allocation state contains an unexpected entry",
            ));
        }
        remove_regular_if_present(&entry.path())?;
    }
    Ok(())
}

fn remove_regular_if_present(path: &Path) -> Result<(), RuntimeError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            std::fs::remove_file(path).map_err(RuntimeError::manifest)
        }
        Ok(_) => Err(RuntimeError::manifest(
            "refusing to remove non-file smoke state",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(RuntimeError::manifest(error)),
    }
}

fn remove_empty_directory(path: &Path) -> Result<(), RuntimeError> {
    match std::fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(RuntimeError::manifest(error)),
    }
}

fn sync_existing_parent(path: &Path) -> Result<(), RuntimeError> {
    let parent = path
        .parent()
        .ok_or_else(|| RuntimeError::manifest("smoke state has no parent"))?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(RuntimeError::manifest)
}

fn smoke_directory(state_dir: &Path) -> PathBuf {
    flanforge_paths::libvirt_state_paths(state_dir)
        .root
        .join(SMOKE_DIRECTORY)
}

fn ownership_path(state_dir: &Path) -> PathBuf {
    smoke_directory(state_dir).join(OWNERSHIP_FILE)
}

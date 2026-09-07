use std::time::Duration;

use flanforge_core::Allocation;
use flanforge_libvirt_wire::OwnershipManifest;
use flanforge_manager::WorkerError;

use crate::{
    RuntimeError,
    checkpoint::{
        allocation_path as checkpoint_path, is_temporary_name as is_checkpoint_temporary_name,
        load as load_checkpoint, merge_allocation,
    },
    manifest::{
        allocation_dir, ensure_cleanup_matches, ensure_matches, is_cleanup_temporary_name,
        is_ownership_temporary_name, load, load_cleanup, path, record_cleanup,
    },
};

use super::LibvirtWorker;

#[derive(Debug)]
pub(super) enum GuestCleanup {
    Empty,
    /// Boxed so the owned arm does not widen every other outcome.
    Owned(Box<OwnershipManifest>),
    Tombstoned,
}

impl LibvirtWorker {
    pub(super) async fn cleanup_guest(
        &self,
        allocation: &Allocation,
        timeout: Duration,
    ) -> Result<GuestCleanup, WorkerError> {
        let state_dir = self.state_dir.clone();
        let allocation_id = allocation.id.into_uuid();
        let manifest_path = path(&state_dir, allocation_id);
        let loaded = tokio::task::spawn_blocking(move || load(&manifest_path)).await;
        let mut manifest = match loaded {
            Ok(Ok(manifest)) => manifest,
            Ok(Err(RuntimeError::ManifestNotFound)) if !allocation.vm_created => {
                let checkpoint = checkpoint_path(&self.state_dir, allocation_id);
                let checkpoint = tokio::task::spawn_blocking(move || load_checkpoint(&checkpoint))
                    .await
                    .map_err(|_| WorkerError::new("volume checkpoint task failed"))??;
                if checkpoint.is_some() {
                    return Err(WorkerError::new(
                        "volume checkpoint exists without allocation ownership",
                    ));
                }
                return Ok(GuestCleanup::Empty);
            }
            Ok(Err(RuntimeError::ManifestNotFound)) => {
                let state_dir = self.state_dir.clone();
                let loaded =
                    tokio::task::spawn_blocking(move || load_cleanup(&state_dir, allocation_id))
                        .await
                        .map_err(|_| WorkerError::new("cleanup tombstone task failed"))??;
                let tombstone = loaded.ok_or_else(|| {
                    WorkerError::new("ownership authority is absent for a created libvirt guest")
                })?;
                ensure_cleanup_matches(
                    &tombstone,
                    allocation_id,
                    &allocation.vm_name,
                    &self.instance,
                )?;
                let checkpoint = checkpoint_path(&self.state_dir, allocation_id);
                tokio::task::spawn_blocking(move || crate::checkpoint::remove(&checkpoint))
                    .await
                    .map_err(|_| WorkerError::new("volume checkpoint removal task failed"))??;
                return Ok(GuestCleanup::Tombstoned);
            }
            Ok(Err(error)) => return Err(error.into()),
            Err(_) => return Err(WorkerError::new("ownership load task failed")),
        };
        ensure_matches(
            &manifest,
            allocation_id,
            &allocation.vm_name,
            &self.instance,
            &self.state_dir,
        )?;
        let checkpoint_file = checkpoint_path(&self.state_dir, allocation_id);
        let checkpoint = {
            let path = checkpoint_file.clone();
            tokio::task::spawn_blocking(move || load_checkpoint(&path))
                .await
                .map_err(|_| WorkerError::new("volume checkpoint task failed"))??
        };
        if let Some(checkpoint) = checkpoint {
            merge_allocation(
                &checkpoint,
                &mut manifest,
                self.instance.id(),
                self.actor.pool(),
            )?;
            let recovered = manifest.clone();
            let ownership = path(&self.state_dir, allocation_id);
            tokio::task::spawn_blocking(move || crate::manifest::save(&recovered, &ownership))
                .await
                .map_err(|_| WorkerError::new("recovered ownership save task failed"))??;
        }
        self.actor.cleanup(manifest.clone(), timeout).await?;
        Ok(GuestCleanup::Owned(Box::new(manifest)))
    }
}

pub(super) async fn finalize_local_state(
    state_dir: &std::path::Path,
    allocation_id: uuid::Uuid,
    outcome: GuestCleanup,
) -> Result<(), WorkerError> {
    match outcome {
        GuestCleanup::Tombstoned => Ok(()),
        GuestCleanup::Owned(manifest) => {
            let state = state_dir.to_owned();
            tokio::task::spawn_blocking(move || record_cleanup(&state, &manifest))
                .await
                .map_err(|_| WorkerError::new("cleanup tombstone task failed"))??;
            remove_authority_files(state_dir, allocation_id).await
        }
        GuestCleanup::Empty => remove_empty_local_state(state_dir, allocation_id).await,
    }
}

pub(super) async fn remove_authority_files(
    state_dir: &std::path::Path,
    allocation_id: uuid::Uuid,
) -> Result<(), WorkerError> {
    let directory = allocation_dir(state_dir, allocation_id);
    match tokio::fs::symlink_metadata(&directory).await {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => {
            return Err(WorkerError::new("allocation state path is not a directory"));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(WorkerError::new("cannot inspect allocation state")),
    }
    remove_stale_temporaries(&directory).await?;
    for name in ["known_hosts", "ownership.json", "volume-checkpoint.json"] {
        let file = directory.join(name);
        match tokio::fs::symlink_metadata(&file).await {
            Ok(metadata) if metadata.file_type().is_file() => {
                tokio::fs::remove_file(&file)
                    .await
                    .map_err(|_| WorkerError::new("cannot remove allocation state file"))?;
            }
            Ok(_) => {
                return Err(WorkerError::new(
                    "refusing to remove non-file allocation state",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(WorkerError::new("cannot inspect allocation state")),
        }
    }
    sync_directory(directory.clone()).await
}

async fn remove_empty_local_state(
    state_dir: &std::path::Path,
    allocation_id: uuid::Uuid,
) -> Result<(), WorkerError> {
    let directory = allocation_dir(state_dir, allocation_id);
    remove_authority_files(state_dir, allocation_id).await?;
    match tokio::fs::remove_dir(&directory).await {
        Ok(()) => {
            let parent = directory
                .parent()
                .ok_or_else(|| WorkerError::new("allocation state has no parent"))?
                .to_owned();
            sync_directory(parent).await
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(WorkerError::new("allocation state directory is not empty")),
    }
}

async fn remove_stale_temporaries(directory: &std::path::Path) -> Result<(), WorkerError> {
    let mut entries = match tokio::fs::read_dir(directory).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(WorkerError::new("cannot inspect allocation state")),
    };
    let mut count = 0_u8;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|_| WorkerError::new("cannot inspect allocation state"))?
    {
        count = count
            .checked_add(1)
            .ok_or_else(|| WorkerError::new("allocation state has too many entries"))?;
        if count > 32 {
            return Err(WorkerError::new("allocation state has too many entries"));
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            return Err(WorkerError::new("allocation state filename is not UTF-8"));
        };
        if !is_ownership_temporary_name(&name)
            && !is_cleanup_temporary_name(&name)
            && !is_checkpoint_temporary_name(&name)
        {
            continue;
        }
        let metadata = tokio::fs::symlink_metadata(entry.path())
            .await
            .map_err(|_| WorkerError::new("cannot inspect allocation state"))?;
        if !metadata.file_type().is_file() {
            return Err(WorkerError::new(
                "refusing to remove non-file ownership state",
            ));
        }
        tokio::fs::remove_file(entry.path())
            .await
            .map_err(|_| WorkerError::new("cannot remove allocation state file"))?;
    }
    Ok(())
}

async fn sync_directory(directory: std::path::PathBuf) -> Result<(), WorkerError> {
    tokio::task::spawn_blocking(move || std::fs::File::open(directory)?.sync_all())
        .await
        .map_err(|_| WorkerError::new("allocation state sync task failed"))?
        .map_err(|_| WorkerError::new("cannot sync allocation state"))
}

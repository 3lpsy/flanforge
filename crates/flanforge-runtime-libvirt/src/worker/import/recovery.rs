use std::{path::Path, time::Duration};

use flanforge_libvirt_wire::{CheckpointVolume, CheckpointVolumeRole, PublishedBase};

use crate::{
    RuntimeError,
    actor::LibvirtActor,
    checkpoint::{import_key, import_path, load as load_checkpoint, remove as remove_checkpoint},
    image::{ImportIntent, ImportJournal, load_published, publication_path},
};

pub(crate) async fn reconcile_pending(
    journal: &ImportJournal,
    actor: &LibvirtActor,
    publication_directory: &Path,
    service_instance: uuid::Uuid,
    pool: &str,
    state_dir: &Path,
) -> Result<(), RuntimeError> {
    let pending = journal.pending()?;
    for intent in &pending {
        let checkpoint_path = import_path(state_dir, intent.import_id());
        let checkpoint = load_checkpoint(&checkpoint_path)?;
        // A checkpoint that names another instance, pool, or import stays
        // fatal; one that merely never recorded a volume key is a crash
        // window this loop exists to clear.
        let recovered_key = match checkpoint.as_ref() {
            Some(checkpoint) => {
                checkpoint
                    .ensure_import(
                        service_instance,
                        pool,
                        intent.import_id(),
                        &intent.volume_name(),
                    )
                    .map_err(RuntimeError::ownership)?;
                checkpoint
                    .volume(CheckpointVolumeRole::Base)
                    .map(CheckpointVolume::key)
            }
            None => None,
        };
        let intent = match (intent.volume_key(), recovered_key) {
            (None, Some(key)) => journal.mark_created(intent, key.to_owned())?,
            // No key was ever recorded, so the unique per-import volume name
            // is the one safe handle left; deleting by name is what keeps a
            // single crash from wedging every later import.
            (None, None) => {
                actor
                    .delete_volume(None, intent.volume_name(), Duration::from_secs(30))
                    .await?;
                remove_checkpoint(&checkpoint_path)?;
                journal.finish(intent)?;
                continue;
            }
            (Some(recorded), Some(key)) if recorded != key => {
                return Err(RuntimeError::ownership(
                    "import intent and helper checkpoint disagree",
                ));
            }
            _ => intent.clone(),
        };
        if !is_committed(publication_directory, &intent)? {
            actor
                .delete_volume(
                    Some(intent.owned_volume_key()?.to_owned()),
                    intent.volume_name(),
                    Duration::from_secs(30),
                )
                .await?;
        }
        remove_checkpoint(&checkpoint_path)?;
        journal.finish(&intent)?;
    }
    journal.remove_orphan_stages(&[])
}

pub(super) fn ensure_checkpoint_matches(
    intent: &ImportIntent,
    publication: &PublishedBase,
    service_instance: uuid::Uuid,
    pool: &str,
    state_dir: &Path,
) -> Result<(), RuntimeError> {
    let path = import_path(state_dir, intent.import_id());
    let checkpoint = load_checkpoint(&path)?.ok_or_else(|| {
        RuntimeError::ownership("successful import has no durable helper checkpoint")
    })?;
    let key = import_key(
        &checkpoint,
        service_instance,
        pool,
        intent.import_id(),
        &intent.volume_name(),
    )?;
    if key != publication.volume_key() || publication.volume_name() != intent.volume_name() {
        return Err(RuntimeError::ownership(
            "import reply and helper checkpoint disagree",
        ));
    }
    Ok(())
}

fn is_committed(directory: &Path, intent: &ImportIntent) -> Result<bool, RuntimeError> {
    let path = publication_path(directory, intent.logical_name());
    match std::fs::symlink_metadata(&path) {
        Ok(_) => load_published(directory, intent.logical_name()).map(|publication| {
            publication.volume_name() == intent.volume_name()
                && intent
                    .volume_key()
                    .is_some_and(|key| publication.volume_key() == key)
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(RuntimeError::manifest(error)),
    }
}

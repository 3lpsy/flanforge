use flanforge_libvirt_wire::{
    Artifact, CheckpointVolume, CheckpointVolumeRole, OwnershipManifest, VolumeCheckpoint,
};
use uuid::Uuid;

use crate::RuntimeError;

pub(crate) fn merge_allocation(
    checkpoint: &VolumeCheckpoint,
    manifest: &mut OwnershipManifest,
    service_instance: Uuid,
    pool: &str,
) -> Result<(), RuntimeError> {
    checkpoint
        .ensure_allocation(
            service_instance,
            pool,
            manifest.allocation_id(),
            manifest.overlay().name(),
            manifest.seed().name(),
        )
        .map_err(RuntimeError::ownership)?;
    merge_artifact(
        manifest.overlay_mut(),
        checkpoint.volume(CheckpointVolumeRole::Overlay),
    )?;
    merge_artifact(
        manifest.seed_mut(),
        checkpoint.volume(CheckpointVolumeRole::Seed),
    )?;
    manifest.ensure_valid().map_err(RuntimeError::manifest)
}

pub(crate) fn import_key<'a>(
    checkpoint: &'a VolumeCheckpoint,
    service_instance: Uuid,
    pool: &str,
    import_id: Uuid,
    volume_name: &str,
) -> Result<&'a str, RuntimeError> {
    checkpoint
        .ensure_import(service_instance, pool, import_id, volume_name)
        .map_err(RuntimeError::ownership)?;
    checkpoint
        .volume(CheckpointVolumeRole::Base)
        .map(CheckpointVolume::key)
        .ok_or_else(|| {
            RuntimeError::ownership("import checkpoint has no positive volume-key evidence")
        })
}

fn merge_artifact(
    artifact: &mut Artifact,
    checkpoint: Option<&CheckpointVolume>,
) -> Result<(), RuntimeError> {
    let Some(checkpoint) = checkpoint else {
        return Ok(());
    };
    if artifact.name() != checkpoint.name()
        || artifact.key().is_some_and(|key| key != checkpoint.key())
    {
        return Err(RuntimeError::ownership(
            "volume checkpoint disagrees with durable allocation ownership",
        ));
    }
    artifact.set_key(checkpoint.key().to_owned());
    Ok(())
}

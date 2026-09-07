use uuid::Uuid;

use crate::{
    WireError,
    validation::{is_safe_key, is_safe_name},
};

use super::{
    CheckpointOwner, CheckpointVolumeRole, VolumeCheckpoint,
    limits::{MAX_CHECKPOINT_VOLUMES, MAX_SAFE_NAME_BYTES, MAX_VOLUME_KEY_BYTES},
    model::CheckpointVolume,
};

const CONTRACT: &str = "libvirt volume checkpoint";

pub(super) fn ensure_valid(checkpoint: &VolumeCheckpoint) -> Result<(), WireError> {
    if checkpoint.schema_version != 1
        || checkpoint.service_instance.is_nil()
        || !is_safe_name(&checkpoint.pool, MAX_SAFE_NAME_BYTES)
        || checkpoint.volumes.len() > MAX_CHECKPOINT_VOLUMES
    {
        return invalid("structure");
    }
    let owner_id = match &checkpoint.owner {
        CheckpointOwner::Allocation { allocation_id } => *allocation_id,
        CheckpointOwner::Import { import_id } => *import_id,
        CheckpointOwner::Warm {
            profile,
            capture_id,
        } => {
            if !is_safe_name(profile, MAX_SAFE_NAME_BYTES) {
                return invalid("owner");
            }
            *capture_id
        }
    };
    if owner_id.is_nil() {
        return invalid("owner");
    }
    for (index, volume) in checkpoint.volumes.iter().enumerate() {
        ensure_volume(volume)?;
        if checkpoint.volumes[..index].iter().any(|other| {
            other.role == volume.role || other.name == volume.name || other.key == volume.key
        }) {
            return invalid("duplicate volume");
        }
    }
    match &checkpoint.owner {
        CheckpointOwner::Allocation { .. } => ensure_allocation_roles(&checkpoint.volumes),
        CheckpointOwner::Import { import_id } => {
            ensure_import_roles(&checkpoint.volumes, *import_id)
        }
        CheckpointOwner::Warm { capture_id, .. } => {
            ensure_warm_roles(&checkpoint.volumes, *capture_id)
        }
    }
}

pub(super) fn ensure_warm(
    checkpoint: &VolumeCheckpoint,
    service_instance: Uuid,
    pool: &str,
    profile: &str,
    capture_id: Uuid,
) -> Result<(), WireError> {
    ensure_valid(checkpoint)?;
    let expected = CheckpointOwner::Warm {
        profile: profile.to_owned(),
        capture_id,
    };
    if checkpoint.service_instance != service_instance
        || checkpoint.pool != pool
        || checkpoint.owner != expected
    {
        return invalid("warm binding");
    }
    Ok(())
}

pub(super) fn ensure_allocation(
    checkpoint: &VolumeCheckpoint,
    service_instance: Uuid,
    pool: &str,
    allocation_id: Uuid,
    overlay_name: &str,
    seed_name: &str,
) -> Result<(), WireError> {
    ensure_valid(checkpoint)?;
    if checkpoint.service_instance != service_instance
        || checkpoint.pool != pool
        || checkpoint.owner != (CheckpointOwner::Allocation { allocation_id })
        || checkpoint.volumes.iter().any(|volume| match volume.role {
            CheckpointVolumeRole::Overlay => volume.name != overlay_name,
            CheckpointVolumeRole::Seed => volume.name != seed_name,
            CheckpointVolumeRole::Base | CheckpointVolumeRole::Warm => true,
        })
    {
        return invalid("allocation binding");
    }
    Ok(())
}

pub(super) fn ensure_import(
    checkpoint: &VolumeCheckpoint,
    service_instance: Uuid,
    pool: &str,
    import_id: Uuid,
    volume_name: &str,
) -> Result<(), WireError> {
    ensure_valid(checkpoint)?;
    if checkpoint.service_instance != service_instance
        || checkpoint.pool != pool
        || checkpoint.owner != (CheckpointOwner::Import { import_id })
        || checkpoint
            .volumes
            .iter()
            .any(|volume| volume.role != CheckpointVolumeRole::Base || volume.name != volume_name)
    {
        return invalid("import binding");
    }
    Ok(())
}

fn ensure_volume(volume: &CheckpointVolume) -> Result<(), WireError> {
    if !is_safe_name(&volume.name, MAX_SAFE_NAME_BYTES)
        || !is_safe_key(&volume.key, MAX_VOLUME_KEY_BYTES)
    {
        return invalid("volume");
    }
    Ok(())
}

fn ensure_allocation_roles(volumes: &[CheckpointVolume]) -> Result<(), WireError> {
    if volumes.iter().any(|volume| {
        matches!(
            volume.role,
            CheckpointVolumeRole::Base | CheckpointVolumeRole::Warm
        )
    }) {
        return invalid("allocation role");
    }
    let overlay = volumes
        .iter()
        .find(|volume| volume.role == CheckpointVolumeRole::Overlay)
        .and_then(|volume| artifact_id(&volume.name, "root-", ".qcow2"));
    let seed = volumes
        .iter()
        .find(|volume| volume.role == CheckpointVolumeRole::Seed)
        .and_then(|volume| artifact_id(&volume.name, "seed-", ".img"));
    if overlay.is_none()
        && volumes
            .iter()
            .any(|volume| volume.role == CheckpointVolumeRole::Overlay)
        || seed.is_none()
            && volumes
                .iter()
                .any(|volume| volume.role == CheckpointVolumeRole::Seed)
        || overlay.is_some() && seed.is_some() && overlay != seed
    {
        return invalid("allocation volume name");
    }
    Ok(())
}

fn ensure_import_roles(volumes: &[CheckpointVolume], import_id: Uuid) -> Result<(), WireError> {
    if volumes.len() > 1
        || volumes.iter().any(|volume| {
            volume.role != CheckpointVolumeRole::Base
                || volume.name != format!("base-import-{import_id}.qcow2")
        })
    {
        return invalid("import role");
    }
    Ok(())
}

/// The name is derived from the capture id, so a checkpoint alone always names
/// the volume even when the process died before its key was recorded.
fn ensure_warm_roles(volumes: &[CheckpointVolume], capture_id: Uuid) -> Result<(), WireError> {
    if volumes.len() > 1
        || volumes.iter().any(|volume| {
            volume.role != CheckpointVolumeRole::Warm
                || volume.name != VolumeCheckpoint::warm_volume_name(capture_id)
        })
    {
        return invalid("warm role");
    }
    Ok(())
}

fn artifact_id(value: &str, prefix: &str, suffix: &str) -> Option<Uuid> {
    value
        .strip_prefix(prefix)
        .and_then(|value| value.strip_suffix(suffix))
        .and_then(|value| Uuid::parse_str(value).ok())
        .filter(|value| !value.is_nil())
}

fn invalid<T>(field: &'static str) -> Result<T, WireError> {
    Err(WireError::invalid(CONTRACT, field))
}

use std::path::PathBuf;

use flanforge_libvirt_wire::{
    CheckpointVolumeRole, HelperConfig, OwnershipManifest, VolumeCheckpoint,
};
use uuid::Uuid;

use crate::RuntimeError;

use super::{allocation_path, create, import_path, remove, save, warm_capture_dir, warm_path};

pub(crate) struct VolumeJournal {
    checkpoint: VolumeCheckpoint,
    path: PathBuf,
}

impl VolumeJournal {
    pub(crate) fn allocation(
        config: &HelperConfig,
        manifest: &OwnershipManifest,
    ) -> Result<Self, RuntimeError> {
        if manifest.service_instance() != config.service_instance {
            return Err(RuntimeError::ownership(
                "allocation checkpoint service instance disagrees with the helper",
            ));
        }
        let checkpoint = VolumeCheckpoint::allocation(
            config.service_instance,
            config.pool.clone(),
            manifest.allocation_id(),
        )
        .map_err(RuntimeError::manifest)?;
        Self::begin(
            checkpoint,
            allocation_path(&config.state_dir, manifest.allocation_id()),
        )
    }

    pub(crate) fn image_import(
        config: &HelperConfig,
        volume_name: &str,
    ) -> Result<Self, RuntimeError> {
        let import_id = volume_name
            .strip_prefix("base-import-")
            .and_then(|value| value.strip_suffix(".qcow2"))
            .and_then(|value| Uuid::parse_str(value).ok())
            .filter(|value| !value.is_nil())
            .ok_or_else(|| RuntimeError::manifest("import volume identity is invalid"))?;
        let checkpoint =
            VolumeCheckpoint::image_import(config.service_instance, config.pool.clone(), import_id)
                .map_err(RuntimeError::manifest)?;
        Self::begin(checkpoint, import_path(&config.state_dir, import_id))
    }

    /// Opens the durable intent for one warm capture. The volume name is
    /// derived from `capture_id`, so this checkpoint alone always names the
    /// volume — even when the process dies before the key is recorded.
    pub(crate) fn warm(
        config: &HelperConfig,
        profile: &str,
        capture_id: Uuid,
    ) -> Result<Self, RuntimeError> {
        let checkpoint = VolumeCheckpoint::warm(
            config.service_instance,
            config.pool.clone(),
            profile.to_owned(),
            capture_id,
        )
        .map_err(RuntimeError::manifest)?;
        crate::warm::ensure_private_directory(&warm_capture_dir(&config.state_dir))?;
        Self::begin(checkpoint, warm_path(&config.state_dir, capture_id))
    }

    fn begin(checkpoint: VolumeCheckpoint, path: PathBuf) -> Result<Self, RuntimeError> {
        if super::load(&path)?.is_some() {
            return Err(RuntimeError::ownership(
                "volume creation checkpoint already exists",
            ));
        }
        create(&checkpoint, &path)?;
        Ok(Self { checkpoint, path })
    }

    pub(crate) fn record(
        &mut self,
        role: CheckpointVolumeRole,
        name: String,
        key: String,
    ) -> Result<(), RuntimeError> {
        self.checkpoint
            .record(role, name, key)
            .map_err(RuntimeError::manifest)?;
        save(&self.checkpoint, &self.path)
    }

    pub(crate) fn finish(&self) -> Result<(), RuntimeError> {
        remove(&self.path)
    }
}

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::WireError;

use super::MAX_VOLUME_CHECKPOINT_BYTES;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CheckpointOwner {
    Allocation { allocation_id: Uuid },
    Import { import_id: Uuid },
    Warm { profile: String, capture_id: Uuid },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointVolumeRole {
    Overlay,
    Seed,
    Base,
    Warm,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointVolume {
    pub(super) role: CheckpointVolumeRole,
    pub(super) name: String,
    pub(super) key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VolumeCheckpoint {
    pub(super) schema_version: u8,
    pub(super) service_instance: Uuid,
    pub(super) pool: String,
    pub(super) owner: CheckpointOwner,
    pub(super) volumes: Vec<CheckpointVolume>,
}

impl VolumeCheckpoint {
    /// Starts an allocation checkpoint before its first volume mutation.
    ///
    /// # Errors
    /// Returns an error when any owner identity is structurally invalid.
    pub fn allocation(
        service_instance: Uuid,
        pool: String,
        allocation_id: Uuid,
    ) -> Result<Self, WireError> {
        Self::new(
            service_instance,
            pool,
            CheckpointOwner::Allocation { allocation_id },
        )
    }

    /// Starts an image-import checkpoint before its volume mutation.
    ///
    /// # Errors
    /// Returns an error when any owner identity is structurally invalid.
    pub fn image_import(
        service_instance: Uuid,
        pool: String,
        import_id: Uuid,
    ) -> Result<Self, WireError> {
        Self::new(
            service_instance,
            pool,
            CheckpointOwner::Import { import_id },
        )
    }

    /// Starts a warm-capture checkpoint before its volume mutation.
    ///
    /// The physical name is pinned to `warm-<capture_id>.qcow2`, so recovery
    /// can reconstruct it from the owner alone and delete by name when the
    /// process died before the key was recorded.
    ///
    /// # Errors
    /// Returns an error when any owner identity is structurally invalid.
    pub fn warm(
        service_instance: Uuid,
        pool: String,
        profile: String,
        capture_id: Uuid,
    ) -> Result<Self, WireError> {
        Self::new(
            service_instance,
            pool,
            CheckpointOwner::Warm {
                profile,
                capture_id,
            },
        )
    }

    /// The physical volume name a warm capture id always bears.
    #[must_use]
    pub fn warm_volume_name(capture_id: Uuid) -> String {
        format!("warm-{capture_id}.qcow2")
    }

    fn new(
        service_instance: Uuid,
        pool: String,
        owner: CheckpointOwner,
    ) -> Result<Self, WireError> {
        let checkpoint = Self {
            schema_version: 1,
            service_instance,
            pool,
            owner,
            volumes: Vec::new(),
        };
        super::validate::ensure_valid(&checkpoint)?;
        Ok(checkpoint)
    }

    /// Parses and validates one bounded checkpoint document.
    ///
    /// # Errors
    /// Returns an error for malformed, oversized, or invalid checkpoint data.
    pub fn parse(bytes: &[u8]) -> Result<Self, WireError> {
        if bytes.is_empty() || bytes.len() > MAX_VOLUME_CHECKPOINT_BYTES {
            return Err(WireError::invalid("libvirt volume checkpoint", "size"));
        }
        let checkpoint = serde_json::from_slice(bytes)
            .map_err(|_| WireError::decode("libvirt volume checkpoint"))?;
        super::validate::ensure_valid(&checkpoint)?;
        Ok(checkpoint)
    }

    /// Encodes a structurally valid bounded checkpoint document.
    ///
    /// # Errors
    /// Returns an error when this checkpoint is invalid or cannot be encoded.
    pub fn encode(&self) -> Result<Vec<u8>, WireError> {
        super::validate::ensure_valid(self)?;
        let bytes =
            serde_json::to_vec(self).map_err(|_| WireError::encode("libvirt volume checkpoint"))?;
        if bytes.len() > MAX_VOLUME_CHECKPOINT_BYTES {
            return Err(WireError::invalid("libvirt volume checkpoint", "size"));
        }
        Ok(bytes)
    }

    /// Adds one exact generated volume identity to the checkpoint.
    ///
    /// # Errors
    /// Returns an error for an invalid identity or a conflicting role.
    pub fn record(
        &mut self,
        role: CheckpointVolumeRole,
        name: String,
        key: String,
    ) -> Result<(), WireError> {
        if let Some(existing) = self.volumes.iter().find(|volume| volume.role == role) {
            if existing.name == name && existing.key == key {
                return Ok(());
            }
            return Err(WireError::invalid(
                "libvirt volume checkpoint",
                "role collision",
            ));
        }
        self.volumes.push(CheckpointVolume { role, name, key });
        if let Err(error) = super::validate::ensure_valid(self) {
            self.volumes.pop();
            return Err(error);
        }
        Ok(())
    }

    /// Requires an exact allocation, service-instance, pool, and name binding.
    ///
    /// # Errors
    /// Returns an error when any expected identity disagrees with the document.
    pub fn ensure_allocation(
        &self,
        service_instance: Uuid,
        pool: &str,
        allocation_id: Uuid,
        overlay_name: &str,
        seed_name: &str,
    ) -> Result<(), WireError> {
        super::validate::ensure_allocation(
            self,
            service_instance,
            pool,
            allocation_id,
            overlay_name,
            seed_name,
        )
    }

    /// Requires an exact warm-capture, service-instance, and pool binding.
    ///
    /// # Errors
    /// Returns an error when any expected identity disagrees with the document.
    pub fn ensure_warm(
        &self,
        service_instance: Uuid,
        pool: &str,
        profile: &str,
        capture_id: Uuid,
    ) -> Result<(), WireError> {
        super::validate::ensure_warm(self, service_instance, pool, profile, capture_id)
    }

    #[must_use]
    pub const fn owner(&self) -> &CheckpointOwner {
        &self.owner
    }

    /// Requires an exact import, service-instance, pool, and name binding.
    ///
    /// # Errors
    /// Returns an error when any expected identity disagrees with the document.
    pub fn ensure_import(
        &self,
        service_instance: Uuid,
        pool: &str,
        import_id: Uuid,
        volume_name: &str,
    ) -> Result<(), WireError> {
        super::validate::ensure_import(self, service_instance, pool, import_id, volume_name)
    }

    #[must_use]
    pub fn volume(&self, role: CheckpointVolumeRole) -> Option<&CheckpointVolume> {
        self.volumes.iter().find(|volume| volume.role == role)
    }
}

impl CheckpointVolume {
    #[must_use]
    pub const fn role(&self) -> CheckpointVolumeRole {
        self.role
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }
}

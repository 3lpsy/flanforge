use serde::{Deserialize, Serialize};

use super::{
    PublishedBase,
    limits::{MAX_LOGICAL_NAME_BYTES, MAX_VIRTUAL_IMAGE_BYTES, MAX_VOLUME_KEY_BYTES},
};
use crate::{
    WireError,
    validation::{is_safe_key, is_safe_name},
};

const CONTRACT: &str = "libvirt volume pointer";

/// The strict subset of a publication the boot path enforces: identity,
/// declared virtual size, and provenance. Digest and file name are never read
/// at boot, so a warm capture does not compute one it would not check.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VolumePointer {
    pool: String,
    volume_name: String,
    volume_key: String,
    virtual_bytes: u64,
    /// Per-pointer, so a superseded entry carries its own generation rather
    /// than being identified by its position in a list retirement reorders.
    #[serde(default)]
    generation: u64,
    /// Per-pointer, so a superseded entry has its own age. A single
    /// document-level timestamp makes the retirement age floor meaningless.
    #[serde(default)]
    produced_at_unix: u64,
}

impl VolumePointer {
    /// Builds a validated pointer at one generation.
    ///
    /// # Errors
    /// Returns an error when any identity or size field is invalid.
    pub fn new(
        pool: String,
        volume_name: String,
        volume_key: String,
        virtual_bytes: u64,
        generation: u64,
        produced_at_unix: u64,
    ) -> Result<Self, WireError> {
        let pointer = Self {
            pool,
            volume_name,
            volume_key,
            virtual_bytes,
            generation,
            produced_at_unix,
        };
        pointer.ensure_valid()?;
        Ok(pointer)
    }

    /// Applies the pointer's structural validation.
    ///
    /// A cold base pointer carries `generation` and `produced_at_unix` of
    /// zero, so neither is required to be non-zero here.
    ///
    /// # Errors
    /// Returns the first structurally invalid pointer field.
    pub fn ensure_valid(&self) -> Result<(), WireError> {
        for (field, value) in [
            ("pool", self.pool.as_str()),
            ("volume_name", self.volume_name.as_str()),
        ] {
            if !is_safe_name(value, MAX_LOGICAL_NAME_BYTES) {
                return Err(WireError::invalid(CONTRACT, field));
            }
        }
        if !is_safe_key(&self.volume_key, MAX_VOLUME_KEY_BYTES) {
            return Err(WireError::invalid(CONTRACT, "volume_key"));
        }
        if self.virtual_bytes == 0 || self.virtual_bytes > MAX_VIRTUAL_IMAGE_BYTES {
            return Err(WireError::invalid(CONTRACT, "virtual_bytes"));
        }
        Ok(())
    }

    #[must_use]
    pub fn pool(&self) -> &str {
        &self.pool
    }

    #[must_use]
    pub fn volume_name(&self) -> &str {
        &self.volume_name
    }

    #[must_use]
    pub fn volume_key(&self) -> &str {
        &self.volume_key
    }

    #[must_use]
    pub const fn virtual_bytes(&self) -> u64 {
        self.virtual_bytes
    }

    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub const fn produced_at_unix(&self) -> u64 {
        self.produced_at_unix
    }

    /// Two pointers naming the same physical volume, by either identity.
    #[must_use]
    pub fn is_same_volume(&self, other: &Self) -> bool {
        self.volume_name == other.volume_name || self.volume_key == other.volume_key
    }
}

impl PublishedBase {
    /// The boot-time subset of an immutable cold base. A cold base has no
    /// generation and no capture timestamp, so both read zero.
    ///
    /// # Errors
    /// Returns an error when the publication is not structurally valid.
    pub fn pointer(&self) -> Result<VolumePointer, WireError> {
        VolumePointer::new(
            self.pool().to_owned(),
            self.volume_name().to_owned(),
            self.volume_key().to_owned(),
            self.manifest().virtual_bytes(),
            0,
            0,
        )
    }
}

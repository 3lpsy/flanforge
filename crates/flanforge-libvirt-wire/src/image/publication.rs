use serde::{Deserialize, Serialize};

use super::{
    BaseImageManifest, MAX_PUBLISHED_BASE_BYTES,
    limits::{MAX_LOGICAL_NAME_BYTES, MAX_VOLUME_KEY_BYTES},
};
use crate::{
    WireError,
    validation::{is_safe_key, is_safe_name},
};

const CONTRACT: &str = "published libvirt base";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublishedBase {
    schema_version: u8,
    logical_name: String,
    pool: String,
    volume_name: String,
    volume_key: String,
    manifest: BaseImageManifest,
}

impl PublishedBase {
    /// Creates the immutable pointer published after a verified upload.
    ///
    /// # Errors
    /// Returns an error when an identity or manifest field is invalid.
    pub fn new(
        logical_name: String,
        pool: String,
        volume_name: String,
        volume_key: String,
        manifest: BaseImageManifest,
    ) -> Result<Self, WireError> {
        let publication = Self {
            schema_version: 1,
            logical_name,
            pool,
            volume_name,
            volume_key,
            manifest,
        };
        publication.ensure_valid()?;
        Ok(publication)
    }

    /// Decodes a bounded publication pointer.
    ///
    /// # Errors
    /// Returns an error for malformed, oversized, or invalid data.
    pub fn parse(bytes: &[u8]) -> Result<Self, WireError> {
        if bytes.is_empty() || bytes.len() > MAX_PUBLISHED_BASE_BYTES {
            return Err(WireError::invalid(CONTRACT, "size"));
        }
        let publication =
            serde_json::from_slice::<Self>(bytes).map_err(|_| WireError::decode(CONTRACT))?;
        publication.ensure_valid()?;
        Ok(publication)
    }

    /// Encodes a bounded publication pointer.
    ///
    /// # Errors
    /// Returns an error when validation or serialization fails.
    pub fn encode(&self) -> Result<Vec<u8>, WireError> {
        self.ensure_valid()?;
        serde_json::to_vec(self).map_err(|_| WireError::encode(CONTRACT))
    }

    /// Applies the v1 publication's structural validation.
    ///
    /// # Errors
    /// Returns the first structurally invalid publication field.
    pub fn ensure_valid(&self) -> Result<(), WireError> {
        if self.schema_version != 1 {
            return invalid("schema_version");
        }
        for (field, value) in [
            ("logical_name", self.logical_name.as_str()),
            ("pool", self.pool.as_str()),
            ("volume_name", self.volume_name.as_str()),
        ] {
            if !is_safe_name(value, MAX_LOGICAL_NAME_BYTES) {
                return invalid(field);
            }
        }
        if !is_safe_key(&self.volume_key, MAX_VOLUME_KEY_BYTES) {
            return invalid("volume_key");
        }
        self.manifest.ensure_valid()
    }

    #[must_use]
    pub fn logical_name(&self) -> &str {
        &self.logical_name
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
    pub const fn manifest(&self) -> &BaseImageManifest {
        &self.manifest
    }
}

fn invalid<T>(field: &'static str) -> Result<T, WireError> {
    Err(WireError::invalid(CONTRACT, field))
}

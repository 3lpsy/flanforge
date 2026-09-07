use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{WireError, validation::is_safe_name};

use super::{MAX_CLEANUP_TOMBSTONE_BYTES, limits::MAX_SAFE_NAME_BYTES};

const CONTRACT: &str = "libvirt cleanup tombstone";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupTombstone {
    schema_version: u8,
    allocation_id: Uuid,
    service_instance: Uuid,
    domain_name: String,
    domain_uuid: Uuid,
    cleaned_unix_seconds: u64,
}

impl CleanupTombstone {
    #[must_use]
    pub fn new(
        allocation_id: Uuid,
        service_instance: Uuid,
        domain_name: String,
        domain_uuid: Uuid,
        cleaned_unix_seconds: u64,
    ) -> Self {
        Self {
            schema_version: 1,
            allocation_id,
            service_instance,
            domain_name,
            domain_uuid,
            cleaned_unix_seconds,
        }
    }

    /// Parses and structurally validates a bounded cleanup tombstone.
    ///
    /// # Errors
    /// Returns an error for malformed, oversized, or invalid documents.
    pub fn parse(bytes: &[u8]) -> Result<Self, WireError> {
        if bytes.is_empty() || bytes.len() > MAX_CLEANUP_TOMBSTONE_BYTES {
            return Err(WireError::invalid(CONTRACT, "size"));
        }
        let tombstone: Self =
            serde_json::from_slice(bytes).map_err(|_| WireError::decode(CONTRACT))?;
        tombstone.ensure_valid()?;
        Ok(tombstone)
    }

    /// Encodes a structurally valid cleanup tombstone.
    ///
    /// # Errors
    /// Returns an error when the tombstone is invalid or cannot be encoded.
    pub fn encode(&self) -> Result<Vec<u8>, WireError> {
        self.ensure_valid()?;
        let bytes = serde_json::to_vec(self).map_err(|_| WireError::encode(CONTRACT))?;
        if bytes.len() > MAX_CLEANUP_TOMBSTONE_BYTES {
            return Err(WireError::invalid(CONTRACT, "size"));
        }
        Ok(bytes)
    }

    /// Applies structural validation owned by the wire contract.
    ///
    /// # Errors
    /// Returns the first invalid field in the v1 document.
    pub fn ensure_valid(&self) -> Result<(), WireError> {
        if self.schema_version != 1 {
            return Err(WireError::invalid(CONTRACT, "schema_version"));
        }
        for (field, id) in [
            ("allocation_id", self.allocation_id),
            ("service_instance", self.service_instance),
            ("domain_uuid", self.domain_uuid),
        ] {
            if id.is_nil() {
                return Err(WireError::invalid(CONTRACT, field));
            }
        }
        if !is_safe_name(&self.domain_name, MAX_SAFE_NAME_BYTES) {
            return Err(WireError::invalid(CONTRACT, "domain_name"));
        }
        if self.cleaned_unix_seconds == 0 {
            return Err(WireError::invalid(CONTRACT, "cleaned_unix_seconds"));
        }
        Ok(())
    }

    #[must_use]
    pub const fn allocation_id(&self) -> Uuid {
        self.allocation_id
    }

    #[must_use]
    pub const fn service_instance(&self) -> Uuid {
        self.service_instance
    }

    #[must_use]
    pub fn domain_name(&self) -> &str {
        &self.domain_name
    }

    #[must_use]
    pub const fn domain_uuid(&self) -> Uuid {
        self.domain_uuid
    }
}

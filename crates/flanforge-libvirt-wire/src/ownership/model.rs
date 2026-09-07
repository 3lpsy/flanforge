use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::WireError;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OwnershipManifest {
    pub(super) schema_version: u8,
    pub(super) allocation_id: Uuid,
    pub(super) service_instance: Uuid,
    pub(super) domain_name: String,
    pub(super) domain_uuid: Uuid,
    pub(super) mac_address: String,
    pub(super) overlay: Artifact,
    pub(super) seed: Artifact,
    pub(super) host_key_alias: String,
    pub(super) known_hosts_file: PathBuf,
    pub(super) created_unix_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub(super) name: String,
    pub(super) key: Option<String>,
}

impl Artifact {
    #[must_use]
    pub fn new(name: String) -> Self {
        Self { name, key: None }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }

    pub fn set_key(&mut self, key: String) {
        self.key = Some(key);
    }
}

impl OwnershipManifest {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        allocation_id: Uuid,
        service_instance: Uuid,
        domain_name: String,
        domain_uuid: Uuid,
        mac_address: String,
        overlay: Artifact,
        seed: Artifact,
        host_key_alias: String,
        known_hosts_file: PathBuf,
        created_unix_seconds: u64,
    ) -> Self {
        Self {
            schema_version: 1,
            allocation_id,
            service_instance,
            domain_name,
            domain_uuid,
            mac_address,
            overlay,
            seed,
            host_key_alias,
            known_hosts_file,
            created_unix_seconds,
        }
    }

    /// Applies structural validation owned by the wire contract.
    ///
    /// # Errors
    /// Returns the first invalid field in the v1 ownership document.
    pub fn ensure_valid(&self) -> Result<(), WireError> {
        super::validate::ensure_valid(self)
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

    #[must_use]
    pub fn mac_address(&self) -> &str {
        &self.mac_address
    }

    #[must_use]
    pub const fn overlay(&self) -> &Artifact {
        &self.overlay
    }

    #[must_use]
    pub const fn seed(&self) -> &Artifact {
        &self.seed
    }

    #[must_use]
    pub fn host_key_alias(&self) -> &str {
        &self.host_key_alias
    }

    #[must_use]
    pub fn known_hosts_file(&self) -> &Path {
        &self.known_hosts_file
    }

    #[must_use]
    pub const fn created_unix_seconds(&self) -> u64 {
        self.created_unix_seconds
    }

    #[must_use]
    pub const fn overlay_mut(&mut self) -> &mut Artifact {
        &mut self.overlay
    }

    #[must_use]
    pub const fn seed_mut(&mut self) -> &mut Artifact {
        &mut self.seed
    }
}

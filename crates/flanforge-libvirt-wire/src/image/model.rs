use serde::{Deserialize, Serialize};

use crate::WireError;

use super::{
    MAX_BASE_IMAGE_MANIFEST_BYTES,
    guest::{GUEST_CONTRACT_AGENT_CHANNEL, Guest},
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
/// What flanforge knows about a base image.
///
/// Only `image` is required, because only `image` describes the artifact the
/// runtime actually boots — and every field of it can be read off the file.
/// The rest is provenance the Packer build records; an operator who built a
/// qcow2 another way simply omits it. Unknown fields are still rejected, so a
/// typo in a full manifest is an error rather than a silent default.
pub struct BaseImageManifest {
    #[serde(default = "default_schema_version")]
    pub(super) schema_version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) source: Option<ImageSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) builder: Option<ImageBuilder>,
    pub(super) image: Image,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) provenance: Option<Provenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) guest: Option<Guest>,
}

const fn default_schema_version() -> u8 {
    1
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ImageSource {
    pub(super) os_image_url: String,
    pub(super) os_image_sha256: String,
    pub(super) repository_revision: String,
    pub(super) repository_dirty: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ImageBuilder {
    pub(super) packer_version: String,
    pub(super) qemu_plugin_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Image {
    pub(super) file: String,
    pub(super) format: String,
    pub(super) sha256: String,
    pub(super) bytes: u64,
    pub(super) virtual_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Provenance {
    pub(super) runner_release: RunnerRelease,
    pub(super) dependency_proxy: DependencyProxy,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RunnerRelease {
    pub(super) api_url: String,
    pub(super) download_base_url: String,
    pub(super) signing_primary_fingerprint: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DependencyProxy {
    pub(super) configured: bool,
    pub(super) ca_sha256: Option<String>,
}

impl BaseImageManifest {
    /// Decodes and validates the bounded v1 image contract.
    ///
    /// # Errors
    /// Returns a typed wire error for malformed, oversized, or unsupported data.
    pub fn parse(bytes: &[u8]) -> Result<Self, WireError> {
        if bytes.is_empty() || bytes.len() > MAX_BASE_IMAGE_MANIFEST_BYTES {
            return Err(WireError::invalid("base image manifest", "size"));
        }
        let manifest: Self =
            serde_json::from_slice(bytes).map_err(|_| WireError::decode("base image manifest"))?;
        manifest.ensure_valid()?;
        Ok(manifest)
    }

    /// Applies structural validation without changing the serialized contract.
    ///
    /// # Errors
    /// Returns the first structurally invalid manifest field.
    pub fn ensure_valid(&self) -> Result<(), WireError> {
        super::validate::ensure_valid(self)
    }

    #[must_use]
    pub fn image_file(&self) -> &str {
        &self.image.file
    }

    #[must_use]
    pub const fn image_bytes(&self) -> u64 {
        self.image.bytes
    }

    #[must_use]
    pub const fn virtual_bytes(&self) -> u64 {
        self.image.virtual_bytes
    }

    #[must_use]
    pub fn image_sha256(&self) -> &str {
        &self.image.sha256
    }

    #[must_use]
    pub fn image_format(&self) -> &str {
        &self.image.format
    }

    /// `None` when the manifest does not describe the guest, which is not the
    /// same as describing an unsupported one.
    #[must_use]
    pub fn guest_os_id(&self) -> Option<&str> {
        self.guest.as_ref().map(|guest| guest.os.id.as_str())
    }

    #[must_use]
    pub fn guest_architecture(&self) -> Option<&str> {
        self.guest
            .as_ref()
            .map(|guest| guest.os.architecture.as_str())
    }

    #[must_use]
    pub fn is_podman_rootless(&self) -> Option<bool> {
        self.guest.as_ref().map(|guest| guest.podman.rootless)
    }

    #[must_use]
    pub fn is_docker_api_available(&self) -> Option<bool> {
        self.guest.as_ref().map(|guest| guest.podman.docker_api)
    }

    /// The guest contract this base was built to, or `None` when the manifest
    /// does not describe the guest at all.
    #[must_use]
    pub fn guest_contract_version(&self) -> Option<u8> {
        self.guest.as_ref().map(|guest| guest.contract_version)
    }

    /// Whether this base publishes the guest agent's exec channel.
    ///
    /// `None` is a manifest that does not describe the guest, which is a
    /// hand-built image the runtime probes rather than refuses. `Some(false)`
    /// is a positive report that the channel is unusable: every contract-1
    /// base predates it, and a contract-2 base can still record it blocked.
    #[must_use]
    pub fn is_guest_exec_enabled(&self) -> Option<bool> {
        self.guest.as_ref().map(|guest| {
            guest.contract_version >= GUEST_CONTRACT_AGENT_CHANNEL
                && guest.agent.as_ref().is_some_and(|agent| agent.exec_enabled)
        })
    }

    /// The unprivileged account the base bakes in, once it records one.
    #[must_use]
    pub fn guest_job_account(&self) -> Option<(&str, u32)> {
        self.guest
            .as_ref()?
            .job_account
            .as_ref()
            .map(|account| (account.name.as_str(), account.uid))
    }

    /// The privileged automation account, when the build baked one.
    #[must_use]
    pub fn guest_privileged_account(&self) -> Option<(&str, u32)> {
        self.guest
            .as_ref()?
            .privileged_account
            .as_ref()
            .map(|account| (account.name.as_str(), account.uid))
    }

    /// Describes an image the operator supplied directly. Every field here is
    /// read off the file itself, which is why no provenance is invented.
    #[must_use]
    pub fn from_image(
        file: String,
        format: String,
        sha256: String,
        bytes: u64,
        virtual_bytes: u64,
    ) -> Self {
        Self {
            schema_version: default_schema_version(),
            created_at: None,
            source: None,
            builder: None,
            image: Image {
                file,
                format,
                sha256,
                bytes,
                virtual_bytes,
            },
            provenance: None,
            guest: None,
        }
    }
}

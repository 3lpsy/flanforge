use std::{net::IpAddr, path::PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{OwnershipManifest, PublishedBase, VolumePointer, WireError};

use super::{MAX_LIBVIRT_HELPER_FAILURE_BYTES, MAX_LIBVIRT_HELPER_REPLY_BYTES};

pub const LIBVIRT_HELPER_ARGUMENT: &str = "__libvirt-helper";

mod agent;
mod request;

pub use agent::{AgentExecOutcome, AgentExecRequest};
pub use request::HelperRequest;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HelperConfig {
    pub uri: String,
    pub pool: String,
    pub network: String,
    pub state_dir: PathBuf,
    pub service_instance: Uuid,
    pub min_storage_free_bytes: u64,
    /// Carried across the boundary so the contract admits exactly the
    /// transports the operator configured, not a wider set.
    pub allow_insecure_transport: bool,
    /// Warm images require a directory-backed pool, which only the helper can
    /// see. The probe refuses at startup rather than deep inside a promotion.
    #[serde(default)]
    pub is_warm_declared: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HelperMachineState {
    Running,
    Stopped,
    Other,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HelperMachineOwnership {
    Owned,
    Foreign,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HelperGuestSize {
    pub cpu_count: u8,
    pub memory_mb: u32,
    pub storage_mb: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HelperMachine {
    pub name: String,
    pub state: HelperMachineState,
    /// How long the daemon has owned this guest. Without it the sweep has no
    /// candidate it may act on, so the helper has to carry it across.
    #[serde(default)]
    pub age_seconds: Option<u64>,
    pub size: Option<HelperGuestSize>,
    pub ownership: HelperMachineOwnership,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HelperFailureCode {
    InvalidRequest,
    Configuration,
    Unavailable,
    Deadline,
    NotFound,
    Transient,
    Capacity,
    Conflict,
    Libvirt,
    Ownership,
    Manifest,
    Seed,
    Internal,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HelperFailure {
    code: HelperFailureCode,
    message: String,
}

impl HelperFailure {
    #[must_use]
    pub fn new(code: HelperFailureCode, message: impl std::fmt::Display) -> Self {
        let message = message
            .to_string()
            .chars()
            .map(|character| {
                if character.is_ascii_control() {
                    ' '
                } else {
                    character
                }
            })
            .collect::<String>();
        let message = flanforge_utils::bounded_text(message, MAX_LIBVIRT_HELPER_FAILURE_BYTES);
        Self { code, message }
    }

    #[must_use]
    pub const fn code(&self) -> HelperFailureCode {
        self.code
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "result", content = "value", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum HelperReply {
    Unit,
    Inventory(Vec<HelperMachine>),
    Manifest(OwnershipManifest),
    Address(Option<IpAddr>),
    Published(PublishedBase),
    Warm(VolumePointer),
    /// Volume keys the retirement proof cleared and deleted.
    Retired(Vec<String>),
    /// Whether the live agent will run `guest-exec`. A blocked RPC is a fact
    /// about the image, never a transient condition.
    AgentProbe(bool),
    /// The guest pid `guest-exec` started, which only `guest-exec-status` on
    /// the same domain can interpret.
    AgentStarted(i64),
    AgentOutcome(AgentExecOutcome),
    Error(HelperFailure),
}

impl HelperReply {
    #[must_use]
    pub fn error(code: HelperFailureCode, message: impl std::fmt::Display) -> Self {
        Self::Error(HelperFailure::new(code, message))
    }

    /// Serializes one structurally validated helper reply.
    ///
    /// # Errors
    /// Returns an error when the reply is invalid or oversized.
    pub fn encode(&self) -> Result<Vec<u8>, WireError> {
        super::validate::reply(self)?;
        let bytes = serde_json::to_vec(self).map_err(|_| WireError::encode("libvirt helper"))?;
        if bytes.len() > MAX_LIBVIRT_HELPER_REPLY_BYTES {
            return Err(WireError::invalid("libvirt helper", "reply size"));
        }
        Ok(bytes)
    }

    /// Parses one bounded, structurally validated helper reply.
    ///
    /// # Errors
    /// Returns an error for malformed, oversized, or invalid replies.
    pub fn parse(bytes: &[u8]) -> Result<Self, WireError> {
        if bytes.is_empty() || bytes.len() > MAX_LIBVIRT_HELPER_REPLY_BYTES {
            return Err(WireError::invalid("libvirt helper", "reply size"));
        }
        let reply =
            serde_json::from_slice(bytes).map_err(|_| WireError::decode("libvirt helper reply"))?;
        super::validate::reply(&reply)?;
        Ok(reply)
    }
}

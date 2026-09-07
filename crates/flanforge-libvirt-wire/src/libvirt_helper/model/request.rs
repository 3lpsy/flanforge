use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{BaseImageManifest, OwnershipManifest, VolumePointer, WireError};

use super::super::MAX_LIBVIRT_HELPER_REQUEST_BYTES;
use super::{AgentExecRequest, HelperConfig};

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
#[allow(clippy::large_enum_variant)]
pub enum HelperRequest {
    Probe {
        config: HelperConfig,
    },
    Inventory {
        config: HelperConfig,
    },
    CheckSource {
        config: HelperConfig,
        pointer: VolumePointer,
    },
    Create {
        config: HelperConfig,
        manifest: OwnershipManifest,
        source: VolumePointer,
        seed: Vec<u8>,
        storage_bytes: u64,
    },
    Define {
        config: HelperConfig,
        manifest: OwnershipManifest,
        cpu_count: u8,
        memory_mb: u32,
    },
    Start {
        config: HelperConfig,
        manifest: OwnershipManifest,
    },
    Address {
        config: HelperConfig,
        manifest: OwnershipManifest,
        timeout_seconds: u8,
    },
    Cleanup {
        config: HelperConfig,
        manifest: OwnershipManifest,
        timeout_seconds: u16,
    },
    Import {
        config: HelperConfig,
        logical_name: String,
        volume_name: String,
        staged_image_path: PathBuf,
        manifest: BaseImageManifest,
    },
    /// A `None` key is the crash window between creating a volume and
    /// recording it: the name still carries this operation's own artifact id,
    /// so it cannot name another tool's volume.
    DeleteVolume {
        config: HelperConfig,
        key: Option<String>,
        name: String,
    },
    /// Ordered shutdown with no undefine and no volume deletion. A guest that
    /// will not stop gracefully is an error, never a forced power-off: the
    /// identity reset is verified through the live filesystem, so a
    /// crash-consistent capture could invalidate that verification.
    WarmQuiesce {
        config: HelperConfig,
        manifest: OwnershipManifest,
        timeout_seconds: u16,
    },
    /// Flattens this allocation's quiesced overlay into a new standalone
    /// volume named `warm-<capture_id>.qcow2`. Purely additive: nothing a
    /// consumer could be reading is replaced or deleted.
    WarmCapture {
        config: HelperConfig,
        manifest: OwnershipManifest,
        profile: String,
        capture_id: Uuid,
        generation: u64,
        virtual_bytes: u64,
    },
    /// Re-reads the captured volume from libvirt and from its own bytes.
    WarmVerify {
        config: HelperConfig,
        profile: String,
        capture_id: Uuid,
        generation: u64,
        produced_at_unix: u64,
        virtual_bytes: u64,
    },
    /// Proves the guest agent is alive and reports whether it will run the
    /// exec channel. Both in one helper process: they are asked together, and a
    /// second process would double the cost of every readiness poll.
    AgentProbe {
        config: HelperConfig,
        manifest: OwnershipManifest,
        timeout_seconds: u8,
    },
    /// Runs one command in a booted guest over its QEMU guest agent, and
    /// returns the guest pid it started. The agent channel is virtio-serial and
    /// host-local to the hypervisor, so it reaches a guest no route reaches.
    AgentExec {
        config: HelperConfig,
        manifest: OwnershipManifest,
        request: AgentExecRequest,
        timeout_seconds: u8,
    },
    /// Reads one started command's status. The agent frees its record on the
    /// first reply reporting `exited`, so the caller reads a terminal outcome
    /// once and caches it.
    AgentExecStatus {
        config: HelperConfig,
        manifest: OwnershipManifest,
        pid: i64,
        timeout_seconds: u8,
    },
    /// Proves and deletes in one call: within this daemon no overlay can be
    /// created between proving a generation unreferenced and deleting it,
    /// because every mutating request serializes behind one actor mutex.
    WarmRetire {
        config: HelperConfig,
        candidates: Vec<VolumePointer>,
        protected: Vec<VolumePointer>,
    },
}

impl HelperRequest {
    /// Serializes one structurally validated helper request.
    ///
    /// # Errors
    /// Returns an error when any field violates the helper contract.
    pub fn encode(&self) -> Result<Vec<u8>, WireError> {
        super::super::validate::request(self)?;
        let bytes = serde_json::to_vec(self).map_err(|_| WireError::encode("libvirt helper"))?;
        if bytes.len() > MAX_LIBVIRT_HELPER_REQUEST_BYTES {
            return Err(WireError::invalid("libvirt helper", "request size"));
        }
        Ok(bytes)
    }

    /// Parses one bounded, structurally validated helper request.
    ///
    /// # Errors
    /// Returns an error for malformed, oversized, or invalid requests.
    pub fn parse(bytes: &[u8]) -> Result<Self, WireError> {
        if bytes.is_empty() || bytes.len() > MAX_LIBVIRT_HELPER_REQUEST_BYTES {
            return Err(WireError::invalid("libvirt helper", "request size"));
        }
        let request = serde_json::from_slice(bytes)
            .map_err(|_| WireError::decode("libvirt helper request"))?;
        super::super::validate::request(&request)?;
        Ok(request)
    }
}

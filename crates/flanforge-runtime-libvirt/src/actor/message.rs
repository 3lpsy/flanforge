use std::path::PathBuf;

use flanforge_libvirt_wire::{BaseImageManifest, OwnershipManifest, VolumePointer};

#[derive(Debug)]
pub(crate) struct CreateRequest {
    pub(crate) manifest: OwnershipManifest,
    pub(crate) source: VolumePointer,
    pub(crate) seed: Vec<u8>,
    pub(crate) storage_bytes: u64,
}

#[derive(Debug)]
pub(crate) struct DefineRequest {
    pub(crate) manifest: OwnershipManifest,
    pub(crate) cpu_count: u8,
    pub(crate) memory_mb: u32,
}

#[derive(Debug)]
pub(crate) struct ImportRequest {
    pub(crate) logical_name: String,
    pub(crate) volume_name: String,
    pub(crate) staged_image_path: PathBuf,
    pub(crate) manifest: BaseImageManifest,
}

#[derive(Debug)]
pub(crate) struct WarmCaptureRequest {
    pub(crate) manifest: OwnershipManifest,
    pub(crate) profile: String,
    pub(crate) capture_id: uuid::Uuid,
    pub(crate) generation: u64,
    pub(crate) virtual_bytes: u64,
}

#[derive(Debug)]
pub(crate) struct WarmVerifyRequest {
    pub(crate) profile: String,
    pub(crate) capture_id: uuid::Uuid,
    pub(crate) generation: u64,
    pub(crate) produced_at_unix: u64,
    pub(crate) virtual_bytes: u64,
}

pub(crate) type ActorConfig = flanforge_libvirt_wire::HelperConfig;

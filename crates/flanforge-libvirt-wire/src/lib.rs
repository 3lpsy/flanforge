mod error;
mod image;
mod libvirt_helper;
mod ownership;
mod validation;
mod volume_checkpoint;
mod warm;

pub use error::WireError;
pub use image::{
    BaseImageManifest, MAX_BASE_IMAGE_MANIFEST_BYTES, MAX_PUBLISHED_BASE_BYTES, PublishedBase,
    VolumePointer,
};
pub use libvirt_helper::{
    AgentExecOutcome, AgentExecRequest, HelperConfig, HelperFailure, HelperFailureCode,
    HelperGuestSize, HelperMachine, HelperMachineOwnership, HelperMachineState, HelperReply,
    HelperRequest, LIBVIRT_HELPER_ARGUMENT, MAX_LIBVIRT_HELPER_FAILURE_BYTES,
    MAX_LIBVIRT_HELPER_REPLY_BYTES, MAX_LIBVIRT_HELPER_REQUEST_BYTES,
};
pub use ownership::{
    Artifact, CleanupTombstone, DomainOwnershipMetadata, LIBVIRT_OWNERSHIP_METADATA_URI,
    MAX_CLEANUP_TOMBSTONE_BYTES, MAX_DOMAIN_OWNERSHIP_METADATA_BYTES, MAX_OWNERSHIP_MANIFEST_BYTES,
    OwnershipManifest,
};
pub use volume_checkpoint::{
    CheckpointOwner, CheckpointVolume, CheckpointVolumeRole, MAX_VOLUME_CHECKPOINT_BYTES,
    VolumeCheckpoint,
};
pub use warm::{
    MAX_PUBLISHED_WARM_BYTES, MAX_RETAINED_WARM_GENERATIONS, MAX_SUPERSEDED_POINTERS, PublishedWarm,
};

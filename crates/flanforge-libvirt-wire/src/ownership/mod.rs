mod cleanup;
mod domain;
mod limits;
mod model;
mod validate;

pub use cleanup::CleanupTombstone;
pub use domain::DomainOwnershipMetadata;
pub use limits::{
    LIBVIRT_OWNERSHIP_METADATA_URI, MAX_CLEANUP_TOMBSTONE_BYTES,
    MAX_DOMAIN_OWNERSHIP_METADATA_BYTES, MAX_OWNERSHIP_MANIFEST_BYTES,
};
pub use model::{Artifact, OwnershipManifest};

#[cfg(test)]
mod tests;

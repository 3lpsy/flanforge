pub const LIBVIRT_OWNERSHIP_METADATA_URI: &str = "urn:flanforge:allocation:v1";
pub const MAX_CLEANUP_TOMBSTONE_BYTES: usize = 4 * 1_024;
pub const MAX_DOMAIN_OWNERSHIP_METADATA_BYTES: usize = 8 * 1_024;
pub const MAX_OWNERSHIP_MANIFEST_BYTES: usize = 16 * 1_024;

pub(super) const MAX_ARTIFACT_KEY_BYTES: usize = 1_024;
pub(super) const MAX_KNOWN_HOSTS_PATH_BYTES: usize = 4_096;
pub(super) const MAX_SAFE_NAME_BYTES: usize = 128;

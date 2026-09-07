pub const MAX_BASE_IMAGE_MANIFEST_BYTES: usize = 64 * 1_024;
pub const MAX_PUBLISHED_BASE_BYTES: usize = 96 * 1_024;

pub(super) const MAX_IMAGE_FILE_BYTES: usize = 128;
pub(super) const MAX_IMAGE_FORMAT_BYTES: usize = 32;
pub(super) const MAX_LOGICAL_NAME_BYTES: usize = 128;
pub(super) const MAX_VOLUME_KEY_BYTES: usize = 4_096;
pub(crate) const MAX_VIRTUAL_IMAGE_BYTES: u64 = 1_024_u64.pow(4);

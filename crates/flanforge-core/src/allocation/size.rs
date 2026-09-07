use serde::{Deserialize, Serialize};
use validator::Validate;

/// Guest sizing resolved once, beneath the profile ceiling.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, Validate)]
pub struct GuestSize {
    #[validate(range(min = 1, max = 64))]
    pub cpu_count: u8,
    #[validate(range(min = 2_048, max = 131_072))]
    pub memory_mb: u32,
    #[serde(default = "default_storage_mb")]
    #[validate(range(min = 16_384, max = 1_048_576))]
    pub storage_mb: u64,
}

impl GuestSize {
    /// The validated `storage_mb` ceiling. A boot source larger than the
    /// profile raises the charged size, and that raise clamps here rather than
    /// producing a record that cannot validate.
    pub const MAX_STORAGE_MB: u64 = 1_048_576;
}

const fn default_storage_mb() -> u64 {
    40_960
}

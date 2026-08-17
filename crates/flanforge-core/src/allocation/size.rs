use serde::{Deserialize, Serialize};
use validator::Validate;

/// Guest sizing resolved once, beneath the profile ceiling.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, Validate)]
pub struct GuestSize {
    #[validate(range(min = 1, max = 64))]
    pub cpu_count: u8,
    #[validate(range(min = 2_048, max = 131_072))]
    pub memory_mb: u32,
}

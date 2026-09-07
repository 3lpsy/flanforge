pub const MAX_PUBLISHED_WARM_BYTES: usize = 64 * 1_024;

/// Structural bound, so the document stays bounded whatever policy allows.
pub const MAX_SUPERSEDED_POINTERS: usize = 8;

/// Policy cap: promotion refuses rather than growing the pool past this many
/// superseded generations awaiting a provable retirement. New in this crate.
pub const MAX_RETAINED_WARM_GENERATIONS: usize = 3;

pub(super) const MAX_PROFILE_BYTES: usize = 128;
pub(super) const MAX_LOGICAL_NAME_BYTES: usize = 128;
